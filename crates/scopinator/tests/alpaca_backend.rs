//! A whole Alpaca server as a neutral `Backend`: which devices it hands
//! out, and the events its poller makes from their state.
#![cfg(feature = "alpaca")]

mod alpaca_common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use alpaca_common::{FakeAlpaca, full_telescope};
use scopinator::backend::alpaca::{AlpacaBackend, AlpacaConfig};
use scopinator::discovery::Discovered;
use scopinator::{
    Backend, DeviceManager, EventPayload, EventType, ScopinatorError, UnifiedEvent, UnifiedEventBus,
};
use scopinator_types::Coordinates;
use serde_json::{Value, json};
use tokio::sync::broadcast::Receiver;

fn camera(name: &str) -> Value {
    json!({
        "connected": true, "name": name, "driverinfo": "fake", "cameraxsize": 100,
        "cameraysize": 80, "pixelsizex": 3.0, "pixelsizey": 3.0, "sensortype": 0,
        "maxadu": 65535, "gainmin": 0, "gainmax": 100, "offsetmin": 0, "offsetmax": 10,
        "exposuremin": 0.001, "exposuremax": 100.0, "maxbinx": 2, "maxbiny": 2,
        "cansetccdtemperature": false, "fullwellcapacity": 1000.0, "electronsperadu": 1.0,
        "canabortexposure": true, "imageready": false, "camerastate": 0,
    })
}

/// A server with a mount, two cameras, a focuser, a filter wheel and a dome
/// (which this library does not drive).
async fn observatory() -> FakeAlpaca {
    let fake = FakeAlpaca::start().await;
    fake.set_devices(json!([
        {"DeviceName": "Guide Cam", "DeviceType": "Camera", "DeviceNumber": 1, "UniqueID": "c1"},
        {"DeviceName": "Mount", "DeviceType": "Telescope", "DeviceNumber": 0, "UniqueID": "t0"},
        {"DeviceName": "Main Cam", "DeviceType": "Camera", "DeviceNumber": 0, "UniqueID": "c0"},
        {"DeviceName": "Focuser", "DeviceType": "Focuser", "DeviceNumber": 0, "UniqueID": "f0"},
        {"DeviceName": "Wheel", "DeviceType": "FilterWheel", "DeviceNumber": 0, "UniqueID": "w0"},
        {"DeviceName": "Dome", "DeviceType": "Dome", "DeviceNumber": 0, "UniqueID": "d0"},
    ]));
    fake.set_all("telescope/0", full_telescope());
    fake.set_all("camera/0", camera("Main Cam"));
    fake.set_all("camera/1", camera("Guide Cam"));
    for number in 0..2 {
        fake.not_implemented(&format!("camera/{number}/cooleron"));
        fake.not_implemented(&format!("camera/{number}/coolerpower"));
        fake.not_implemented(&format!("camera/{number}/ccdtemperature"));
    }
    fake.set_all(
        "focuser/0",
        json!({"connected": true, "name": "Focuser", "driverinfo": "fake", "absolute": true,
            "position": 5000, "maxstep": 10000, "maxincrement": 10000, "stepsize": 1.0,
            "ismoving": false, "temperature": 5.0, "tempcompavailable": false}),
    );
    fake.set_all(
        "filterwheel/0",
        json!({"connected": true, "name": "Wheel", "driverinfo": "fake",
            "names": ["L", "R", "G", "B"], "focusoffsets": [0, 0, 0, 0], "position": 0}),
    );
    fake
}

fn fast() -> AlpacaConfig {
    AlpacaConfig {
        poll_interval: Duration::from_millis(20),
        ..AlpacaConfig::default()
    }
}

async fn connect(
    fake: &FakeAlpaca,
    config: AlpacaConfig,
) -> (AlpacaBackend, Receiver<UnifiedEvent>) {
    let bus = Arc::new(UnifiedEventBus::new(64));
    let events = bus.subscribe();
    let backend = AlpacaBackend::connect(Ipv4Addr::LOCALHOST, fake.addr.port(), config, bus)
        .await
        .expect("connect");
    (backend, events)
}

/// The next event of one of `wanted` types, skipping others, within two
/// seconds.
async fn next(events: &mut Receiver<UnifiedEvent>, wanted: &[EventType]) -> UnifiedEvent {
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let event = events.recv().await.expect("event bus");
            if wanted.contains(&event.event_type) {
                return event;
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("no {wanted:?} event within two seconds"))
}

/// Every event that arrives in the next `millis` milliseconds.
async fn drain(events: &mut Receiver<UnifiedEvent>, millis: u64) -> Vec<EventType> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(millis);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        seen.push(event.event_type);
    }
    seen
}

#[tokio::test]
async fn a_server_hands_out_its_mount_cameras_focuser_and_filter_wheel() {
    let fake = observatory().await;
    let (backend, _events) = connect(&fake, fast()).await;
    let port = fake.addr.port();

    assert_eq!(backend.kind(), "alpaca");
    assert_eq!(backend.id().as_str(), format!("alpaca:127.0.0.1:{port}"));
    assert_eq!(
        backend.mount().expect("mount").id().as_str(),
        format!("alpaca:127.0.0.1:{port}:telescope:0")
    );
    // Cameras in device-number order, whatever order the server lists them.
    let names: Vec<String> = backend
        .cameras()
        .await
        .unwrap()
        .iter()
        .map(|camera| camera.info().name)
        .collect();
    assert_eq!(names, ["Main Cam", "Guide Cam"]);
    assert_eq!(backend.focuser().expect("focuser").info().name, "Focuser");
    assert_eq!(backend.filter_wheel().expect("wheel").info().name, "Wheel");

    // Every device was connected; the dome was not touched.
    for device in [
        "telescope/0",
        "camera/0",
        "camera/1",
        "focuser/0",
        "filterwheel/0",
    ] {
        assert_eq!(
            fake.puts(&format!("{device}/connected"))[0]["Connected"],
            "True",
            "{device}"
        );
    }
    assert!(fake.puts("dome/0/connected").is_empty());
}

#[tokio::test]
async fn the_configuration_chooses_among_several_devices() {
    let fake = observatory().await;
    let config = AlpacaConfig {
        cameras: Some(vec![1]),
        ..fast()
    };
    let (backend, _events) = connect(&fake, config).await;
    let cameras = backend.cameras().await.unwrap();
    assert_eq!(cameras.len(), 1);
    assert_eq!(cameras[0].info().name, "Guide Cam");
    assert!(fake.puts("camera/0/connected").is_empty());

    // Asking for a device the server does not have is an error.
    let bus = Arc::new(UnifiedEventBus::new(8));
    let missing = AlpacaConfig {
        telescope: Some(3),
        ..fast()
    };
    let err = AlpacaBackend::connect(Ipv4Addr::LOCALHOST, fake.addr.port(), missing, bus)
        .await
        .err()
        .expect("no telescope 3");
    assert!(
        matches!(err, ScopinatorError::InvalidArgument(_)),
        "{err:?}"
    );
}

#[tokio::test]
async fn a_server_with_only_a_camera_has_no_mount() {
    let fake = FakeAlpaca::start().await;
    fake.set_devices(json!([
        {"DeviceName": "Cam", "DeviceType": "Camera", "DeviceNumber": 0, "UniqueID": "c0"},
    ]));
    fake.set_all("camera/0", camera("Cam"));
    for property in ["cooleron", "coolerpower", "ccdtemperature"] {
        fake.not_implemented(&format!("camera/0/{property}"));
    }
    let (backend, _events) = connect(&fake, fast()).await;
    assert!(backend.mount().is_none());
    assert!(backend.focuser().is_none() && backend.filter_wheel().is_none());
    assert_eq!(backend.cameras().await.unwrap().len(), 1);
}

#[tokio::test]
async fn a_slew_is_one_start_and_one_completion_on_the_bus() {
    let fake = observatory().await;
    fake.on_put("telescope/0/slewtocoordinatesasync", |values, _| {
        values.insert("telescope/0/slewing".into(), json!(true));
    });
    let (backend, mut events) = connect(&fake, fast()).await;
    let mount = backend.mount().unwrap();

    // The first poll says the instrument is there, and nothing else.
    let first = next(&mut events, &[EventType::Connected]).await;
    assert_eq!(first.device_id, backend.id());
    assert_eq!(first.source, None);
    assert!(
        drain(&mut events, 100).await.is_empty(),
        "nothing changed yet"
    );

    mount
        .slew_to_coordinates(&Coordinates::from_hours(7.0, 30.0).unwrap())
        .await
        .unwrap();
    let started = next(
        &mut events,
        &[EventType::SlewStarted, EventType::SlewCompleted],
    )
    .await;
    assert_eq!(started.event_type, EventType::SlewStarted);
    assert_eq!(started.source, Some(mount.id()));
    assert_eq!(started.device_id, backend.id());

    fake.set("telescope/0/rightascension", json!(7.0));
    fake.set("telescope/0/declination", json!(30.0));
    fake.set("telescope/0/slewing", json!(false));
    let completed = next(
        &mut events,
        &[EventType::SlewStarted, EventType::SlewCompleted],
    )
    .await;
    assert_eq!(completed.event_type, EventType::SlewCompleted);
    match completed.payload {
        EventPayload::Coordinates(position) => assert_eq!(position.ra.as_hours(), 7.0),
        other => panic!("got {other:?}"),
    }
    // Exactly one of each.
    let rest = drain(&mut events, 150).await;
    assert!(
        !rest.contains(&EventType::SlewStarted) && !rest.contains(&EventType::SlewCompleted),
        "{rest:?}"
    );
}

#[tokio::test]
async fn camera_focuser_and_filter_changes_reach_the_bus_with_their_source() {
    let fake = observatory().await;
    let (backend, mut events) = connect(&fake, fast()).await;
    next(&mut events, &[EventType::Connected]).await;
    let cameras = backend.cameras().await.unwrap();

    // The guide camera finishes an exposure.
    fake.set("camera/1/imageready", json!(true));
    let ready = next(&mut events, &[EventType::ImageReady]).await;
    assert_eq!(ready.source, Some(cameras[1].id()));

    // The focuser moves and stops. The position is in place before the
    // focuser stops: a poll reads position and motion in two requests, and
    // one landing between two changes made here would pair the old position
    // with "stopped".
    fake.set("focuser/0/ismoving", json!(true));
    fake.set("focuser/0/position", json!(6000));
    tokio::time::sleep(Duration::from_millis(80)).await;
    fake.set("focuser/0/ismoving", json!(false));
    let moved = next(&mut events, &[EventType::FocuserMoveCompleted]).await;
    assert!(matches!(moved.payload, EventPayload::FocuserPosition(6000)));
    assert_eq!(moved.source, Some(backend.focuser().unwrap().id()));

    // The wheel turns to the third filter.
    fake.set("filterwheel/0/position", json!(-1));
    tokio::time::sleep(Duration::from_millis(80)).await;
    fake.set("filterwheel/0/position", json!(2));
    let changed = next(&mut events, &[EventType::FilterChanged]).await;
    match changed.payload {
        EventPayload::FilterPosition { position, name } => {
            assert_eq!((position, name.as_deref()), (2, Some("G")));
        }
        other => panic!("got {other:?}"),
    }
}

#[tokio::test]
async fn an_outage_is_one_error_then_disconnected_and_recovery_is_connected() {
    let fake = observatory().await;
    let (_backend, mut events) = connect(&fake, fast()).await;
    next(&mut events, &[EventType::Connected]).await;

    fake.set_broken(true);
    let error = next(&mut events, &[EventType::Error, EventType::Disconnected]).await;
    assert_eq!(error.event_type, EventType::Error);
    let gone = next(&mut events, &[EventType::Disconnected]).await;
    assert_eq!(gone.source, None);
    // One error for the outage, not one per failed poll.
    assert!(!drain(&mut events, 300).await.contains(&EventType::Error));

    fake.set_broken(false);
    next(&mut events, &[EventType::Connected]).await;
}

#[tokio::test]
async fn disconnect_clears_connected_on_every_device_and_stops_polling() {
    let fake = observatory().await;
    let (backend, mut events) = connect(&fake, fast()).await;
    next(&mut events, &[EventType::Connected]).await;

    backend.disconnect().await.unwrap();
    for device in [
        "telescope/0",
        "camera/0",
        "camera/1",
        "focuser/0",
        "filterwheel/0",
    ] {
        let puts = fake.puts(&format!("{device}/connected"));
        assert_eq!(puts.last().unwrap()["Connected"], "False", "{device}");
    }
    // A request already on its way may still land; after that, silence.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = fake.request_count();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.request_count(), before, "the poller is still asking");
}

#[tokio::test]
async fn dropping_the_backend_stops_polling() {
    let fake = observatory().await;
    let (backend, mut events) = connect(&fake, fast()).await;
    next(&mut events, &[EventType::Connected]).await;
    drop(backend);
    tokio::time::sleep(Duration::from_millis(100)).await;
    let before = fake.request_count();
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(fake.request_count(), before);
}

#[tokio::test]
async fn the_device_manager_adds_and_connects_to_an_alpaca_server() {
    let fake = observatory().await;
    let manager = DeviceManager::new();
    let id = manager
        .add_alpaca(Ipv4Addr::LOCALHOST, fake.addr.port())
        .await
        .unwrap();
    assert_eq!(
        id.as_str(),
        format!("alpaca:127.0.0.1:{}", fake.addr.port())
    );
    let backend = manager.get(&id).unwrap();
    assert_eq!(backend.kind(), "alpaca");
    assert!(backend.mount().unwrap().is_tracking().await.unwrap());
    // What only an Alpaca backend has is one downcast away.
    assert!(backend.as_any().downcast_ref::<AlpacaBackend>().is_some());

    // The same server twice is refused.
    let again = manager
        .add_alpaca(Ipv4Addr::LOCALHOST, fake.addr.port())
        .await;
    assert!(matches!(again, Err(ScopinatorError::InvalidState(_))));
    manager.disconnect_all().await.unwrap();

    // And a discovery result of kind "alpaca" connects the same way.
    let found = Discovered::new("alpaca", fake.addr);
    let id = manager.connect(&found).await.unwrap();
    assert_eq!(manager.list_backends(), [id]);
    manager.disconnect_all().await.unwrap();
}
