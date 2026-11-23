//! The Alpaca backend against the ASCOM Alpaca Omni Simulators.
//!
//! The other Alpaca tests run against a fake server written alongside the
//! client, so they share its misunderstandings. These run against the
//! simulator the ASCOM Initiative publishes, through the neutral traits
//! only.
//!
//! They are ignored unless asked for, and need a simulator in its default
//! state at `ALPACA_SIM_URL`. `tools/alpaca-sim.sh` (or `just
//! alpaca-sim-test`) downloads a pinned release, starts it, runs these and
//! stops it. Tested with OmniSim v0.4.0.
#![cfg(feature = "alpaca")]

use std::net::{Ipv4Addr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use scopinator::backend::alpaca::{AlpacaBackend, AlpacaConfig};
use scopinator::{
    Backend, EventType, UnifiedEvent, UnifiedEventBus, WaitOptions, wait_for_filter_wheel,
    wait_for_focuser, wait_for_image, wait_for_park, wait_for_slew,
};
use scopinator_types::{
    Coordinates, DecDegrees, Epoch, ExposureSettings, MountAxis, PixelFormat, RaDegrees, Roi,
    SlewState,
};
use tokio::sync::broadcast::Receiver;

/// Where the simulator is, from `ALPACA_SIM_URL` (`http://127.0.0.1:32323`).
fn simulator() -> SocketAddrV4 {
    let url = std::env::var("ALPACA_SIM_URL")
        .expect("set ALPACA_SIM_URL to a running simulator, e.g. http://127.0.0.1:32323");
    url.trim_start_matches("http://")
        .trim_end_matches('/')
        .parse()
        .unwrap_or_else(|e| panic!("ALPACA_SIM_URL `{url}` is not http://<ipv4>:<port>: {e}"))
}

async fn connect() -> (AlpacaBackend, Receiver<UnifiedEvent>) {
    let address = simulator();
    let bus = Arc::new(UnifiedEventBus::new(256));
    let events = bus.subscribe();
    let config = AlpacaConfig {
        poll_interval: Duration::from_millis(200),
        ..AlpacaConfig::default()
    };
    let backend = AlpacaBackend::connect(*address.ip(), address.port(), config, bus)
        .await
        .expect("connect to the simulator");
    (backend, events)
}

fn patient() -> WaitOptions {
    WaitOptions {
        poll_interval: Duration::from_millis(200),
        timeout: Duration::from_secs(120),
        settle: Duration::ZERO,
    }
}

/// Angle between two positions in the same system, in arcminutes.
fn separation_arcmin(a: Coordinates, b: Coordinates) -> f64 {
    let (ra1, dec1) = (
        a.ra.as_degrees().to_radians(),
        a.dec.as_degrees().to_radians(),
    );
    let (ra2, dec2) = (
        b.ra.as_degrees().to_radians(),
        b.dec.as_degrees().to_radians(),
    );
    let h = ((dec1 - dec2) / 2.0).sin().powi(2)
        + dec1.cos() * dec2.cos() * ((ra1 - ra2) / 2.0).sin().powi(2);
    (2.0 * h.sqrt().asin()).to_degrees() * 60.0
}

/// A J2000 position two hours east of the meridian at +40 degrees: well
/// above the horizon for the simulator's default site (51 degrees north)
/// whenever this runs.
async fn target_in_the_sky(mount: &dyn scopinator::Mount) -> Coordinates {
    // The mount starts pointing due south; its right ascension there is the
    // local sidereal time.
    let south = mount.get_coordinates().await.expect("position");
    let ra_hours = (south.ra.as_hours() - 2.0).rem_euclid(24.0);
    Coordinates::with_epoch(
        RaDegrees::from_hours(ra_hours).unwrap(),
        DecDegrees::new(40.0).unwrap(),
        south.epoch,
    )
    .to_epoch(Epoch::J2000, chrono::Utc::now())
}

/// Every event already on the bus, plus those of the next half second.
async fn collect(events: &mut Receiver<UnifiedEvent>) -> Vec<UnifiedEvent> {
    let mut seen = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while let Ok(Ok(event)) = tokio::time::timeout_at(deadline, events.recv()).await {
        seen.push(event);
    }
    seen
}

#[tokio::test]
#[ignore = "needs a simulator at ALPACA_SIM_URL; run tools/alpaca-sim.sh"]
async fn the_simulator_hands_out_a_mount_camera_focuser_and_filter_wheel() {
    let (backend, _events) = connect().await;

    let mount = backend.mount().expect("the simulator has a telescope");
    assert!(mount.is_connected());
    let caps = mount.capabilities();
    assert!(caps.can_slew && caps.can_sync && caps.can_park && caps.can_unpark);
    assert!(caps.can_move_axis);
    println!("mount: {:?}\n  {caps:?}", mount.info());

    let cameras = backend.cameras().await.unwrap();
    assert_eq!(cameras.len(), 1);
    let camera_caps = cameras[0].capabilities();
    assert!(camera_caps.max_width.is_some() && camera_caps.max_height.is_some());
    println!("camera: {:?}\n  {camera_caps:?}", cameras[0].info());

    let focuser = backend.focuser().expect("the simulator has a focuser");
    println!(
        "focuser: {:?}\n  {:?}",
        focuser.info(),
        focuser.capabilities()
    );
    let wheel = backend
        .filter_wheel()
        .expect("the simulator has a filter wheel");
    assert!(wheel.capabilities().slot_count > 0);
    println!("wheel: {:?}\n  {:?}", wheel.info(), wheel.capabilities());

    backend.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "needs a simulator at ALPACA_SIM_URL; run tools/alpaca-sim.sh"]
async fn the_mount_slews_syncs_moves_and_parks() {
    let (backend, mut events) = connect().await;
    let mount = backend.mount().unwrap();

    // Leave nothing moving from an earlier run that was cut short: an axis
    // left in motion keeps the mount "slewing" for ever.
    mount.stop_motion().await.unwrap();
    let _ = mount.abort_slew().await;
    wait_for_slew(mount.as_ref(), patient()).await.unwrap();

    if mount.is_parked().await.unwrap() {
        mount.unpark().await.unwrap();
    }
    assert!(!mount.is_parked().await.unwrap());
    mount.set_tracking(true).await.unwrap();
    assert!(mount.is_tracking().await.unwrap());

    // A slew to a J2000 position: accepted at once, then waited for.
    let target = target_in_the_sky(mount.as_ref()).await;
    collect(&mut events).await;
    mount.slew_to_coordinates(&target).await.unwrap();
    assert!(mount.is_slewing().await.unwrap(), "returned before arrival");
    wait_for_slew(mount.as_ref(), patient()).await.unwrap();

    let arrived = mount
        .get_coordinates()
        .await
        .unwrap()
        .to_epoch(Epoch::J2000, chrono::Utc::now());
    let off = separation_arcmin(target, arrived);
    assert!(off < 1.0, "arrived {off:.2} arcmin from the target");
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Tracking
    );

    // The poller saw it: one start and one completion, from the mount.
    let seen = collect(&mut events).await;
    let count = |wanted: EventType| seen.iter().filter(|e| e.event_type == wanted).count();
    assert_eq!(count(EventType::SlewStarted), 1, "{seen:?}");
    assert_eq!(count(EventType::SlewCompleted), 1, "{seen:?}");
    assert!(
        seen.iter()
            .filter(|e| e.event_type == EventType::SlewCompleted)
            .all(|e| e.source == Some(mount.id()))
    );

    // Sync: tell the mount it is a quarter of a degree north of where it
    // thinks it is, and it should then report that.
    let nudged = Coordinates::with_epoch(
        arrived.ra,
        DecDegrees::new(arrived.dec.as_degrees() + 0.25).unwrap(),
        Epoch::J2000,
    );
    mount.sync_to_coordinates(&nudged).await.unwrap();
    let synced = mount
        .get_coordinates()
        .await
        .unwrap()
        .to_epoch(Epoch::J2000, chrono::Utc::now());
    let off = separation_arcmin(nudged, synced);
    assert!(
        off < 1.0,
        "after sync the mount is {off:.2} arcmin from where it was told"
    );

    // Manual motion: every speed fraction must be one the mount accepts,
    // and moving the declination axis must change the declination.
    let before = mount.get_coordinates().await.unwrap();
    for speed in [0.1, 0.4, 0.75, 1.0, -0.4] {
        mount
            .move_axis(MountAxis::Secondary, speed)
            .await
            .unwrap_or_else(|e| panic!("speed {speed}: {e}"));
    }
    mount.move_axis(MountAxis::Secondary, 0.25).await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;
    mount.stop_motion().await.unwrap();
    wait_for_slew(mount.as_ref(), patient()).await.unwrap();
    let after = mount.get_coordinates().await.unwrap();
    assert!(
        (after.dec.as_degrees() - before.dec.as_degrees()).abs() > 0.05,
        "declination did not change: {} -> {}",
        before.dec.as_degrees(),
        after.dec.as_degrees()
    );

    // Site and clock.
    let site = mount.get_site().await.unwrap();
    println!("site: {site:?}");
    let clock = mount.get_utc().await.unwrap();
    let skew = (chrono::Utc::now() - clock).num_seconds().abs();
    assert!(
        skew < 120,
        "the simulator's clock is {skew} s from this computer's"
    );

    // Park, and wait for it.
    mount.park().await.unwrap();
    wait_for_park(mount.as_ref(), patient()).await.unwrap();
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Parked
    );
    // A parked mount refuses to slew.
    let err = mount.slew_to_coordinates(&target).await.unwrap_err();
    assert!(
        matches!(err, scopinator::ScopinatorError::Parked),
        "{err:?}"
    );
    assert!(
        collect(&mut events)
            .await
            .iter()
            .any(|e| e.event_type == EventType::Parked)
    );

    mount.unpark().await.unwrap();
    backend.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "needs a simulator at ALPACA_SIM_URL; run tools/alpaca-sim.sh"]
async fn the_camera_takes_a_binned_subframe() {
    let (backend, mut events) = connect().await;
    let camera = backend.cameras().await.unwrap().remove(0);
    let caps = camera.capabilities();
    let (sensor_width, sensor_height) = (caps.max_width.unwrap(), caps.max_height.unwrap());
    assert!(sensor_width >= 200 && sensor_height >= 200);

    // A 100x80 region of the sensor, binned 2x2: 50x40 pixels.
    let bin = if caps.max_bin.is_some_and(|max| max >= 2) {
        2
    } else {
        1
    };
    let settings = ExposureSettings::new(1.0)
        .with_binning(bin)
        .with_roi(Roi::new(40, 20, 100, 80).unwrap());
    collect(&mut events).await;
    camera.start_exposure(&settings).await.unwrap();
    wait_for_image(camera.as_ref(), patient()).await.unwrap();
    let image = camera.get_image().await.unwrap();

    assert_eq!((image.width, image.height), (100 / bin, 80 / bin));
    assert!(image.has_expected_len(), "{} bytes", image.data.len());
    assert!(
        matches!(
            image.format,
            PixelFormat::Mono16 | PixelFormat::Bayer16(_) | PixelFormat::Rgb16
        ),
        "{:?}",
        image.format
    );
    assert_eq!(image.metadata.exposure_seconds, Some(1.0));
    assert_eq!((image.metadata.bin_x, image.metadata.bin_y), (bin, bin));
    println!(
        "image: {}x{} {:?}, sensor {sensor_width}x{sensor_height} {:?}",
        image.width, image.height, image.format, caps.sensor_type
    );

    let seen = collect(&mut events).await;
    assert!(
        seen.iter()
            .any(|e| e.event_type == EventType::ImageReady && e.source == Some(camera.id())),
        "{seen:?}"
    );

    // And the whole sensor, unbinned.
    camera
        .start_exposure(&ExposureSettings::new(0.5))
        .await
        .unwrap();
    wait_for_image(camera.as_ref(), patient()).await.unwrap();
    let full = camera.get_image().await.unwrap();
    assert_eq!((full.width, full.height), (sensor_width, sensor_height));
    assert!(full.has_expected_len());

    backend.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "needs a simulator at ALPACA_SIM_URL; run tools/alpaca-sim.sh"]
async fn the_focuser_and_filter_wheel_move_and_report_it() {
    let (backend, mut events) = connect().await;

    let focuser = backend.focuser().unwrap();
    let caps = focuser.capabilities();
    let start = focuser.focuser_position().await.unwrap();
    println!("focuser at {start:?}");
    collect(&mut events).await;
    if caps.absolute {
        let max = caps.max_position.expect("an absolute focuser has a travel");
        let target = if start.position + 500 <= max {
            start.position + 500
        } else {
            start.position - 500
        };
        focuser.move_to(target).await.unwrap();
        wait_for_focuser(focuser.as_ref(), patient()).await.unwrap();
        assert_eq!(focuser.focuser_position().await.unwrap().position, target);
        // Beyond the travel is refused.
        assert!(focuser.move_to(max + 1).await.is_err());
    } else {
        focuser.move_relative(100).await.unwrap();
        wait_for_focuser(focuser.as_ref(), patient()).await.unwrap();
    }
    assert!(
        collect(&mut events)
            .await
            .iter()
            .any(|e| e.event_type == EventType::FocuserMoveCompleted)
    );

    let wheel = backend.filter_wheel().unwrap();
    let names = wheel.get_filter_names().await.unwrap();
    assert_eq!(names.len() as u32, wheel.capabilities().slot_count);
    wait_for_filter_wheel(wheel.as_ref(), patient())
        .await
        .unwrap();
    let current = wheel.filter_position().await.unwrap().expect("at rest");
    let next = (current.position + 1) % names.len() as u32;
    wheel.set_filter_position(next).await.unwrap();
    wait_for_filter_wheel(wheel.as_ref(), patient())
        .await
        .unwrap();
    let arrived = wheel.filter_position().await.unwrap().expect("at rest");
    assert_eq!(arrived.position, next);
    assert_eq!(arrived.name.as_deref(), Some(names[next as usize].as_str()));
    assert!(wheel.set_filter_position(names.len() as u32).await.is_err());
    assert!(
        collect(&mut events)
            .await
            .iter()
            .any(|e| e.event_type == EventType::FilterChanged)
    );

    backend.disconnect().await.unwrap();
}

#[tokio::test]
#[ignore = "needs a simulator at ALPACA_SIM_URL; run tools/alpaca-sim.sh"]
async fn connecting_by_address_through_the_device_manager_works() {
    let address = simulator();
    let manager = scopinator::DeviceManager::new();
    let id = manager
        .add_alpaca(*address.ip(), address.port())
        .await
        .unwrap();
    let backend = manager.get(&id).unwrap();
    assert_eq!(backend.kind(), "alpaca");
    assert!(backend.mount().is_some());
    // Reads of things the simulator may or may not implement must come back
    // as a value or as "not supported", never as a transport or parse error.
    let mount = backend.mount().unwrap();
    for (what, result) in [
        ("alt-az", mount.get_alt_az().await.map(|_| ())),
        ("side of pier", mount.side_of_pier().await.map(|_| ())),
        ("alignment", mount.alignment_mode().await.map(|_| ())),
        ("home", mount.is_at_home().await.map(|_| ())),
    ] {
        if let Err(e) = result {
            assert!(e.is_not_supported(), "{what}: {e:?}");
        }
    }
    manager.disconnect_all().await.unwrap();
    assert_eq!(
        Ipv4Addr::LOCALHOST,
        *address.ip(),
        "these tests expect a local simulator"
    );
}
