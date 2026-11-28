//! The INDI client against a scripted server.

mod common;

use std::time::Duration;

use common::{FakeIndi, member};
use scopinator_indi::protocol::message::IndiMessage;
use scopinator_indi::protocol::property::{PropertyState, SwitchState};
use scopinator_indi::{IndiClient, IndiClientConfig, IndiError};

const CONNECTION: &str = r#"<defSwitchVector device="Scope" name="CONNECTION" state="Idle" perm="rw" rule="OneOfMany"><defSwitch name="CONNECT">Off</defSwitch><defSwitch name="DISCONNECT">On</defSwitch></defSwitchVector>"#;
const COORDINATES: &str = r#"<defNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Idle" perm="rw"><defNumber name="RA" format="%010.6m" min="0" max="24" step="0">1:30:00</defNumber><defNumber name="DEC" format="%010.6m" min="-90" max="90" step="0">20</defNumber></defNumberVector>"#;
const DRIVER: &str = r#"<defTextVector device="Scope" name="DRIVER_INFO" state="Idle" perm="ro"><defText name="DRIVER_NAME">Telescope Simulator</defText><defText name="DRIVER_INTERFACE">5</defText></defTextVector>"#;
const CONFIG: &str = r#"<defTextVector device="Scope" name="DEVICE_PORT" state="Idle" perm="rw"><defText name="PORT">/dev/ttyUSB0</defText></defTextVector>"#;
const FREE: &str = r#"<defNumberVector device="Scope" name="OFFSET" state="Idle" perm="rw"><defNumber name="VALUE" format="%g" min="0" max="0" step="0">0</defNumber></defNumberVector>"#;

fn quick() -> IndiClientConfig {
    IndiClientConfig::default()
        .with_reconnect_delay(Duration::from_millis(30), Duration::from_millis(100))
}

async fn scope() -> (FakeIndi, IndiClient) {
    let fake = FakeIndi::start().await;
    fake.define(CONNECTION)
        .define(COORDINATES)
        .define(DRIVER)
        .define(CONFIG)
        .define(FREE);
    let client = IndiClient::connect(fake.addr, quick())
        .await
        .expect("connect");
    client
        .wait_for("Scope", "OFFSET", Duration::from_secs(2), |_| true)
        .await
        .expect("definitions");
    (fake, client)
}

async fn eventually(check: impl Fn() -> bool) {
    for _ in 0..150 {
        if check() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("it did not happen within three seconds");
}

#[tokio::test]
async fn connecting_asks_for_properties_and_keeps_what_comes() {
    let (fake, client) = scope().await;
    assert!(client.is_connected());
    assert_eq!(fake.count("getProperties"), 1);
    assert!(fake.received()[0].contains("version=\"1.7\""));
    assert_eq!(client.devices(), ["Scope"]);
    assert_eq!(client.properties("Scope").len(), 5);

    let coordinates = client.property("Scope", "EQUATORIAL_EOD_COORD").unwrap();
    assert_eq!(coordinates.number("RA"), Some(1.5));
    assert_eq!(coordinates.number("DEC"), Some(20.0));
    assert_eq!(coordinates.state, PropertyState::Idle);
    assert!(client.property("Scope", "NOPE").is_none());
    assert!(client.property("Nobody", "CONNECTION").is_none());
    client.shutdown().await;
    assert!(!client.is_connected());
}

#[tokio::test]
async fn an_update_changes_the_stored_value_and_ends_a_wait() {
    let (fake, client) = scope().await;
    let mut messages = client.subscribe();
    let waiting = {
        let wait = client.wait_for(
            "Scope",
            "EQUATORIAL_EOD_COORD",
            Duration::from_secs(2),
            |property| property.state == PropertyState::Ok,
        );
        fake.send(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Busy"><oneNumber name="RA">5.5</oneNumber></setNumberVector>"#);
        fake.send(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Ok"><oneNumber name="RA">6</oneNumber><oneNumber name="DEC">21</oneNumber></setNumberVector>"#);
        wait.await.expect("arrived")
    };
    assert_eq!(waiting.number("RA"), Some(6.0));
    assert_eq!(waiting.number("DEC"), Some(21.0));
    // Both updates were broadcast, in order.
    let mut states = Vec::new();
    while states.len() < 2 {
        if let IndiMessage::Update(update) = messages.recv().await.unwrap() {
            states.push(update.state);
        }
    }
    assert_eq!(states, [Some(PropertyState::Busy), Some(PropertyState::Ok)]);
    client.shutdown().await;
}

#[tokio::test]
async fn a_wait_for_what_never_comes_times_out() {
    let (_fake, client) = scope().await;
    let started = std::time::Instant::now();
    let outcome = client
        .wait_for(
            "Scope",
            "EQUATORIAL_EOD_COORD",
            Duration::from_millis(150),
            |property| property.state == PropertyState::Alert,
        )
        .await;
    assert!(matches!(outcome, Err(IndiError::Timeout)), "{outcome:?}");
    assert!(started.elapsed() >= Duration::from_millis(140));
    // A property that is never defined, the same.
    let outcome = client
        .wait_for("Scope", "NOPE", Duration::from_millis(50), |_| true)
        .await;
    assert!(matches!(outcome, Err(IndiError::Timeout)));
    client.shutdown().await;
}

#[tokio::test]
async fn switches_numbers_and_text_are_sent_as_new_vectors() {
    let (fake, client) = scope().await;
    client
        .set_switches(
            "Scope",
            "CONNECTION",
            &[
                ("CONNECT", SwitchState::On),
                ("DISCONNECT", SwitchState::Off),
            ],
        )
        .await
        .unwrap();
    client
        .set_numbers(
            "Scope",
            "EQUATORIAL_EOD_COORD",
            &[("RA", 12.5), ("DEC", -5.25)],
        )
        .await
        .unwrap();
    client
        .set_text("Scope", "DEVICE_PORT", &[("PORT", "/dev/tty <1> & co")])
        .await
        .unwrap();
    eventually(|| fake.received_new("Scope", "DEVICE_PORT").len() == 1).await;

    let switches = &fake.received_new("Scope", "CONNECTION")[0];
    assert!(switches.starts_with("<newSwitchVector"));
    assert_eq!(
        member(switches, "oneSwitch", "CONNECT").as_deref(),
        Some("On")
    );
    assert_eq!(
        member(switches, "oneSwitch", "DISCONNECT").as_deref(),
        Some("Off")
    );
    let numbers = &fake.received_new("Scope", "EQUATORIAL_EOD_COORD")[0];
    assert_eq!(member(numbers, "oneNumber", "RA").as_deref(), Some("12.5"));
    assert_eq!(
        member(numbers, "oneNumber", "DEC").as_deref(),
        Some("-5.25")
    );
    // Escaped on the way out, the same text on arrival.
    let text = &fake.received_new("Scope", "DEVICE_PORT")[0];
    assert_eq!(
        member(text, "oneText", "PORT").as_deref(),
        Some("/dev/tty <1> & co")
    );
    client.shutdown().await;
}

#[tokio::test]
async fn what_cannot_be_written_is_refused_and_nothing_is_sent() {
    let (fake, client) = scope().await;
    let on = [("CONNECT", SwitchState::On)];

    assert!(matches!(
        client
            .set_text("Scope", "DRIVER_INFO", &[("DRIVER_NAME", "x")])
            .await,
        Err(IndiError::ReadOnly { .. })
    ));
    assert!(matches!(
        client.set_switches("Scope", "NOPE", &on).await,
        Err(IndiError::PropertyNotFound { .. })
    ));
    assert!(matches!(
        client
            .set_switches("Scope", "CONNECTION", &[("SIDEWAYS", SwitchState::On)])
            .await,
        Err(IndiError::MemberNotFound { .. })
    ));
    assert!(matches!(
        client
            .set_numbers("Scope", "CONNECTION", &[("CONNECT", 1.0)])
            .await,
        Err(IndiError::WrongKind { .. })
    ));
    // Outside the member's limits, and not a number at all.
    for value in [24.5, -0.1, f64::NAN, f64::INFINITY] {
        assert!(
            matches!(
                client
                    .set_numbers("Scope", "EQUATORIAL_EOD_COORD", &[("RA", value)])
                    .await,
                Err(IndiError::OutOfRange { .. })
            ),
            "{value}"
        );
    }
    assert!(matches!(
        client
            .set_numbers("Scope", "EQUATORIAL_EOD_COORD", &[("ALT", 1.0)])
            .await,
        Err(IndiError::MemberNotFound { .. })
    ));
    // A member defined with min = max has no limits.
    client
        .set_numbers("Scope", "OFFSET", &[("VALUE", 1.0e6)])
        .await
        .unwrap();
    eventually(|| fake.received_new("Scope", "OFFSET").len() == 1).await;
    assert_eq!(
        fake.received()
            .iter()
            .filter(|e| e.starts_with("<new"))
            .count(),
        1
    );
    client.shutdown().await;
}

#[tokio::test]
async fn deleting_removes_a_property_and_then_the_device() {
    let (fake, client) = scope().await;
    fake.send(r#"<delProperty device="Scope" name="DEVICE_PORT"/>"#);
    eventually(|| client.property("Scope", "DEVICE_PORT").is_none()).await;
    assert_eq!(client.properties("Scope").len(), 4);
    fake.send(r#"<delProperty device="Scope"/>"#);
    eventually(|| client.devices().is_empty()).await;
    client.shutdown().await;
}

#[tokio::test]
async fn messages_split_across_writes_are_read() {
    let (fake, client) = scope().await;
    fake.send_in_chunks(
        r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD" state="Ok"><oneNumber name="RA">7.25</oneNumber></setNumberVector>"#,
        5,
    );
    let property = client
        .wait_for(
            "Scope",
            "EQUATORIAL_EOD_COORD",
            Duration::from_secs(2),
            |property| property.number("RA") == Some(7.25),
        )
        .await
        .unwrap();
    assert_eq!(property.state, PropertyState::Ok);
    client.shutdown().await;
}

#[tokio::test]
async fn an_update_for_an_undefined_property_and_an_unreadable_one_are_passed_over() {
    let (fake, client) = scope().await;
    fake.send(r#"<setNumberVector device="Scope" name="GHOST" state="Ok"><oneNumber name="A">1</oneNumber></setNumberVector>"#);
    fake.send(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD"><oneNumber name="RA">soon</oneNumber></setNumberVector>"#);
    fake.send(r#"<setNumberVector device="Scope" name="EQUATORIAL_EOD_COORD"><oneNumber name="RA">9</oneNumber></setNumberVector>"#);
    client
        .wait_for(
            "Scope",
            "EQUATORIAL_EOD_COORD",
            Duration::from_secs(2),
            |property| property.number("RA") == Some(9.0),
        )
        .await
        .unwrap();
    assert!(client.property("Scope", "GHOST").is_none());
    client.shutdown().await;
}

#[tokio::test]
async fn a_lost_connection_is_made_again_and_the_properties_are_asked_for_again() {
    let (fake, client) = scope().await;
    // The server comes back with a different idea of the mount.
    fake.clear_definitions();
    fake.define(CONNECTION);
    fake.drop_connections();

    eventually(|| fake.connection_count() == 2 && fake.count("getProperties") == 2).await;
    eventually(|| client.is_connected() && client.property("Scope", "CONNECTION").is_some()).await;
    // What the old connection defined is gone.
    assert_eq!(client.properties("Scope").len(), 1);
    assert!(client.property("Scope", "EQUATORIAL_EOD_COORD").is_none());

    // And it works as before.
    client
        .set_switches("Scope", "CONNECTION", &[("CONNECT", SwitchState::On)])
        .await
        .unwrap();
    eventually(|| fake.received_new("Scope", "CONNECTION").len() == 1).await;
    client.shutdown().await;
}

#[tokio::test]
async fn nothing_is_sent_while_the_server_is_away() {
    let (fake, client) = scope().await;
    fake.drop_connections();
    eventually(|| !client.is_connected() || fake.connection_count() == 2).await;
    // Whether it has reconnected yet or not, a request made while it was
    // down is refused rather than kept for later.
    if !client.is_connected() {
        let refused = client
            .set_switches("Scope", "CONNECTION", &[("CONNECT", SwitchState::On)])
            .await;
        assert!(
            matches!(
                refused,
                Err(IndiError::NotConnected | IndiError::PropertyNotFound { .. })
            ),
            "{refused:?}"
        );
    }
    client.shutdown().await;
}

#[tokio::test]
async fn a_server_that_is_not_there_is_an_error() {
    let unused = "127.0.0.1:1".parse().unwrap();
    let outcome = IndiClient::connect(unused, quick()).await;
    assert!(
        matches!(outcome, Err(IndiError::Connection(_) | IndiError::Timeout)),
        "{outcome:?}"
    );
}

#[tokio::test]
async fn shutting_down_closes_the_socket_and_stops_reconnecting() {
    let (fake, client) = scope().await;
    client.shutdown().await;
    assert!(!client.is_connected());
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.connection_count(), 1);
    assert!(client.devices().is_empty());
    assert!(matches!(
        client
            .set_switches("Scope", "CONNECTION", &[("CONNECT", SwitchState::On)])
            .await,
        Err(IndiError::NotConnected)
    ));
}
