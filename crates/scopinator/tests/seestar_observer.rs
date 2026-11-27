//! Finding targets with a Seestar through the neutral `Observer`.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use chrono::{Datelike, Timelike};
use common::FakeSeestar;
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{
    Backend, Mount, Observer, ScopinatorError, UnifiedEventBus, WaitOptions, wait_for_acquisition,
};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{
    AcquisitionState, Coordinates, Epoch, Prerequisite, Readiness, SolarSystemBody, Target,
};
use serde_json::{Value, json};

/// The device state of a telescope whose mount is doing `move_type`, at a
/// site of (`lon`, `lat`).
fn state(move_type: &str, site: [f64; 2]) -> Value {
    json!({
        "device": {"name": "fake", "firmware_ver_int": 2975, "product_model": "Seestar S30"},
        "mount": {"move_type": move_type, "tracking": false, "close": false, "equ_mode": false},
        "location_lon_lat": site,
    })
}

const CHICAGO: [f64; 2] = [-87.63, 41.88];

/// `pi_get_time` for a clock `offset` away from now, in UTC.
fn clock(offset: chrono::Duration) -> Value {
    let t = chrono::Utc::now() + offset;
    json!({"year": t.year(), "mon": t.month(), "day": t.day(), "hour": t.hour(),
        "min": t.minute(), "sec": t.second(), "time_zone": "UTC"})
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

fn observer(backend: &SeestarBackend) -> Arc<dyn Observer> {
    Backend::observer(backend).expect("a Seestar has an observer")
}

fn sent(fake: &FakeSeestar, method: &str) -> Vec<Value> {
    fake.received()
        .into_iter()
        .filter(|msg| msg["method"] == method)
        .map(|msg| msg["params"].clone())
        .collect()
}

#[tokio::test]
async fn a_telescope_with_its_clock_and_site_set_is_ready() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok("pi_get_time", clock(chrono::Duration::hours(-6)));
    let backend = backend(&fake).await;
    assert_eq!(
        observer(&backend).readiness().await.unwrap(),
        Readiness::Ready
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn an_unset_clock_and_an_unset_site_are_named() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", [0.0, 0.0]));
    // An S30 that has lost its clock reads years back.
    fake.respond_ok("pi_get_time", clock(chrono::Duration::days(-2200)));
    let backend = backend(&fake).await;
    assert_eq!(
        observer(&backend).readiness().await.unwrap(),
        Readiness::Needs(vec![Prerequisite::Clock, Prerequisite::SiteLocation])
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn prepare_sets_a_wrong_clock_and_leaves_a_right_one_alone() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok(
        "pi_get_time",
        json!({"year": 2020, "mon": 10, "day": 10, "hour": 3, "min": 0, "sec": 0,
            "time_zone": "America/Chicago"}),
    );
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    assert!(observer.capabilities().can_prepare);

    observer.prepare().await.unwrap();
    let set = sent(&fake, "pi_set_time");
    assert_eq!(set.len(), 1);
    let time = if set[0].is_array() {
        &set[0][0]
    } else {
        &set[0]
    };
    // In the telescope's own zone, and this year.
    assert_eq!(time["time_zone"], "America/Chicago");
    assert!(time["year"].as_i64().unwrap() >= 2026);

    // With the clock right, prepare sends nothing.
    fake.respond_ok("pi_get_time", clock(chrono::Duration::zero()));
    observer.prepare().await.unwrap();
    assert_eq!(sent(&fake, "pi_set_time").len(), 1);
    backend.disconnect().await;
}

#[tokio::test]
async fn prepare_cannot_invent_a_site() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", [0.0, 0.0]));
    fake.respond_ok("pi_get_time", clock(chrono::Duration::zero()));
    let backend = backend(&fake).await;
    match observer(&backend).prepare().await {
        Err(ScopinatorError::NotReady(needs)) => {
            assert_eq!(needs, [Prerequisite::SiteLocation]);
        }
        other => panic!("got {other:?}"),
    }
    assert!(sent(&fake, "set_user_location").is_empty());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_sky_target_is_sent_by_name_in_the_equinox_of_date() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    let backend = backend(&fake).await;
    let j2000 = Coordinates::from_hours(6.0, 20.0).unwrap();

    observer(&backend)
        .acquire(&Target::named("M35", j2000))
        .await
        .unwrap();
    let view = &sent(&fake, "iscope_start_view")[0];
    assert_eq!(view["mode"], "star");
    assert_eq!(view["target_name"], "M35");
    let expected = j2000.to_epoch(Epoch::JNow, chrono::Utc::now());
    let ra = view["target_ra_dec"][0].as_f64().unwrap();
    assert!((ra - expected.ra.as_hours()).abs() < 1e-6, "{ra}");
    assert!(ra - 6.0 > 0.02);

    // A target with no name is named after its coordinates.
    observer(&backend)
        .acquire(&Target::at(j2000))
        .await
        .unwrap();
    let view = &sent(&fake, "iscope_start_view")[1];
    assert!(
        view["target_name"]
            .as_str()
            .unwrap()
            .starts_with("RA 6.0000")
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn solar_system_and_scenery_targets_choose_the_view_mode() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    let caps = observer.capabilities();
    assert!(caps.sky_targets && caps.scenery && caps.centres_by_plate_solve);
    assert!(caps.solar_system.contains(&SolarSystemBody::Saturn));

    for body in [
        SolarSystemBody::Sun,
        SolarSystemBody::Moon,
        SolarSystemBody::Jupiter,
    ] {
        observer.acquire(&Target::SolarSystem(body)).await.unwrap();
    }
    observer.acquire(&Target::Scenery).await.unwrap();

    let views = sent(&fake, "iscope_start_view");
    let summary: Vec<(&str, Option<&str>)> = views
        .iter()
        .map(|view| (view["mode"].as_str().unwrap(), view["target_type"].as_str()))
        .collect();
    assert_eq!(
        summary,
        [
            ("solar_sys", Some("sun")),
            ("solar_sys", Some("moon")),
            // The telescope has one kind of target for all the planets.
            ("solar_sys", Some("planet")),
            ("scenery", None),
        ]
    );
    // No coordinates are sent for any of them.
    assert!(views.iter().all(|view| view.get("target_ra_dec").is_none()));
    backend.disconnect().await;
}

#[tokio::test]
async fn acquisition_goes_from_slewing_through_centring_to_on_target() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Idle
    );

    let target = Target::named("M35", Coordinates::from_hours(6.0, 20.0).unwrap());
    observer.acquire(&target).await.unwrap();
    // Accepted; the telescope has said nothing yet.
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Slewing
    );

    fake.respond_ok("get_device_state", state("ScopeGoto", CHICAGO));
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "ScopeGoto": {"state": "working"}}}}),
    );
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Slewing
    );

    let opts = WaitOptions {
        poll_interval: Duration::from_millis(20),
        ..WaitOptions::default()
    };
    let waiting = tokio::spawn({
        let observer = Arc::clone(&observer);
        async move { wait_for_acquisition(observer.as_ref(), opts).await }
    });

    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "ScopeGoto": {"state": "complete"},
                "PlateSolve": {"state": "working"}}}}),
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Centring
    );
    assert!(!waiting.is_finished());

    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star", "target_name": "M35",
            "AutoGoto": {"state": "complete"}}}),
    );
    waiting.await.unwrap().unwrap();
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::OnTarget
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn a_failed_acquisition_is_reported_once_and_shared_with_the_mount() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    observer
        .acquire(&Target::at(Coordinates::from_hours(6.0, 20.0).unwrap()))
        .await
        .unwrap();

    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working",
                "ScopeGoto": {"state": "fail", "code": 501, "error": "mount goto failed"}}}}),
    );
    let err = observer.acquisition().await.unwrap_err();
    assert!(err.to_string().contains("mount goto failed"), "{err}");
    // Once: after that the telescope is simply not looking for anything,
    // for the observer and for the mount alike.
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Idle
    );
    assert!(!backend.mount().is_slewing().await.unwrap());
    backend.disconnect().await;
}

// Seen on an S30 (9.75) indoors: seven failed plate solves
// under an `AutoGoto` that stayed `working`, then one `AutoGoto` event with
// `fail`, after which the view state has no `AutoGoto` at all.
#[tokio::test]
async fn a_goto_that_never_solves_fails_through_its_event_and_is_not_on_target() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    observer
        .acquire(&Target::at(Coordinates::from_hours(4.6, 16.5).unwrap()))
        .await
        .unwrap();

    // A failed solve under a working goto: the telescope is trying again.
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star", "stage": "AutoGoto",
            "AutoGoto": {"state": "working", "count": 3, "hint": true, "stage": "PlateSolve",
                "PlateSolve": {"state": "fail", "code": 251, "error": "solve failed"}}}}),
    );
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Centring
    );
    assert!(backend.mount().is_slewing().await.unwrap());

    // It gives up, and the view carries on as it would after a success.
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star", "stage": "ContinuousExposure",
            "ContinuousExposure": {"state": "working", "fps": 0.09}}}),
    );
    fake.send_event(
        json!({"Event": "AutoGoto", "Timestamp": "350.0", "state": "fail",
        "lapse_ms": 349600, "count": 7, "hint": true, "error": "solve failed", "code": 251}),
    );
    let mut failure = None;
    for _ in 0..100 {
        match observer.acquisition().await {
            Err(e) => {
                failure = Some(e.to_string());
                break;
            }
            Ok(_) => tokio::time::sleep(Duration::from_millis(20)).await,
        }
    }
    let failure = failure.expect("the failed goto was never reported");
    assert!(failure.contains("solve failed (code 251)"), "{failure}");
    // Once; and the view that is left is not a view of the target.
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Idle
    );
    assert!(!backend.mount().is_slewing().await.unwrap());
    backend.disconnect().await;
}

#[tokio::test]
async fn a_failed_goto_still_in_the_view_state_is_not_read_as_running() {
    // The failure event comes about two seconds before the goto leaves the
    // view state (S30, 9.75): for those seconds it still shows as working.
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "working", "mode": "star", "stage": "AutoGoto",
            "AutoGoto": {"state": "working", "count": 7, "hint": true, "stage": "PlateSolve",
                "PlateSolve": {"state": "fail", "code": 251, "error": "solve failed"}}}}),
    );
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    observer
        .acquire(&Target::at(Coordinates::from_hours(4.6, 16.5).unwrap()))
        .await
        .unwrap();
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Centring
    );

    fake.send_event(
        json!({"Event": "AutoGoto", "Timestamp": "350.0", "state": "fail",
        "lapse_ms": 349600, "count": 7, "hint": true, "error": "solve failed", "code": 251}),
    );
    let mut failed = false;
    for _ in 0..100 {
        if observer.acquisition().await.is_err() {
            failed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(failed, "the failed goto was never reported");

    // The view state has not caught up; the goto is over all the same.
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Idle
    );
    assert!(!backend.mount().is_slewing().await.unwrap());

    // A new goto is followed from the view state again.
    observer
        .acquire(&Target::at(Coordinates::from_hours(5.0, 10.0).unwrap()))
        .await
        .unwrap();
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Centring
    );
    backend.disconnect().await;
}

#[tokio::test]
async fn stop_cancels_the_goto_and_leaves_the_view() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_device_state", state("none", CHICAGO));
    fake.respond_ok("get_view_state", json!({}));
    let backend = backend(&fake).await;
    let observer = observer(&backend);
    observer
        .acquire(&Target::at(Coordinates::from_hours(6.0, 20.0).unwrap()))
        .await
        .unwrap();
    observer.stop().await.unwrap();

    let stops = sent(&fake, "iscope_stop_view");
    assert_eq!(stops.len(), 2);
    assert_eq!(stops[0]["stage"], "AutoGoto");
    // The cancelled goto is not a failed acquisition, and not "slewing".
    fake.respond_ok(
        "get_view_state",
        json!({"View": {"state": "cancel", "mode": "none",
            "AutoGoto": {"state": "fail", "code": 207, "error": "fail to operate"}}}),
    );
    assert_eq!(
        observer.acquisition().await.unwrap(),
        AcquisitionState::Idle
    );
    backend.disconnect().await;
}
