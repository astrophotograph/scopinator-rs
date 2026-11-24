//! Autofocus and plate solving on a Seestar through the neutral traits.
//! Both are followed from the telescope's events.
#![cfg(feature = "seestar")]

#[path = "../../scopinator-seestar/tests/common/mod.rs"]
mod common;

use std::net::Ipv4Addr;
use std::sync::Arc;
use std::time::Duration;

use common::FakeSeestar;
use scopinator::backend::seestar::{SeestarBackend, SeestarEndpoints};
use scopinator::{
    AutoFocus, Backend, PlateSolver, UnifiedEventBus, WaitOptions, wait_for_autofocus,
    wait_for_solution,
};
use scopinator_seestar::SeestarConfig;
use scopinator_types::{AutoFocusState, Epoch};
use serde_json::json;

async fn backend(fake: &FakeSeestar) -> SeestarBackend {
    fake.respond_ok(
        "get_device_state",
        json!({"device": {"name": "fake", "firmware_ver_int": 2670}}),
    );
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

fn fast() -> WaitOptions {
    WaitOptions {
        poll_interval: Duration::from_millis(20),
        timeout: Duration::from_secs(5),
        settle: Duration::ZERO,
    }
}

fn sent(fake: &FakeSeestar, method: &str) -> usize {
    fake.received()
        .iter()
        .filter(|msg| msg["method"] == method)
        .count()
}

/// Poll `state` until it satisfies `done`, for up to two seconds: events
/// reach the backend a moment after the fake sends them.
async fn eventually<T, F, Fut>(mut state: F, done: impl Fn(&T) -> bool) -> T
where
    F: FnMut() -> Fut,
    Fut: std::future::Future<Output = T>,
{
    for _ in 0..100 {
        let value = state().await;
        if done(&value) {
            return value;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the state did not change within two seconds");
}

#[tokio::test]
async fn autofocus_runs_from_acceptance_until_the_telescope_reports_it_done() {
    let fake = FakeSeestar::start().await;
    fake.respond_ok("get_focuser_position", json!(1580));
    let backend = backend(&fake).await;
    let focus: Arc<dyn AutoFocus> = Backend::auto_focus(&backend).expect("autofocus");
    assert_eq!(focus.autofocus_state().await.unwrap(), AutoFocusState::Idle);

    focus.start_autofocus().await.unwrap();
    // The firmware spells it this way.
    assert_eq!(sent(&fake, "start_auto_focuse"), 1);
    // Running at once, before any event.
    assert_eq!(
        focus.autofocus_state().await.unwrap(),
        AutoFocusState::Running
    );

    let waiting = tokio::spawn({
        let focus = Arc::clone(&focus);
        async move { wait_for_autofocus(focus.as_ref(), fast()).await }
    });
    fake.send_event(
        json!({"Event": "AutoFocus", "Timestamp": "1.0", "state": "working",
        "lapse_ms": 500}),
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!waiting.is_finished());

    fake.send_event(
        json!({"Event": "AutoFocus", "Timestamp": "2.0", "state": "complete",
        "lapse_ms": 9000}),
    );
    // The wait returns where the focuser ended.
    assert_eq!(waiting.await.unwrap().unwrap(), Some(1580));
    backend.disconnect().await;
}

#[tokio::test]
async fn a_failed_autofocus_is_reported_once() {
    let fake = FakeSeestar::start().await;
    let backend = backend(&fake).await;
    let focus = Backend::auto_focus(&backend).unwrap();
    focus.start_autofocus().await.unwrap();
    fake.send_event(
        json!({"Event": "AutoFocus", "Timestamp": "1.0", "state": "fail",
        "lapse_ms": 4000}),
    );

    let outcome = eventually(
        || async { focus.autofocus_state().await },
        |state| !matches!(state, Ok(AutoFocusState::Running)),
    )
    .await;
    assert!(
        outcome
            .unwrap_err()
            .to_string()
            .contains("autofocus failed")
    );
    assert_eq!(focus.autofocus_state().await.unwrap(), AutoFocusState::Idle);
    backend.disconnect().await;
}

#[tokio::test]
async fn stopping_an_autofocus_leaves_it_idle() {
    let fake = FakeSeestar::start().await;
    let backend = backend(&fake).await;
    let focus = Backend::auto_focus(&backend).unwrap();
    focus.start_autofocus().await.unwrap();
    focus.stop_autofocus().await.unwrap();
    assert_eq!(sent(&fake, "stop_auto_focuse"), 1);
    assert_eq!(focus.autofocus_state().await.unwrap(), AutoFocusState::Idle);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_solve_has_no_solution_until_the_telescope_reports_one() {
    let fake = FakeSeestar::start().await;
    let backend = backend(&fake).await;
    let solver: Arc<dyn PlateSolver> = Backend::plate_solver(&backend).expect("solver");
    assert_eq!(solver.solution().await.unwrap(), None);

    solver.start_solve().await.unwrap();
    assert_eq!(sent(&fake, "start_solve"), 1);
    let waiting = tokio::spawn({
        let solver = Arc::clone(&solver);
        async move { wait_for_solution(solver.as_ref(), fast()).await }
    });
    // Shapes from the session corpus.
    fake.send_event(
        json!({"Event": "PlateSolve", "Timestamp": "1.0", "page": "stack",
        "state": "solving", "star_number": 1473}),
    );
    tokio::time::sleep(Duration::from_millis(80)).await;
    assert!(!waiting.is_finished());
    fake.send_event(
        json!({"Event": "PlateSolve", "Timestamp": "2.0", "state": "complete",
        "lapse_ms": 2733, "ra_dec": [8.69067, 19.502521]}),
    );

    let solution = waiting.await.unwrap().unwrap();
    assert!((solution.center.ra.as_hours() - 8.69067).abs() < 1e-9);
    assert!((solution.center.dec.as_degrees() - 19.502521).abs() < 1e-9);
    assert_eq!(solution.center.epoch, Epoch::JNow);
    // It stays available.
    assert!(solver.solution().await.unwrap().is_some());

    // A new solve clears it until the next result.
    solver.start_solve().await.unwrap();
    assert_eq!(solver.solution().await.unwrap(), None);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_failed_solve_is_reported_once() {
    let fake = FakeSeestar::start().await;
    let backend = backend(&fake).await;
    let solver = Backend::plate_solver(&backend).unwrap();
    solver.start_solve().await.unwrap();
    fake.send_event(
        json!({"Event": "PlateSolve", "Timestamp": "1.0", "state": "fail",
        "error": "solve failed", "code": 251}),
    );

    let outcome = eventually(|| async { solver.solution().await }, |s| s.is_err()).await;
    let text = outcome.unwrap_err().to_string();
    assert!(
        text.contains("solve failed") && text.contains("251"),
        "{text}"
    );
    assert_eq!(solver.solution().await.unwrap(), None);
    backend.disconnect().await;
}

#[tokio::test]
async fn a_solve_the_telescope_ran_by_itself_counts() {
    // While centring a target the telescope plate solves without being
    // asked; its result is the latest solution.
    let fake = FakeSeestar::start().await;
    let backend = backend(&fake).await;
    let solver = Backend::plate_solver(&backend).unwrap();
    fake.send_event(
        json!({"Event": "PlateSolve", "Timestamp": "2.0", "state": "complete",
        "lapse_ms": 2733, "ra_dec": [8.69067, 19.502521], "route": ["View", "AutoGoto"]}),
    );
    let solution = eventually(
        || async { solver.solution().await.unwrap() },
        Option::is_some,
    )
    .await;
    assert!(solution.is_some());
    backend.disconnect().await;
}
