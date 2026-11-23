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
use scopinator_types::{
    Coordinates, DecDegrees, Epoch, MountAxis, RaDegrees, SiteLocation, SlewState,
};
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

/// The `target_ra_dec` of the last goto the fake telescope received.
fn sent_target(fake: &FakeSeestar) -> (f64, f64) {
    let request = fake
        .received()
        .into_iter()
        .rev()
        .find(|msg| msg["method"] == "iscope_start_view")
        .expect("a goto was sent");
    let target = &request["params"]["target_ra_dec"];
    (
        target[0].as_f64().expect("ra"),
        target[1].as_f64().expect("dec"),
    )
}

#[tokio::test]
async fn a_j2000_target_is_sent_in_the_equinox_of_date() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    assert_eq!(mount.capabilities().equatorial_system, Epoch::JNow);

    let j2000 = Coordinates::from_hours(6.0, 20.0).unwrap();
    assert_eq!(j2000.epoch, Epoch::J2000);
    mount.slew_to_coordinates(&j2000).await.unwrap();

    let (ra_hours, dec_deg) = sent_target(&fake);
    let expected = j2000.to_epoch(Epoch::JNow, chrono::Utc::now());
    assert!(
        (ra_hours - expected.ra.as_hours()).abs() < 1e-6,
        "{ra_hours}"
    );
    assert!(
        (dec_deg - expected.dec.as_degrees()).abs() < 1e-6,
        "{dec_deg}"
    );
    // And that is not what was passed in: precession since 2000 moves a
    // position at 6 h by more than a minute of right ascension.
    assert!(ra_hours - 6.0 > 0.02, "{ra_hours}");
    backend.disconnect().await;
}

#[tokio::test]
async fn a_target_already_in_the_equinox_of_date_is_sent_unchanged() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    let of_date = Coordinates::with_epoch(
        RaDegrees::from_hours(6.0).unwrap(),
        DecDegrees::new(20.0).unwrap(),
        Epoch::JNow,
    );
    mount.slew_to_coordinates(&of_date).await.unwrap();
    assert_eq!(sent_target(&fake), (6.0, 20.0));
    backend.disconnect().await;
}

#[tokio::test]
async fn the_reported_position_is_labelled_equinox_of_date() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    fake.respond_ok("scope_get_equ_coord", json!({"ra": 20.6917, "dec": 45.279}));
    let backend = backend(&fake).await;

    let position = backend.mount().get_coordinates().await.unwrap();
    assert_eq!(position.epoch, Epoch::JNow);
    assert!((position.ra.as_hours() - 20.6917).abs() < 1e-9);
    assert!((position.dec.as_degrees() - 45.279).abs() < 1e-9);
    backend.disconnect().await;
}

/// The parameters of every request for `method` the fake telescope got.
fn sent(fake: &FakeSeestar, method: &str) -> Vec<Value> {
    fake.received()
        .into_iter()
        .filter(|msg| msg["method"] == method)
        .map(|msg| msg["params"].clone())
        .collect()
}

#[tokio::test]
async fn sync_sends_the_position_in_the_equinox_of_date() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    assert!(mount.capabilities().can_sync);

    let j2000 = Coordinates::from_hours(6.0, 20.0).unwrap();
    mount.sync_to_coordinates(&j2000).await.unwrap();

    let params = sent(&fake, "scope_sync");
    assert_eq!(params.len(), 1);
    let expected = j2000.to_epoch(Epoch::JNow, chrono::Utc::now());
    let ra = params[0][0].as_f64().unwrap();
    let dec = params[0][1].as_f64().unwrap();
    assert!((ra - expected.ra.as_hours()).abs() < 1e-6, "{ra}");
    assert!((dec - expected.dec.as_degrees()).abs() < 1e-6, "{dec}");
    backend.disconnect().await;
}

#[tokio::test]
async fn a_refused_sync_is_an_error() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    fake.respond_error("scope_sync", 207, "fail to operate");
    let backend = backend(&fake).await;
    let err = backend
        .mount()
        .sync_to_coordinates(&target())
        .await
        .unwrap_err();
    assert!(err.to_string().contains("207"), "{err}");
    backend.disconnect().await;
}

#[tokio::test]
async fn move_axis_jogs_in_the_direction_of_the_axis_and_sign() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    assert!(mount.capabilities().can_move_axis);

    // Angles as found on an S50 in equatorial mode: west 0, north 90, east
    // 180, south 270. East increases right ascension, north declination.
    mount.move_axis(MountAxis::Primary, 0.6).await.unwrap();
    mount.move_axis(MountAxis::Primary, -1.0).await.unwrap();
    mount.move_axis(MountAxis::Secondary, 0.25).await.unwrap();
    mount.move_axis(MountAxis::Secondary, -0.5).await.unwrap();

    let moves: Vec<(i64, i64)> = sent(&fake, "scope_speed_move")
        .iter()
        .map(|p| (p["angle"].as_i64().unwrap(), p["percent"].as_i64().unwrap()))
        .collect();
    assert_eq!(moves, [(180, 60), (0, 100), (90, 25), (270, 50)]);
    // Each move stops by itself after ten seconds.
    for params in sent(&fake, "scope_speed_move") {
        assert_eq!(params["dur_sec"], 10);
    }
    backend.disconnect().await;
}

#[tokio::test]
async fn zero_speed_and_stop_motion_stop_the_jog() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    mount.move_axis(MountAxis::Primary, 0.0).await.unwrap();
    mount.stop_motion().await.unwrap();
    let stops = sent(&fake, "scope_speed_move");
    assert_eq!(stops.len(), 2);
    for params in stops {
        assert_eq!(params["percent"], 0);
    }
    backend.disconnect().await;
}

#[tokio::test]
async fn an_axis_speed_outside_minus_one_to_one_is_refused_before_anything_is_sent() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", true, false));
    let backend = backend(&fake).await;
    let mount = backend.mount();

    for speed in [1.5, -1.01, f64::NAN, f64::INFINITY] {
        let err = mount
            .move_axis(MountAxis::Primary, speed)
            .await
            .unwrap_err();
        assert!(
            matches!(err, ScopinatorError::InvalidArgument(_)),
            "{speed}: {err:?}"
        );
    }
    assert!(sent(&fake, "scope_speed_move").is_empty());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_seestar_has_no_unpark_request() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    let backend = backend(&fake).await;
    let mount = backend.mount();
    assert!(!mount.capabilities().can_unpark);
    assert!(mount.unpark().await.unwrap_err().is_not_supported());
    backend.disconnect().await;
}

#[tokio::test]
async fn the_site_is_read_longitude_first_and_written_by_name() {
    let fake = FakeSeestar::start().await;
    // The device state lists longitude before latitude.
    fake.respond_ok(
        "get_device_state",
        json!({"device": {"name": "fake", "firmware_ver_int": 2975},
            "location_lon_lat": [-87.63, 41.88]}),
    );
    let backend = backend(&fake).await;
    let mount = backend.mount();
    let caps = mount.capabilities();
    assert!(caps.can_set_site && caps.can_set_time);

    let site = mount.get_site().await.unwrap();
    assert_eq!(site.latitude_deg(), 41.88);
    assert_eq!(site.longitude_deg(), -87.63);
    assert_eq!(site.elevation_m(), None);

    let new_site = SiteLocation::new(-33.87, 151.21).unwrap();
    mount.set_site(&new_site).await.unwrap();
    let params = sent(&fake, "set_user_location");
    assert_eq!(params.len(), 1);
    assert_eq!(params[0]["lat"], -33.87);
    assert_eq!(params[0]["lon"], 151.21);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_state_reply_without_a_location_is_an_error() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok(
        "get_device_state",
        json!({"device": {"name": "fake", "firmware_ver_int": 2975}}),
    );
    let backend = backend(&fake).await;
    assert!(backend.mount().get_site().await.is_err());
    backend.disconnect().await;
}

#[tokio::test]
async fn the_clock_is_read_through_the_telescope_time_zone() {
    use chrono::TimeZone;

    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    fake.respond_ok(
        "pi_get_time",
        json!({"year": 2026, "mon": 11, "day": 15, "hour": 11, "min": 18, "sec": 54,
            "time_zone": "America/Chicago"}),
    );
    let backend = backend(&fake).await;
    assert_eq!(
        backend.mount().get_utc().await.unwrap(),
        chrono::Utc
            .with_ymd_and_hms(2026, 11, 15, 17, 18, 54)
            .unwrap()
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn setting_the_clock_keeps_the_telescope_time_zone() {
    use chrono::TimeZone;

    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    // A telescope whose clock was never set: years off, but with a zone.
    fake.respond_ok(
        "pi_get_time",
        json!({"year": 2020, "mon": 10, "day": 10, "hour": 3, "min": 0, "sec": 0,
            "time_zone": "America/Chicago"}),
    );
    let backend = backend(&fake).await;
    let now = chrono::Utc.with_ymd_and_hms(2026, 11, 16, 3, 0, 0).unwrap();
    backend.mount().set_utc(now).await.unwrap();

    let params = sent(&fake, "pi_set_time");
    assert_eq!(params.len(), 1);
    // `pi_set_time` takes its fields in a one-element array.
    let time = if params[0].is_array() {
        &params[0][0]
    } else {
        &params[0]
    };
    // Local time in Chicago, the evening before, and the zone is not
    // replaced with UTC.
    assert_eq!(time["time_zone"], "America/Chicago");
    assert_eq!(
        (
            time["year"].as_i64(),
            time["mon"].as_i64(),
            time["day"].as_i64()
        ),
        (Some(2026), Some(11), Some(15))
    );
    assert_eq!(
        (
            time["hour"].as_i64(),
            time["min"].as_i64(),
            time["sec"].as_i64()
        ),
        (Some(21), Some(0), Some(0))
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn a_telescope_with_no_usable_zone_is_set_to_utc() {
    use chrono::TimeZone;

    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", mount_state("none", false, true));
    fake.respond_ok("pi_get_time", json!({}));
    let backend = backend(&fake).await;
    let now = chrono::Utc.with_ymd_and_hms(2026, 11, 16, 3, 0, 0).unwrap();
    backend.mount().set_utc(now).await.unwrap();

    let params = sent(&fake, "pi_set_time");
    let time = if params[0].is_array() {
        &params[0][0]
    } else {
        &params[0]
    };
    assert_eq!(time["time_zone"], "UTC");
    assert_eq!(
        (time["day"].as_i64(), time["hour"].as_i64()),
        (Some(16), Some(3))
    );
    backend.disconnect().await;
}
