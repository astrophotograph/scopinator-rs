//! The Seestar mount's state queries (`is_slewing`, `is_parked`,
//! `is_tracking`, `get_mount_status`), through `SeestarBackend`.
//!
//! What the fake telescope answers follows an S30 on firmware 9.75
//! (`docs/protocol/hardware-observations.md`): `mount.move_type` is
//! `ScopeGoto` during a slew and `none` otherwise, `mount.close` is true
//! when parked, and the view state nests the goto's stages as
//! `View` > `AutoGoto` > `ScopeGoto`.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::FakeSeestar;
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{Mount, ScopinatorError, UnifiedEventBus};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{Coordinates, SlewState};
use serde_json::{Value, json};

fn mount_state(move_type: &str, tracking: bool, close: bool) -> Value {
    json!({
        "device": {
            "name": "fake", "firmware_ver_int": 2975, "firmware_ver_string": "9.75",
            "product_model": "Seestar S30",
        },
        "mount": {"move_type": move_type, "tracking": tracking, "close": close, "equ_mode": false},
    })
}

async fn backend(fake: &FakeSeestar) -> SeestarBackend {
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

fn target() -> Coordinates {
    Coordinates::from_hours(20.69, 45.28).unwrap()
}

#[tokio::test]
async fn an_idle_unparked_mount_is_neither_slewing_nor_parked() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    assert!(!mount.is_slewing().await.unwrap());
    assert!(!mount.is_parked().await.unwrap());
    assert!(mount.is_tracking().await.unwrap());
    let status = mount.get_mount_status().await.unwrap();
    assert_eq!(status.slew_state, SlewState::Tracking);
    assert!(status.is_tracking);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_parked_mount_reports_parked() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    // `get_view_state` is `{}` when parked.
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    assert!(mount.is_parked().await.unwrap());
    assert!(!mount.is_slewing().await.unwrap());
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Parked
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn slewing_from_acceptance_until_the_goto_completes() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    assert!(!mount.is_slewing().await.unwrap());

    // Accepted: the telescope says nothing about a goto yet, and the mount
    // has not started moving. That is still a slew.
    mount.slew_to_coordinates(&target()).await.unwrap();
    assert!(mount.is_slewing().await.unwrap());

    // Moving.
    fake.respond_ok("get_device_state", mount_state("ScopeGoto", false, false));
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "ScopeGoto": {"state": "working"}}}}),
    );
    assert!(mount.is_slewing().await.unwrap());
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Slewing
    );

    // The mount has stopped but the telescope is still plate solving and
    // re-centring: not ready yet.
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "PlateSolve": {"state": "working"}}}}),
    );
    assert!(mount.is_slewing().await.unwrap());

    // Centred.
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "complete"}}}),
    );
    // The telescope was seen working on the goto, so the time allowed for
    // it to start no longer applies: the slew is over now.
    assert!(!mount.is_slewing().await.unwrap());
    assert!(!mount.is_slewing().await.unwrap());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_goto_that_fails_under_way_is_reported_once() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    mount.slew_to_coordinates(&target()).await.unwrap();

    // What an S30 with an unset clock showed 9 s after accepting a goto: the
    // outer stage still working, the inner one failed.
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working",
                "ScopeGoto": {"state": "fail", "code": 501, "error": "mount goto failed"}}}}),
    );
    let err = mount.is_slewing().await.unwrap_err();
    assert!(matches!(err, ScopinatorError::Backend(_)), "{err:?}");
    let text = err.to_string();
    assert!(text.contains("ScopeGoto"), "{text}");
    assert!(text.contains("mount goto failed"), "{text}");
    assert!(text.contains("501"), "{text}");

    // The failed stage stays in the view state; it is not reported again.
    assert!(!mount.is_slewing().await.unwrap());
    backend.disconnect().await;
}

#[tokio::test]
async fn an_aborted_goto_is_not_a_failed_slew() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, false));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    mount.slew_to_coordinates(&target()).await.unwrap();
    mount.abort_slew().await.unwrap();

    // Cancelling a goto leaves `ScopeGoto` fail 207 behind.
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "cancel", "mode": "none",
            "AutoGoto": {"state": "fail", "code": 207, "error": "fail to operate"}}}),
    );
    assert!(!mount.is_slewing().await.unwrap());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_state_reply_without_the_mount_members_is_an_error() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok(
        "get_device_state",
        json!({"device": {"name": "fake", "firmware_ver_int": 2975}, "mount": {}}),
    );
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    // Unknown is not "not parked" and not "not tracking".
    assert!(mount.is_parked().await.is_err());
    assert!(mount.is_tracking().await.is_err());
    assert!(mount.is_slewing().await.is_err());
    backend.disconnect().await;
}
