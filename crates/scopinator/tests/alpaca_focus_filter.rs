//! An Alpaca focuser and filter wheel through the neutral traits.
#![cfg(feature = "alpaca")]

mod alpaca_common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use alpaca_common::FakeAlpaca;
use scopinator::backend::alpaca::{AlpacaFilterWheelDevice, AlpacaFocuserDevice};
use scopinator::{
    Device, FilterWheel, Focuser, ScopinatorError, WaitOptions, wait_for_filter_wheel,
    wait_for_focuser,
};
use scopinator_alpaca::{AlpacaClient, AlpacaFilterWheel, AlpacaFocuser};
use scopinator_types::DeviceId;
use serde_json::json;

fn client(fake: &FakeAlpaca) -> (Arc<AlpacaClient>, DeviceId) {
    (
        Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, fake.addr.port())),
        DeviceId::new(format!("alpaca:127.0.0.1:{}", fake.addr.port())),
    )
}

async fn focuser(fake: &FakeAlpaca) -> AlpacaFocuserDevice {
    let (client, id) = client(fake);
    let focuser = AlpacaFocuserDevice::new(AlpacaFocuser::new(client, 0), &id);
    focuser.connect().await.expect("connect");
    focuser
}

fn absolute_focuser(fake: &FakeAlpaca) {
    fake.set_all(
        "focuser/0",
        json!({"connected": true, "name": "Fake Focuser", "driverinfo": "fake",
            "absolute": true, "position": 12000, "maxstep": 50000, "maxincrement": 50000,
            "stepsize": 1.5, "ismoving": false, "temperature": 4.5, "tempcompavailable": true}),
    );
}

fn fast() -> WaitOptions {
    WaitOptions {
        poll_interval: Duration::from_millis(10),
        ..WaitOptions::default()
    }
}

#[tokio::test]
async fn an_absolute_focuser_reports_its_travel_and_moves_to_a_position() {
    let fake = FakeAlpaca::start().await;
    absolute_focuser(&fake);
    let focuser = focuser(&fake).await;

    let caps = focuser.capabilities();
    assert!(caps.absolute && caps.has_temperature && caps.can_temp_compensate);
    assert_eq!(caps.max_position, Some(50000));
    assert_eq!(caps.step_size_um, Some(1.5));
    assert_eq!(focuser.info().name, "Fake Focuser");

    let position = focuser.focuser_position().await.unwrap();
    assert_eq!(position.position, 12000);
    assert_eq!(position.max_position, Some(50000));
    assert_eq!(position.temperature, Some(4.5));
    assert!(!position.is_moving);

    focuser.move_to(20000).await.unwrap();
    assert_eq!(fake.puts("focuser/0/move")[0]["Position"], "20000");

    // A relative move on an absolute focuser is a move to position + offset.
    focuser.move_relative(-500).await.unwrap();
    assert_eq!(fake.puts("focuser/0/move")[1]["Position"], "11500");

    focuser.halt().await.unwrap();
    assert!(fake.put_keys().contains(&"focuser/0/halt".to_string()));
}

#[tokio::test]
async fn moves_beyond_the_travel_are_refused_before_they_are_sent() {
    let fake = FakeAlpaca::start().await;
    absolute_focuser(&fake);
    let focuser = focuser(&fake).await;

    for position in [-1, 50001] {
        let err = focuser.move_to(position).await.unwrap_err();
        assert!(
            matches!(err, ScopinatorError::OutOfRange(_)),
            "{position}: {err:?}"
        );
    }
    // 12000 + 40000 is past the end.
    assert!(matches!(
        focuser.move_relative(40000).await,
        Err(ScopinatorError::OutOfRange(_))
    ));
    assert!(fake.puts("focuser/0/move").is_empty());
    // The ends themselves are fine.
    focuser.move_to(0).await.unwrap();
    focuser.move_to(50000).await.unwrap();
}

#[tokio::test]
async fn a_relative_focuser_moves_by_steps_and_has_no_position() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(
        "focuser/0",
        json!({"connected": true, "name": "Relative", "absolute": false,
            "maxstep": 10000, "maxincrement": 1000, "ismoving": false}),
    );
    for property in ["driverinfo", "stepsize", "temperature", "tempcompavailable"] {
        fake.not_implemented(&format!("focuser/0/{property}"));
    }
    // Alpaca makes reading the position of a relative focuser an error.
    fake.not_implemented("focuser/0/position");
    let focuser = focuser(&fake).await;

    let caps = focuser.capabilities();
    assert!(!caps.absolute && !caps.has_temperature && !caps.can_temp_compensate);
    assert_eq!(caps.max_increment, Some(1000));

    assert!(focuser.move_to(500).await.unwrap_err().is_not_supported());
    focuser.move_relative(-250).await.unwrap();
    assert_eq!(fake.puts("focuser/0/move")[0]["Position"], "-250");
    assert!(matches!(
        focuser.move_relative(1001).await,
        Err(ScopinatorError::OutOfRange(_))
    ));
    assert_eq!(fake.puts("focuser/0/move").len(), 1);

    let position = focuser.focuser_position().await.unwrap();
    assert_eq!(position.temperature, None);
    assert!(!position.is_moving);
}

#[tokio::test]
async fn waiting_for_a_focuser_ends_when_it_stops() {
    let fake = FakeAlpaca::start().await;
    absolute_focuser(&fake);
    fake.on_put("focuser/0/move", |values, _| {
        values.insert("focuser/0/ismoving".into(), json!(true));
    });
    let focuser = Arc::new(focuser(&fake).await);

    focuser.move_to(30000).await.unwrap();
    assert!(focuser.is_moving().await.unwrap());
    let waiting = tokio::spawn({
        let focuser = Arc::clone(&focuser);
        async move { wait_for_focuser(focuser.as_ref(), fast()).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!waiting.is_finished());
    fake.set("focuser/0/ismoving", json!(false));
    waiting.await.unwrap().unwrap();
}

async fn wheel(fake: &FakeAlpaca) -> AlpacaFilterWheelDevice {
    fake.set_all(
        "filterwheel/0",
        json!({"connected": true, "name": "Fake Wheel", "driverinfo": "fake",
            "names": ["L", "R", "G", "B"], "focusoffsets": [0, 12, -5, 30], "position": 0}),
    );
    let (client, id) = client(fake);
    let wheel = AlpacaFilterWheelDevice::new(AlpacaFilterWheel::new(client, 0), &id);
    wheel.connect().await.expect("connect");
    wheel
}

#[tokio::test]
async fn a_filter_wheel_names_its_filters_and_counts_from_zero() {
    let fake = FakeAlpaca::start().await;
    let wheel = wheel(&fake).await;

    let caps = wheel.capabilities();
    assert_eq!(caps.slot_count, 4);
    assert!(caps.has_focus_offsets);
    assert_eq!(
        wheel.get_filter_names().await.unwrap(),
        ["L", "R", "G", "B"]
    );
    assert_eq!(wheel.get_focus_offsets().await.unwrap(), [0, 12, -5, 30]);

    let first = wheel.filter_position().await.unwrap().unwrap();
    assert_eq!((first.position, first.name.as_deref()), (0, Some("L")));

    wheel.set_filter_position(3).await.unwrap();
    assert_eq!(fake.puts("filterwheel/0/position")[0]["Position"], "3");
    fake.set("filterwheel/0/position", json!(3));
    let last = wheel.filter_position().await.unwrap().unwrap();
    assert_eq!((last.position, last.name.as_deref()), (3, Some("B")));

    let err = wheel.set_filter_position(4).await.unwrap_err();
    assert!(matches!(err, ScopinatorError::OutOfRange(_)), "{err:?}");
    assert_eq!(fake.puts("filterwheel/0/position").len(), 1);
}

#[tokio::test]
async fn a_turning_wheel_has_no_position_and_the_wait_ends_when_it_arrives() {
    let fake = FakeAlpaca::start().await;
    let wheel = Arc::new(wheel(&fake).await);
    fake.on_put("filterwheel/0/position", |values, _| {
        values.insert("filterwheel/0/position".into(), json!(-1));
    });

    wheel.set_filter_position(2).await.unwrap();
    assert_eq!(wheel.filter_position().await.unwrap(), None);
    assert!(wheel.is_moving().await.unwrap());

    let waiting = tokio::spawn({
        let wheel = Arc::clone(&wheel);
        async move { wait_for_filter_wheel(wheel.as_ref(), fast()).await }
    });
    tokio::time::sleep(Duration::from_millis(60)).await;
    assert!(!waiting.is_finished());
    fake.set("filterwheel/0/position", json!(2));
    waiting.await.unwrap().unwrap();
    assert_eq!(wheel.filter_position().await.unwrap().unwrap().position, 2);
}

#[tokio::test]
async fn a_wheel_without_focus_offsets_says_so() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(
        "filterwheel/0",
        json!({"connected": true, "name": "Plain", "driverinfo": "fake",
            "names": ["A", "B"], "position": 1}),
    );
    fake.not_implemented("filterwheel/0/focusoffsets");
    let (client, id) = client(&fake);
    let wheel = AlpacaFilterWheelDevice::new(AlpacaFilterWheel::new(client, 0), &id);
    wheel.connect().await.unwrap();

    assert!(!wheel.capabilities().has_focus_offsets);
    assert!(
        wheel
            .get_focus_offsets()
            .await
            .unwrap_err()
            .is_not_supported()
    );
    assert_eq!(
        wheel.info().id.as_str(),
        format!("alpaca:127.0.0.1:{}:filterwheel:0", fake.addr.port())
    );
}
