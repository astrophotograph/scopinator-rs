//! An Alpaca telescope through the neutral `Mount` trait.
#![cfg(feature = "alpaca")]

mod alpaca_common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use alpaca_common::{FakeAlpaca, full_telescope};
use scopinator::backend::alpaca::AlpacaMount;
use scopinator::{Device, Mount, ScopinatorError, WaitOptions, wait_for_park, wait_for_slew};
use scopinator_alpaca::{AlpacaClient, AlpacaTelescope};
use scopinator_types::{
    AlignmentMode, Coordinates, DecDegrees, DeviceId, DeviceKind, Epoch, GuideDirection, MountAxis,
    PierSide, RaDegrees, SiteLocation, SlewState, TrackingRate,
};
use serde_json::json;

const SCOPE: &str = "telescope/0";

fn key(property: &str) -> String {
    format!("{SCOPE}/{property}")
}

async fn mount(fake: &FakeAlpaca) -> AlpacaMount {
    let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, fake.addr.port()));
    let backend_id = DeviceId::new(format!("alpaca:127.0.0.1:{}", fake.addr.port()));
    AlpacaMount::new(AlpacaTelescope::new(client, 0), &backend_id)
}

/// A connected mount on a full-featured fake telescope.
async fn connected(fake: &FakeAlpaca) -> AlpacaMount {
    fake.set_all(SCOPE, full_telescope());
    let mount = mount(fake).await;
    mount.connect().await.expect("connect");
    mount
}

fn number(params: &alpaca_common::Params, name: &str) -> f64 {
    params[name].parse().unwrap()
}

#[tokio::test]
async fn connect_sets_connected_and_reads_what_the_mount_can_do() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;

    assert_eq!(fake.puts(&key("connected"))[0]["Connected"], "True");
    assert!(mount.is_connected());
    let caps = mount.capabilities();
    assert!(caps.can_slew && caps.can_sync && caps.can_park && caps.can_unpark);
    assert!(caps.can_track && caps.can_move_axis && caps.can_slew_alt_az);
    assert!(caps.can_find_home && caps.can_pulse_guide);
    assert!(caps.can_set_tracking_rate);
    assert_eq!(
        caps.tracking_rates,
        [
            TrackingRate::Sidereal,
            TrackingRate::Lunar,
            TrackingRate::Solar
        ]
    );
    assert_eq!(caps.equatorial_system, Epoch::J2000);

    let info = mount.info();
    assert_eq!(
        info.id.as_str(),
        format!("alpaca:127.0.0.1:{}:telescope:0", fake.addr.port())
    );
    assert_eq!(info.kind, DeviceKind::Mount);
    assert_eq!(info.name, "Fake Mount");
    assert_eq!(info.driver.as_deref(), Some("fake driver 1.0"));
}

#[tokio::test]
async fn a_minimal_mount_reports_only_what_it_has() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(
        SCOPE,
        json!({"connected": true, "name": "Bare", "canslewasync": true, "equatorialsystem": 1,
            "canmoveaxis": false}),
    );
    for property in [
        "driverinfo",
        "trackingrates",
        "cansync",
        "canpark",
        "canunpark",
        "cansettracking",
        "canslewaltazasync",
        "canfindhome",
        "canpulseguide",
    ] {
        fake.not_implemented(&key(property));
    }
    let mount = mount(&fake).await;
    mount.connect().await.unwrap();

    let caps = mount.capabilities();
    assert!(caps.can_slew);
    assert!(!caps.can_sync && !caps.can_park && !caps.can_unpark && !caps.can_track);
    assert!(!caps.can_move_axis && !caps.can_find_home && !caps.can_pulse_guide);
    assert!(caps.tracking_rates.is_empty() && !caps.can_set_tracking_rate);
    // Topocentric is the equinox of date.
    assert_eq!(caps.equatorial_system, Epoch::JNow);
    assert!(
        mount
            .move_axis(MountAxis::Primary, 0.5)
            .await
            .unwrap_err()
            .is_not_supported()
    );
    assert!(mount.stop_motion().await.unwrap_err().is_not_supported());
}

#[tokio::test]
async fn a_failure_while_reading_capabilities_fails_the_connect() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(SCOPE, full_telescope());
    fake.fail(&key("cansync"), 0x500, "driver crashed");
    let mount = mount(&fake).await;
    assert!(mount.connect().await.is_err());
    assert!(!mount.is_connected());
}

#[tokio::test]
async fn a_j2000_target_goes_unchanged_to_a_j2000_mount() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    mount
        .slew_to_coordinates(&Coordinates::from_hours(5.5, -5.4).unwrap())
        .await
        .unwrap();
    let sent = &fake.puts(&key("slewtocoordinatesasync"))[0];
    assert!((number(sent, "RightAscension") - 5.5).abs() < 1e-9);
    assert!((number(sent, "Declination") + 5.4).abs() < 1e-9);
}

#[tokio::test]
async fn a_j2000_target_is_precessed_for_a_mount_that_works_in_the_equinox_of_date() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(SCOPE, full_telescope());
    fake.set(&key("equatorialsystem"), json!(1));
    let mount = mount(&fake).await;
    mount.connect().await.unwrap();

    let j2000 = Coordinates::from_hours(6.0, 20.0).unwrap();
    mount.slew_to_coordinates(&j2000).await.unwrap();
    let sent = &fake.puts(&key("slewtocoordinatesasync"))[0];
    let expected = j2000.to_epoch(Epoch::JNow, chrono::Utc::now());
    assert!((number(sent, "RightAscension") - expected.ra.as_hours()).abs() < 1e-6);
    assert!((number(sent, "Declination") - expected.dec.as_degrees()).abs() < 1e-6);
    assert!(number(sent, "RightAscension") - 6.0 > 0.02);

    // And the position read back is labelled as the mount's system.
    assert_eq!(mount.get_coordinates().await.unwrap().epoch, Epoch::JNow);
}

#[tokio::test]
async fn a_mount_in_an_old_equinox_connects_but_cannot_be_slewed() {
    let fake = FakeAlpaca::start().await;
    fake.set_all(SCOPE, full_telescope());
    fake.set(&key("equatorialsystem"), json!(4)); // B1950
    let mount = mount(&fake).await;
    mount.connect().await.unwrap();

    let target = Coordinates::from_hours(6.0, 20.0).unwrap();
    let err = mount.slew_to_coordinates(&target).await.unwrap_err();
    assert!(err.is_not_supported(), "{err:?}");
    assert!(err.to_string().contains("B1950"), "{err}");
    assert!(
        mount
            .get_coordinates()
            .await
            .unwrap_err()
            .is_not_supported()
    );
    assert!(fake.puts(&key("slewtocoordinatesasync")).is_empty());
}

#[tokio::test]
async fn a_slew_returns_on_acceptance_and_the_wait_ends_when_the_mount_stops() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    fake.on_put(&key("slewtocoordinatesasync"), |values, _| {
        values.insert("telescope/0/slewing".into(), json!(true));
    });

    let target = Coordinates::from_hours(7.0, 30.0).unwrap();
    mount.slew_to_coordinates(&target).await.unwrap();
    // Accepted, and still moving.
    assert!(mount.is_slewing().await.unwrap());
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Slewing
    );

    let opts = WaitOptions {
        poll_interval: Duration::from_millis(20),
        timeout: Duration::from_secs(5),
        settle: Duration::ZERO,
    };
    let waiting = tokio::spawn({
        let mount = Arc::new(mount);
        let waiter = Arc::clone(&mount);
        async move { wait_for_slew(waiter.as_ref(), opts).await }
    });
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!waiting.is_finished());
    fake.set(&key("slewing"), json!(false));
    waiting.await.unwrap().unwrap();
}

#[tokio::test]
async fn slewing_a_parked_mount_is_refused_without_sending_the_slew() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    fake.set(&key("atpark"), json!(true));

    let target = Coordinates::from_hours(7.0, 30.0).unwrap();
    let err = mount.slew_to_coordinates(&target).await.unwrap_err();
    assert!(matches!(err, ScopinatorError::Parked), "{err:?}");
    assert!(fake.puts(&key("slewtocoordinatesasync")).is_empty());
    assert_eq!(
        mount.get_mount_status().await.unwrap().slew_state,
        SlewState::Parked
    );
}

#[tokio::test]
async fn the_drivers_own_parked_error_is_also_parked() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    // A driver that does not implement AtPark but refuses the slew.
    fake.not_implemented(&key("atpark"));
    fake.fail(&key("slewtocoordinatesasync"), 0x408, "mount is parked");
    let target = Coordinates::from_hours(7.0, 30.0).unwrap();
    let err = mount.slew_to_coordinates(&target).await.unwrap_err();
    assert!(matches!(err, ScopinatorError::Parked), "{err:?}");
}

#[tokio::test]
async fn park_returns_on_acceptance_and_unpark_leaves_park() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    fake.on_put(&key("unpark"), |values, _| {
        values.insert("telescope/0/atpark".into(), json!(false));
    });

    mount.park().await.unwrap();
    assert!(!mount.is_parked().await.unwrap());
    fake.set(&key("atpark"), json!(true));
    let opts = WaitOptions {
        poll_interval: Duration::from_millis(10),
        ..WaitOptions::default()
    };
    wait_for_park(&mount, opts).await.unwrap();

    mount.unpark().await.unwrap();
    assert!(!mount.is_parked().await.unwrap());
    assert_eq!(
        fake.put_keys()
            .iter()
            .filter(|k| k.ends_with("/park"))
            .count(),
        1
    );
}

#[tokio::test]
async fn sync_sends_the_position_in_the_mounts_system() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    // A position given in the equinox of date, for a J2000 mount.
    let of_date = Coordinates::with_epoch(
        RaDegrees::from_hours(6.0).unwrap(),
        DecDegrees::new(20.0).unwrap(),
        Epoch::JNow,
    );
    mount.sync_to_coordinates(&of_date).await.unwrap();
    let sent = &fake.puts(&key("synctocoordinates"))[0];
    let expected = of_date.to_epoch(Epoch::J2000, chrono::Utc::now());
    assert!((number(sent, "RightAscension") - expected.ra.as_hours()).abs() < 1e-6);
    assert!(number(sent, "RightAscension") < 6.0 - 0.02);
}

#[tokio::test]
async fn axis_speed_is_a_fraction_of_the_fastest_rate_the_axis_offers() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;

    // The fake offers up to 4 degrees per second. It is a German
    // equatorial mount, where a positive Alpaca rate on the primary axis
    // decreases the right ascension: the sign is turned round.
    mount.move_axis(MountAxis::Primary, 0.5).await.unwrap();
    mount.move_axis(MountAxis::Secondary, -1.0).await.unwrap();
    let moves = fake.puts(&key("moveaxis"));
    assert_eq!(
        (moves[0]["Axis"].as_str(), number(&moves[0], "Rate")),
        ("0", -2.0)
    );
    assert_eq!(
        (moves[1]["Axis"].as_str(), number(&moves[1], "Rate")),
        ("1", -4.0)
    );

    mount.stop_motion().await.unwrap();
    let moves = fake.puts(&key("moveaxis"));
    assert_eq!(moves.len(), 4);
    assert_eq!(
        (moves[2]["Axis"].as_str(), number(&moves[2], "Rate")),
        ("0", 0.0)
    );
    assert_eq!(
        (moves[3]["Axis"].as_str(), number(&moves[3], "Rate")),
        ("1", 0.0)
    );

    let err = mount.move_axis(MountAxis::Primary, 1.1).await.unwrap_err();
    assert!(
        matches!(err, ScopinatorError::InvalidArgument(_)),
        "{err:?}"
    );
    assert_eq!(fake.puts(&key("moveaxis")).len(), 4);
}

#[tokio::test]
async fn a_positive_primary_speed_is_sent_as_it_is_to_an_alt_az_mount() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;

    // Azimuth increases with the axis: nothing to turn round.
    fake.set(&key("alignmentmode"), json!(0));
    mount.move_axis(MountAxis::Primary, 0.5).await.unwrap();
    // Nor for a mount that does not say how it is aligned.
    fake.not_implemented(&key("alignmentmode"));
    mount.move_axis(MountAxis::Primary, -0.25).await.unwrap();

    let moves = fake.puts(&key("moveaxis"));
    assert_eq!(number(&moves[0], "Rate"), 2.0);
    assert_eq!(number(&moves[1], "Rate"), -1.0);
}

#[tokio::test]
async fn site_and_clock_are_read_and_written() {
    use chrono::TimeZone;

    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;

    let site = mount.get_site().await.unwrap();
    assert_eq!(site.latitude_deg(), 41.88);
    assert_eq!(site.longitude_deg(), -87.63);
    assert_eq!(site.elevation_m(), Some(180.0));

    mount
        .set_site(&SiteLocation::new(-33.87, 151.21).unwrap())
        .await
        .unwrap();
    assert_eq!(
        number(&fake.puts(&key("sitelatitude"))[0], "SiteLatitude"),
        -33.87
    );
    assert_eq!(
        number(&fake.puts(&key("sitelongitude"))[0], "SiteLongitude"),
        151.21
    );
    // No elevation given, none written.
    assert!(fake.puts(&key("siteelevation")).is_empty());

    assert_eq!(
        mount.get_utc().await.unwrap(),
        chrono::Utc
            .with_ymd_and_hms(2026, 11, 15, 17, 18, 54)
            .unwrap()
    );
    mount
        .set_utc(chrono::Utc.with_ymd_and_hms(2026, 11, 16, 3, 0, 0).unwrap())
        .await
        .unwrap();
    assert_eq!(
        fake.puts(&key("utcdate"))[0]["UTCDate"],
        "2026-11-16T03:00:00.000Z"
    );
}

#[tokio::test]
async fn the_remaining_members_map_onto_their_alpaca_counterparts() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;

    let horizontal = mount.get_alt_az().await.unwrap();
    assert_eq!((horizontal.altitude, horizontal.azimuth), (45.0, 180.0));
    assert_eq!(mount.side_of_pier().await.unwrap(), PierSide::East);
    assert_eq!(
        mount.alignment_mode().await.unwrap(),
        AlignmentMode::GermanPolar
    );
    assert!(!mount.is_at_home().await.unwrap());
    assert!(mount.is_tracking().await.unwrap());

    mount.set_tracking(false).await.unwrap();
    assert_eq!(fake.puts(&key("tracking"))[0]["Tracking"], "False");
    mount.set_tracking_rate(TrackingRate::Lunar).await.unwrap();
    assert_eq!(fake.puts(&key("trackingrate"))[0]["TrackingRate"], "1");
    assert!(
        mount
            .set_tracking_rate(TrackingRate::Custom)
            .await
            .unwrap_err()
            .is_not_supported()
    );

    mount
        .pulse_guide(GuideDirection::East, Duration::from_millis(250))
        .await
        .unwrap();
    let pulse = &fake.puts(&key("pulseguide"))[0];
    assert_eq!(
        (pulse["Direction"].as_str(), pulse["Duration"].as_str()),
        ("2", "250")
    );

    mount.find_home().await.unwrap();
    mount.abort_slew().await.unwrap();
    let keys = fake.put_keys();
    assert!(keys.contains(&key("findhome")) && keys.contains(&key("abortslew")));

    let status = mount.get_mount_status().await.unwrap();
    assert_eq!(status.slew_state, SlewState::Tracking);
    assert_eq!(status.tracking_rate, Some(TrackingRate::Sidereal));
    assert_eq!(status.coordinates.unwrap().ra.as_hours(), 6.0);
}

#[tokio::test]
async fn disconnect_clears_connected_on_the_device() {
    let fake = FakeAlpaca::start().await;
    let mount = connected(&fake).await;
    mount.disconnect().await.unwrap();
    assert!(!mount.is_connected());
    assert_eq!(
        fake.puts(&key("connected")).last().unwrap()["Connected"],
        "False"
    );
}
