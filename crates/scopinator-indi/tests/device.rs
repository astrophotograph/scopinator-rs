//! A device handle against a scripted server.

mod common;

use std::sync::Arc;
use std::time::Duration;

use common::{FakeIndi, member};
use scopinator_indi::protocol::property::{PropertyState, SwitchState, interface};
use scopinator_indi::standard::{general, telescope};
use scopinator_indi::{IndiClient, IndiClientConfig, IndiDevice, IndiError};

fn connection(device: &str) -> String {
    format!(
        r#"<defSwitchVector device="{device}" name="CONNECTION" state="Idle" perm="rw" rule="OneOfMany"><defSwitch name="CONNECT">Off</defSwitch><defSwitch name="DISCONNECT">On</defSwitch></defSwitchVector>"#
    )
}

fn driver(device: &str, interfaces: u32) -> String {
    format!(
        r#"<defTextVector device="{device}" name="DRIVER_INFO" state="Idle" perm="ro"><defText name="DRIVER_NAME">{device} driver</defText><defText name="DRIVER_VERSION">1.2</defText><defText name="DRIVER_INTERFACE">{interfaces}</defText></defTextVector>"#
    )
}

const COORDINATES: &str = r#"<defNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Ok" perm="rw"><defNumber name="RA" format="%g" min="0" max="24" step="0">6.5</defNumber><defNumber name="DEC" format="%g" min="-90" max="90" step="0">-20</defNumber></defNumberVector>"#;

fn connected(device: &str, on: bool, state: &str, message: &str) -> String {
    let (connect, disconnect) = if on { ("On", "Off") } else { ("Off", "On") };
    let message = if message.is_empty() {
        String::new()
    } else {
        format!(r#" message="{message}""#)
    };
    format!(
        r#"<setSwitchVector device="{device}" name="CONNECTION" state="{state}"{message}><oneSwitch name="CONNECT">{connect}</oneSwitch><oneSwitch name="DISCONNECT">{disconnect}</oneSwitch></setSwitchVector>"#
    )
}

/// A telescope with a guider port (interfaces 5) and a camera (2).
async fn observatory() -> (FakeIndi, Arc<IndiClient>) {
    let fake = FakeIndi::start().await;
    fake.define(&connection("Scope"))
        .define(&driver("Scope", 5))
        .define(COORDINATES)
        .define(&connection("Camera"))
        .define(&driver("Camera", 2));
    let client = IndiClient::connect(fake.addr, IndiClientConfig::default())
        .await
        .expect("connect");
    client
        .wait_for("Camera", "DRIVER_INFO", Duration::from_secs(2), |_| true)
        .await
        .expect("definitions");
    (fake, Arc::new(client))
}

#[tokio::test]
async fn devices_are_found_by_what_their_driver_says_they_are() {
    let (_fake, client) = observatory().await;
    assert_eq!(client.devices_with(interface::TELESCOPE), ["Scope"]);
    assert_eq!(client.devices_with(interface::GUIDER), ["Scope"]);
    assert_eq!(client.devices_with(interface::CCD), ["Camera"]);
    assert!(client.devices_with(interface::FOCUSER).is_empty());

    let scope = IndiDevice::new(Arc::clone(&client), "Scope");
    assert_eq!(scope.name(), "Scope");
    assert_eq!(scope.interfaces(), Some(5));
    assert!(scope.has_interface(interface::TELESCOPE));
    assert!(!scope.has_interface(interface::CCD));
    assert_eq!(scope.driver().as_deref(), Some("Scope driver 1.2"));

    // A device the server does not have is nothing in particular.
    let nobody = IndiDevice::new(Arc::clone(&client), "Nobody");
    assert_eq!(nobody.interfaces(), None);
    assert!(!nobody.has_interface(interface::TELESCOPE));
    assert_eq!(nobody.driver(), None);
    client.shutdown().await;
}

#[tokio::test]
async fn values_are_read_and_what_is_not_defined_is_none() {
    let (_fake, client) = observatory().await;
    let scope = IndiDevice::new(Arc::clone(&client), "Scope");
    assert_eq!(
        scope.number(telescope::EQUATORIAL_EOD_COORD, telescope::RA),
        Some(6.5)
    );
    assert_eq!(
        scope.number(telescope::EQUATORIAL_EOD_COORD, telescope::DEC),
        Some(-20.0)
    );
    assert_eq!(
        scope.state(telescope::EQUATORIAL_EOD_COORD),
        Some(PropertyState::Ok)
    );
    assert_eq!(
        scope.switch(general::CONNECTION, general::DISCONNECT),
        Some(SwitchState::On)
    );
    assert_eq!(
        scope
            .text(general::DRIVER_INFO, general::DRIVER_NAME)
            .as_deref(),
        Some("Scope driver")
    );
    assert!(scope.has(telescope::EQUATORIAL_EOD_COORD));
    assert!(!scope.has(telescope::TELESCOPE_PARK));
    assert!(!scope.is_connected());

    assert_eq!(scope.number(telescope::TELESCOPE_PARK, "X"), None);
    assert_eq!(scope.number(telescope::EQUATORIAL_EOD_COORD, "ALT"), None);
    // A member of another kind is not a number.
    assert_eq!(scope.number(general::CONNECTION, general::CONNECT), None);
    assert_eq!(scope.state(telescope::TELESCOPE_PARK), None);
    client.shutdown().await;
}

#[tokio::test]
async fn connecting_waits_for_the_driver_to_say_it_is_connected() {
    let (fake, client) = observatory().await;
    fake.on_new("Scope", "CONNECTION", &connected("Scope", true, "Ok", ""));
    let scope = IndiDevice::new(Arc::clone(&client), "Scope");
    scope
        .connect(Duration::from_secs(2))
        .await
        .expect("connected");
    assert!(scope.is_connected());

    let sent = fake.received_new("Scope", "CONNECTION");
    assert_eq!(sent.len(), 1);
    assert_eq!(
        member(&sent[0], "oneSwitch", "CONNECT").as_deref(),
        Some("On")
    );
    assert_eq!(
        member(&sent[0], "oneSwitch", "DISCONNECT").as_deref(),
        Some("Off")
    );

    // Already connected: nothing more is sent.
    scope.connect(Duration::from_secs(2)).await.unwrap();
    assert_eq!(fake.received_new("Scope", "CONNECTION").len(), 1);

    // And back.
    fake.on_new(
        "Scope",
        "CONNECTION",
        &connected("Scope", false, "Idle", ""),
    );
    scope
        .disconnect(Duration::from_secs(2))
        .await
        .expect("disconnected");
    assert!(!scope.is_connected());
    let sent = fake.received_new("Scope", "CONNECTION");
    assert_eq!(
        member(&sent[1], "oneSwitch", "DISCONNECT").as_deref(),
        Some("On")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn a_driver_that_takes_its_time_is_waited_for() {
    let (fake, client) = observatory().await;
    fake.on_new(
        "Camera",
        "CONNECTION",
        &connected("Camera", false, "Busy", ""),
    );
    let camera = IndiDevice::new(Arc::clone(&client), "Camera");
    let connecting = tokio::spawn({
        let camera = camera.clone();
        async move { camera.connect(Duration::from_secs(3)).await }
    });
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(!connecting.is_finished());
    fake.send(&connected("Camera", true, "Ok", ""));
    connecting.await.unwrap().expect("connected");
    client.shutdown().await;
}

#[tokio::test]
async fn a_silent_driver_times_out() {
    let (_fake, client) = observatory().await;
    let scope = IndiDevice::new(Arc::clone(&client), "Scope");
    let outcome = scope.connect(Duration::from_millis(200)).await;
    assert!(matches!(outcome, Err(IndiError::Timeout)), "{outcome:?}");
    // So does a device that never defines itself.
    let nobody = IndiDevice::new(Arc::clone(&client), "Nobody");
    assert!(matches!(
        nobody.connect(Duration::from_millis(100)).await,
        Err(IndiError::Timeout)
    ));
    client.shutdown().await;
}

#[tokio::test]
async fn a_driver_that_refuses_fails_at_once_with_its_reason() {
    let (fake, client) = observatory().await;
    fake.on_new(
        "Scope",
        "CONNECTION",
        &connected("Scope", false, "Alert", "Failed to open /dev/ttyUSB0"),
    );
    let scope = IndiDevice::new(Arc::clone(&client), "Scope");
    let started = std::time::Instant::now();
    let outcome = scope.connect(Duration::from_secs(10)).await;
    assert!(started.elapsed() < Duration::from_secs(3));
    match outcome {
        Err(IndiError::PropertyState(reason)) => assert_eq!(reason, "Failed to open /dev/ttyUSB0"),
        other => panic!("{other:?}"),
    }

    // The alert it left behind is not taken for the answer to the next
    // attempt.
    fake.on_new("Scope", "CONNECTION", &connected("Scope", true, "Ok", ""));
    scope
        .connect(Duration::from_secs(2))
        .await
        .expect("connected this time");
    client.shutdown().await;
}
