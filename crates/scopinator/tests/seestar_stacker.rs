//! Live stacking on a Seestar through the neutral `LiveStacker`.
//!
//! The view-state shapes are from the 6.70 session corpus: the `Stack`
//! stage sits under `View` with its frame counts, and the sub-exposure in
//! use under `Stack` > `Exposure`.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::{FakeFrame, FakeSeestar};
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{
    Backend, LiveStacker, ScopinatorError, UnifiedEventBus, WaitOptions, wait_for_frames,
};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{BayerPattern, FrameKind, PixelFormat, StackSettings, StackState};
use serde_json::{Value, json};

fn star_view(stack: Option<Value>) -> Value {
    let mut view = json!({"state": "working", "lapse_ms": 1000, "mode": "star", "cam_id": 0,
        "target_name": "M44", "lp_filter": false, "gain": 80});
    if let Some(stack) = stack {
        view["Stack"] = stack;
    }
    json!({"View": view})
}

fn stack_stage(state: &str, stacked: u32, dropped: u32) -> Value {
    json!({"state": state, "lapse_ms": 5000, "frame_errcode": 0, "stacked_frame": stacked,
        "dropped_frame": dropped, "can_annotate": true,
        "Exposure": {"state": "working", "lapse_ms": 100, "exp_ms": 10000.0},
        "stage": "Exposure"})
}

async fn backend(fake: &FakeSeestar) -> SeestarBackend {
    fake.respond_ok(
        "get_device_state",
        json!({
            "device": {"name": "fake", "firmware_ver_int": 2670, "product_model": "Seestar S50"},
            "camera": {"chip_size": [1080, 1920], "debayer_pattern": "GR"},
        }),
    );
    fake.emulate_settings(json!({
        "exp_ms": {"stack_l": 10000, "continuous": 500},
        "stack_lenhance": false,
    }));
    let endpoints = SeestarEndpoints {
        control: fake.control_addr,
        imaging: fake.imaging_addr,
        wide_control: fake.control_addr,
        wide_imaging: fake.imaging_addr,
    };
    let backend = SeestarBackend::connect_with_endpoints(
        Ipv4Addr::LOCALHOST,
        endpoints,
        SeestarConfig::default(),
        Arc::new(UnifiedEventBus::new(16)),
    )
    .await
    .expect("backend");
    backend
        .client()
        .wait_for_connection(Duration::from_secs(2))
        .await
        .expect("connection");
    backend
}

fn stacker(backend: &SeestarBackend) -> Arc<dyn LiveStacker> {
    Backend::live_stacker(backend).expect("a Seestar has a live stacker")
}

/// The parameters of every request for `method`, in order.
fn sent(fake: &FakeSeestar, method: &str) -> Vec<Value> {
    fake.received()
        .into_iter()
        .filter(|msg| msg["method"] == method)
        .map(|msg| msg["params"].clone())
        .collect()
}

#[tokio::test]
async fn stacking_starts_on_a_running_star_view() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    let backend = backend(&fake).await;
    let stacker = stacker(&backend);

    stacker
        .start_stacking(&StackSettings::default())
        .await
        .unwrap();
    assert_eq!(sent(&fake, "iscope_start_stack").len(), 1);
    // Nothing was asked for, so no setting was touched.
    assert!(sent(&fake, "set_setting").is_empty());
    assert!(sent(&fake, "set_control_value").is_empty());
    backend.disconnect().await;
}

#[tokio::test]
async fn without_a_star_view_there_is_nothing_to_stack() {
    for view in [
        json!({}),
        json!({"View": {"state": "cancel", "mode": "star"}}),
        json!({"View": {"state": "working", "mode": "scenery"}}),
    ] {
        let fake = FakeSeestar::start().await;
        fake.respond_ok("get_view_state", view);
        let backend = backend(&fake).await;
        let err = stacker(&backend)
            .start_stacking(&StackSettings::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");
        assert!(sent(&fake, "iscope_start_stack").is_empty());
        backend.disconnect().await;
    }
}

#[tokio::test]
async fn exposure_filter_gain_and_restart_are_applied_before_the_stack_starts() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    let backend = backend(&fake).await;

    let mut settings = StackSettings::default();
    settings.exposure = Some(Duration::from_secs(20));
    settings.filter = Some("LP".into());
    settings.gain = Some(120);
    settings.restart = true;
    stacker(&backend).start_stacking(&settings).await.unwrap();

    assert_eq!(fake.setting("exp_ms").unwrap()["stack_l"], 20000);
    assert_eq!(fake.setting("stack_lenhance"), Some(json!(true)));
    // (The connection may append its `verify` marker to the array.)
    let gain = &sent(&fake, "set_control_value")[0];
    assert_eq!((&gain[0], &gain[1]), (&json!("gain"), &json!(120)));
    assert_eq!(sent(&fake, "iscope_start_stack")[0]["restart"], true);

    // The stack request is the last thing sent.
    let methods: Vec<String> = fake
        .received()
        .iter()
        .filter_map(|msg| msg["method"].as_str().map(str::to_string))
        .collect();
    assert_eq!(
        methods.last().map(String::as_str),
        Some("iscope_start_stack")
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn settings_a_seestar_cannot_honour_are_refused_before_anything_changes() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    let backend = backend(&fake).await;
    let stacker = stacker(&backend);
    assert!(!stacker.capabilities().can_limit_frames);
    assert_eq!(stacker.capabilities().filters, ["IR cut", "LP"]);

    let mut limited = StackSettings::default();
    limited.frame_limit = Some(60);
    limited.exposure = Some(Duration::from_secs(20));
    assert!(
        stacker
            .start_stacking(&limited)
            .await
            .unwrap_err()
            .is_not_supported()
    );

    let mut filtered = StackSettings::default();
    filtered.filter = Some("H-alpha".into());
    assert!(matches!(
        stacker.start_stacking(&filtered).await,
        Err(ScopinatorError::InvalidArgument(_))
    ));

    let mut zero = StackSettings::default();
    zero.exposure = Some(Duration::ZERO);
    assert!(matches!(
        stacker.start_stacking(&zero).await,
        Err(ScopinatorError::OutOfRange(_))
    ));

    // The exposure sent along with the refused frame limit was not applied.
    assert_eq!(fake.setting("exp_ms").unwrap()["stack_l"], 10000);
    assert!(sent(&fake, "iscope_start_stack").is_empty());
    backend.disconnect().await;
}

#[tokio::test]
async fn progress_follows_the_stack_and_the_wait_ends_at_the_frame_count() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    let backend = backend(&fake).await;
    let stacker = stacker(&backend);
    assert_eq!(stacker.progress().await.unwrap().state, StackState::Idle);

    stacker
        .start_stacking(&StackSettings::default())
        .await
        .unwrap();
    fake.respond_ok(
        "get_view_state",
        star_view(Some(stack_stage("working", 3, 1))),
    );
    let progress = stacker.progress().await.unwrap();
    assert_eq!(progress.state, StackState::Stacking);
    assert_eq!((progress.stacked, progress.dropped), (3, 1));
    assert_eq!(progress.integration, Some(Duration::from_secs(30)));

    let opts = WaitOptions {
        poll_interval: Duration::from_millis(20),
        ..WaitOptions::default()
    };
    let waiting = tokio::spawn({
        let stacker = Arc::clone(&stacker);
        async move { wait_for_frames(stacker.as_ref(), 10, opts).await }
    });
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!waiting.is_finished());
    fake.respond_ok(
        "get_view_state",
        star_view(Some(stack_stage("working", 10, 2))),
    );
    assert_eq!(waiting.await.unwrap().unwrap().stacked, 10);

    stacker.stop_stacking().await.unwrap();
    assert_eq!(
        sent(&fake, "iscope_stop_view").last().unwrap()["stage"],
        "Stack"
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn a_stack_that_fails_is_reported_once() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    let backend = backend(&fake).await;
    let stacker = stacker(&backend);
    stacker
        .start_stacking(&StackSettings::default())
        .await
        .unwrap();

    fake.respond_ok(
        "get_view_state",
        star_view(Some(
            json!({"state": "fail", "error": "no star detected", "code": 268}),
        )),
    );
    // Through a second handle too: the backend's stacker is one stacker.
    let err = Backend::live_stacker(&backend)
        .unwrap()
        .progress()
        .await
        .unwrap_err();
    assert!(err.to_string().contains("no star detected"), "{err}");
    assert_eq!(stacker.progress().await.unwrap().state, StackState::Idle);
    backend.disconnect().await;
}

#[tokio::test]
async fn the_stacked_image_is_the_stack_and_no_stack_is_an_invalid_state() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok(
        "get_view_state",
        star_view(Some(stack_stage("working", 5, 0))),
    );
    let pixels: Vec<u8> = (0..16).collect();
    fake.respond_image(
        "get_stacked_img",
        vec![FakeFrame::image(2, 1, 4, 2, pixels.clone())],
    );
    let backend = backend(&fake).await;
    // The Bayer pattern comes from the device state; wait for it.
    for _ in 0..100 {
        if backend.device_info().debayer_pattern.is_some() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }

    let image = stacker(&backend).stacked_image().await.unwrap();
    assert_eq!((image.width, image.height), (4, 2));
    assert_eq!(image.format, PixelFormat::Bayer16(BayerPattern::Grbg));
    assert_eq!(image.frame_kind, FrameKind::Stack);
    assert_eq!(&image.data[..], &pixels[..]);
    backend.disconnect().await;

    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_view_state", star_view(None));
    fake.respond_image(
        "get_stacked_img",
        vec![FakeFrame::error("there is no image now")],
    );
    let backend = self::backend(&fake).await;
    let err = stacker(&backend).stacked_image().await.unwrap_err();
    assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");
    backend.disconnect().await;
}
