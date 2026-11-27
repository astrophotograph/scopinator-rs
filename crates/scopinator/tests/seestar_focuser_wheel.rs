//! A Seestar's focuser and filter wheel through the neutral traits. The
//! reply shapes are from an S30 on firmware 9.75.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::FakeSeestar;
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{Backend, ScopinatorError, UnifiedEventBus, WaitOptions, wait_for_focuser};
use scopinator_seestar::SeestarConfig;
use scopinator_types::DeviceKind;
use serde_json::{Value, json};

fn focuser_state(state: &str, step: i32) -> Value {
    json!({"focuser": {"max_step": 2600, "state": state, "step": step}})
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

fn sent(fake: &FakeSeestar, method: &str) -> Vec<Value> {
    fake.received()
        .into_iter()
        .filter(|msg| msg["method"] == method)
        .map(|msg| msg["params"].clone())
        .collect()
}

#[tokio::test]
async fn the_focuser_reports_its_position_travel_and_motion() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", focuser_state("idle", 1253));
    let backend = backend(&fake).await;
    let focuser = Backend::focuser(&backend).expect("a Seestar has a focuser");

    assert_eq!(focuser.id().as_str(), "seestar:127.0.0.1:focuser");
    assert_eq!(focuser.info().kind, DeviceKind::Focuser);
    assert!(focuser.capabilities().absolute);
    let position = focuser.focuser_position().await.unwrap();
    assert_eq!(position.position, 1253);
    assert_eq!(position.max_position, Some(2600));
    assert!(!position.is_moving);
    assert!(!focuser.is_moving().await.unwrap());

    fake.respond_ok("get_device_state", focuser_state("moving", 2379));
    assert!(focuser.is_moving().await.unwrap());
    assert!(focuser.focuser_position().await.unwrap().is_moving);
    backend.disconnect().await;
}

#[tokio::test]
async fn focuser_moves_are_sent_as_absolute_steps_and_kept_inside_the_travel() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", focuser_state("idle", 1253));
    fake.respond_ok("move_focuser", json!({"step": 1273}));
    let backend = backend(&fake).await;
    let focuser = Backend::focuser(&backend).unwrap();

    focuser.move_to(1273).await.unwrap();
    focuser.move_relative(-53).await.unwrap();
    // The fake speaks the verify-marker dialect, which adds a member.
    let moves = sent(&fake, "move_focuser");
    let steps: Vec<_> = moves.iter().map(|m| (&m["step"], &m["ret_step"])).collect();
    assert_eq!(
        steps,
        [(&json!(1273), &json!(true)), (&json!(1200), &json!(true))]
    );
    wait_for_focuser(
        focuser.as_ref(),
        WaitOptions::with_timeout(Duration::from_secs(5)),
    )
    .await
    .unwrap();

    // The telescope would run to the end of its travel; refuse instead.
    for bad in [focuser.move_to(2601).await, focuser.move_to(-1).await] {
        let err = bad.unwrap_err();
        assert!(matches!(err, ScopinatorError::OutOfRange(_)), "{err:?}");
    }
    let err = focuser.move_relative(5000).await.unwrap_err();
    assert!(matches!(err, ScopinatorError::OutOfRange(_)), "{err:?}");
    assert_eq!(sent(&fake, "move_focuser").len(), 2);

    focuser.halt().await.unwrap();
    assert_eq!(sent(&fake, "stop_focuser").len(), 1);
    backend.disconnect().await;
}

#[tokio::test]
async fn the_wheel_has_the_telescopes_names_at_positions_counted_from_zero() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok(
        "get_wheel_setting",
        json!({"exp_sec": [2.0, 2.0, 2.0], "names": ["dark", "IRCUT", "LP"]}),
    );
    fake.respond_ok(
        "get_wheel_state",
        json!({"id": 0, "state": "idle", "unidirection": false}),
    );
    fake.respond_ok("get_wheel_position", json!(1));
    let backend = backend(&fake).await;
    let wheel = Backend::filter_wheel(&backend).expect("a Seestar has a filter wheel");

    assert_eq!(wheel.id().as_str(), "seestar:127.0.0.1:filterwheel");
    assert_eq!(wheel.info().kind, DeviceKind::FilterWheel);
    assert_eq!(
        wheel.get_filter_names().await.unwrap(),
        ["dark", "IRCUT", "LP"]
    );
    assert_eq!(wheel.capabilities().slot_count, 3);
    let filter = wheel.filter_position().await.unwrap().unwrap();
    assert_eq!(
        (filter.position, filter.name.as_deref()),
        (1, Some("IRCUT"))
    );

    wheel.set_filter_position(2).await.unwrap();
    // The position is the first element; the verify marker follows it.
    assert_eq!(sent(&fake, "set_wheel_position")[0][0], json!(2));
    let err = wheel.set_filter_position(3).await.unwrap_err();
    assert!(matches!(err, ScopinatorError::OutOfRange(_)), "{err:?}");
    assert_eq!(sent(&fake, "set_wheel_position").len(), 1);

    // The names are asked for once, whichever handle asks.
    let again = Backend::filter_wheel(&backend).unwrap();
    again.get_filter_names().await.unwrap();
    assert_eq!(sent(&fake, "get_wheel_setting").len(), 1);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_turning_wheel_has_no_filter_in_the_light_path() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok(
        "get_wheel_setting",
        json!({"names": ["dark", "IRCUT", "LP"]}),
    );
    fake.respond_ok("get_wheel_state", json!({"id": 0, "state": "moving"}));
    fake.respond_ok("get_wheel_position", json!(1));
    let backend = backend(&fake).await;
    let wheel = Backend::filter_wheel(&backend).unwrap();

    assert!(wheel.is_moving().await.unwrap());
    assert_eq!(wheel.filter_position().await.unwrap(), None);
    backend.disconnect().await;
}
