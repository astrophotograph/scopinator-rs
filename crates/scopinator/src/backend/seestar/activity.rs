//! Autofocus and plate solving on a Seestar.
//!
//! Neither shows up in a state the telescope can be asked for, only in the
//! events it sends while they run. A listener keeps the latest state of
//! each, and the two neutral traits answer from it.

use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::event::{EventState, SeestarEvent};
use scopinator_types::{AutoFocusState, Coordinates, DecDegrees, PlateSolution, RaDegrees};
use serde_json::Value;

use super::GotoTracker;
use super::mount::SEESTAR_EPOCH;
use crate::device::smart::{AutoFocus, PlateSolver};
use crate::error::ScopinatorError;

/// Where one kind of run stands.
#[derive(Debug, Clone, PartialEq, Default)]
enum Run<T> {
    /// None has been seen.
    #[default]
    Idle,
    Running,
    Done(T),
    /// Failed, and nobody has been told yet.
    Failed(String),
}

/// The latest autofocus and plate solve the telescope has reported,
/// whoever started them.
#[derive(Debug, Default)]
pub(crate) struct Activity {
    autofocus: Mutex<Run<()>>,
    solve: Mutex<Run<PlateSolution>>,
    /// Told when the telescope reports that a goto failed.
    goto: Arc<GotoTracker>,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

impl Activity {
    /// An activity record that also tells `goto` about failed gotos.
    pub(crate) fn following(goto: Arc<GotoTracker>) -> Self {
        Self {
            goto,
            ..Self::default()
        }
    }

    /// Follow `client`'s events until its event channel closes.
    pub(crate) fn listen(self: &Arc<Self>, client: &SeestarClient) {
        let mut events = client.subscribe_events();
        let activity = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                match events.recv().await {
                    Ok(event) => activity.note(&event),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    fn note(&self, event: &SeestarEvent) {
        match event {
            SeestarEvent::AutoFocus(data) => {
                let next = match data.state.as_ref() {
                    Some(EventState::Start | EventState::Working) => Run::Running,
                    Some(EventState::Complete) => Run::Done(()),
                    Some(EventState::Fail) => {
                        Run::Failed(autofocus_failure(data.error.as_deref(), data.code))
                    }
                    // Cancelled: as if it had not run.
                    Some(EventState::Cancel) => Run::Idle,
                    _ => return,
                };
                *lock(&self.autofocus) = next;
            }
            SeestarEvent::AutoGoto(data) => {
                if matches!(data.state, Some(EventState::Fail)) {
                    let reason = data.error.as_deref().unwrap_or("no reason given");
                    self.goto.failed(reason, data.code);
                }
            }
            SeestarEvent::PlateSolve(value) => {
                let Some(next) = solve_run(value) else {
                    return;
                };
                let mut solve = lock(&self.solve);
                *solve = match next {
                    Solve::Now(run) => run,
                    // The second of the two `complete` events: the first
                    // had the position.
                    Solve::CompleteWithoutPosition if matches!(*solve, Run::Done(_)) => return,
                    Solve::CompleteWithoutPosition => {
                        Run::Failed("the solve finished without a position".into())
                    }
                };
            }
            _ => {}
        }
    }
}

/// The words for a failed autofocus, with the telescope's reason if it
/// gave one.
fn autofocus_failure(error: Option<&str>, code: Option<i32>) -> String {
    match (error, code) {
        (Some(error), Some(code)) => format!("autofocus failed: {error} (code {code})"),
        (Some(error), None) => format!("autofocus failed: {error}"),
        (None, Some(code)) => format!("autofocus failed (code {code})"),
        (None, None) => "autofocus failed".into(),
    }
}

/// What one `PlateSolve` event says.
#[derive(Debug, PartialEq)]
enum Solve {
    /// The solve is now in this state.
    Now(Run<PlateSolution>),
    /// `complete`, with no position in the event.
    CompleteWithoutPosition,
}

/// What a `PlateSolve` event says about the solve. The telescope reports
/// `start`, `solving` and `working` while it runs, `fail`, and `complete`
/// twice: first with a `result` holding the centre in `ra_dec` (hours,
/// degrees) and the field in `fov` (degrees), then once more from the
/// function that asked for the solve. That second event repeats `ra_dec`
/// when a goto asked, and has no position when `start_solve` did (S30,
/// firmware 9.75).
fn solve_run(event: &Value) -> Option<Solve> {
    match event.get("state").and_then(Value::as_str)? {
        "start" | "solving" | "working" => Some(Solve::Now(Run::Running)),
        "complete" => {
            let pair = |path: &str| {
                let number = |index: usize| event.pointer(&format!("{path}/{index}"))?.as_f64();
                number(0).zip(number(1))
            };
            let centre = pair("/result/ra_dec").or_else(|| pair("/ra_dec")).and_then(
                |(ra_hours, dec_degrees)| {
                    Some(Coordinates::with_epoch(
                        RaDegrees::from_hours(ra_hours.rem_euclid(24.0)).ok()?,
                        DecDegrees::new(dec_degrees).ok()?,
                        SEESTAR_EPOCH,
                    ))
                },
            );
            Some(match centre {
                Some(centre) => {
                    let mut solution = PlateSolution::new(centre);
                    solution.field_of_view_deg = pair("/result/fov");
                    Solve::Now(Run::Done(solution))
                }
                None => Solve::CompleteWithoutPosition,
            })
        }
        "fail" => {
            let error = event
                .get("error")
                .and_then(Value::as_str)
                .unwrap_or("no reason given");
            Some(Solve::Now(Run::Failed(
                match event.get("code").and_then(Value::as_i64) {
                    Some(code) => format!("{error} (code {code})"),
                    None => error.to_string(),
                },
            )))
        }
        "cancel" => Some(Solve::Now(Run::Idle)),
        _ => None,
    }
}

/// Take a failure out of `run`, so that it is reported once.
fn take_failure<T>(run: &Mutex<Run<T>>) -> Option<String> {
    let mut run = lock(run);
    match std::mem::replace(&mut *run, Run::Idle) {
        Run::Failed(reason) => Some(reason),
        other => {
            *run = other;
            None
        }
    }
}

/// A Seestar focusing itself (`start_auto_focuse`).
pub struct SeestarAutoFocus {
    client: Arc<SeestarClient>,
    activity: Arc<Activity>,
}

impl SeestarAutoFocus {
    pub(crate) fn new(client: Arc<SeestarClient>, activity: Arc<Activity>) -> Self {
        Self { client, activity }
    }
}

#[async_trait]
impl AutoFocus for SeestarAutoFocus {
    async fn start_autofocus(&self) -> Result<(), ScopinatorError> {
        super::mount::command_ok(self.client.start_auto_focus().await?)?;
        // Running from acceptance, before the telescope's first event.
        *lock(&self.activity.autofocus) = Run::Running;
        Ok(())
    }

    async fn stop_autofocus(&self) -> Result<(), ScopinatorError> {
        super::mount::command_ok(self.client.stop_auto_focus().await?)?;
        *lock(&self.activity.autofocus) = Run::Idle;
        Ok(())
    }

    async fn autofocus_state(&self) -> Result<AutoFocusState, ScopinatorError> {
        if let Some(reason) = take_failure(&self.activity.autofocus) {
            return Err(ScopinatorError::Backend(reason));
        }
        let run = lock(&self.activity.autofocus).clone();
        Ok(match run {
            Run::Idle | Run::Failed(_) => AutoFocusState::Idle,
            Run::Running => AutoFocusState::Running,
            Run::Done(()) => AutoFocusState::Done {
                // Where the focuser is now; a telescope that will not say
                // has still finished focusing.
                position: self
                    .client
                    .send_and_validate(Command::GetFocuserPosition)
                    .await
                    .ok()
                    .and_then(|reply| reply.as_i64())
                    .and_then(|position| i32::try_from(position).ok()),
            },
        })
    }
}

/// A Seestar working out where it points (`start_solve`).
pub struct SeestarPlateSolver {
    client: Arc<SeestarClient>,
    activity: Arc<Activity>,
}

impl SeestarPlateSolver {
    pub(crate) fn new(client: Arc<SeestarClient>, activity: Arc<Activity>) -> Self {
        Self { client, activity }
    }
}

#[async_trait]
impl PlateSolver for SeestarPlateSolver {
    async fn start_solve(&self) -> Result<(), ScopinatorError> {
        self.client.send_and_validate(Command::StartSolve).await?;
        *lock(&self.activity.solve) = Run::Running;
        Ok(())
    }

    /// The latest solve the telescope finished, which can be one it ran by
    /// itself while centring a target. The centre is in the telescope's
    /// equatorial system; rotation and scale are not reported.
    async fn solution(&self) -> Result<Option<PlateSolution>, ScopinatorError> {
        if let Some(reason) = take_failure(&self.activity.solve) {
            return Err(ScopinatorError::Backend(format!(
                "plate solve failed: {reason}"
            )));
        }
        Ok(match &*lock(&self.activity.solve) {
            Run::Done(solution) => Some(solution.clone()),
            _ => None,
        })
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn event(value: Value) -> SeestarEvent {
        serde_json::from_value(value).unwrap()
    }

    // Shapes from the session corpus.
    #[test]
    fn a_completed_solve_gives_its_centre_in_hours_and_degrees() {
        let run = solve_run(&json!({"Event": "PlateSolve", "state": "complete",
            "lapse_ms": 2733, "ra_dec": [8.69067, 19.502521], "route": ["View", "AutoGoto"]}))
        .unwrap();
        match run {
            Solve::Now(Run::Done(solution)) => {
                assert!((solution.center.ra.as_hours() - 8.69067).abs() < 1e-9);
                assert!((solution.center.dec.as_degrees() - 19.502521).abs() < 1e-9);
                assert_eq!(solution.center.epoch, SEESTAR_EPOCH);
                assert_eq!(solution.rotation_deg, None);
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn solve_states_follow_the_telescope() {
        for state in ["start", "solving", "working"] {
            let run = solve_run(&json!({"state": state, "star_number": 711}));
            assert_eq!(run, Some(Solve::Now(Run::Running)), "{state}");
        }
        assert_eq!(
            solve_run(&json!({"state": "fail", "error": "solve failed", "code": 251})),
            Some(Solve::Now(Run::Failed("solve failed (code 251)".into())))
        );
        assert_eq!(
            solve_run(&json!({"state": "cancel"})),
            Some(Solve::Now(Run::Idle))
        );
        // Complete without a position is not a solution.
        assert_eq!(
            solve_run(&json!({"state": "complete"})),
            Some(Solve::CompleteWithoutPosition)
        );
        // Not a state this library knows: ignored.
        assert_eq!(solve_run(&json!({"state": "idle"})), None);
        assert_eq!(solve_run(&json!({})), None);
    }

    #[test]
    fn events_move_the_autofocus_and_solve_states() {
        let activity = Activity::default();
        activity.note(&event(json!({"Event": "AutoFocus", "Timestamp": "1.0",
            "state": "working", "lapse_ms": 100})));
        assert_eq!(*lock(&activity.autofocus), Run::Running);
        activity.note(&event(json!({"Event": "AutoFocus", "Timestamp": "2.0",
            "state": "complete", "lapse_ms": 9000})));
        assert_eq!(*lock(&activity.autofocus), Run::Done(()));

        activity.note(&event(json!({"Event": "PlateSolve", "Timestamp": "3.0",
            "page": "stack", "state": "solving", "star_number": 1473})));
        assert_eq!(*lock(&activity.solve), Run::Running);
        // Other events change nothing.
        activity.note(&event(
            json!({"Event": "View", "Timestamp": "4.0", "state": "working",
            "lapse_ms": 0, "mode": "star"}),
        ));
        assert_eq!(*lock(&activity.solve), Run::Running);
        assert_eq!(*lock(&activity.autofocus), Run::Done(()));
    }

    #[test]
    fn a_failed_autofocus_carries_the_reason_the_telescope_gave() {
        let activity = Activity::default();
        // The reason and code as an S30 (9.75) gives them in its view state.
        activity.note(&event(json!({"Event": "AutoFocus", "Timestamp": "1.0",
            "state": "fail", "lapse_ms": 80507, "error": "no star is detected",
            "code": 279})));
        assert_eq!(
            take_failure(&activity.autofocus),
            Some("autofocus failed: no star is detected (code 279)".into())
        );
        // Without one, the failure is still a failure.
        activity.note(&event(json!({"Event": "AutoFocus", "Timestamp": "2.0",
            "state": "fail", "lapse_ms": 4000})));
        assert_eq!(
            take_failure(&activity.autofocus),
            Some("autofocus failed".into())
        );
    }

    // The events of one `start_solve` on an S30 (9.75), 2026-11-27.
    #[test]
    fn a_solve_asked_for_by_hand_keeps_the_position_of_its_first_complete_event() {
        let activity = Activity::default();
        *lock(&activity.solve) = Run::Running;
        for value in [
            json!({"Event": "PlateSolve", "Timestamp": "1291.06", "lapse_ms": 0,
                "route": [], "state": "working"}),
            json!({"Event": "PlateSolve", "Timestamp": "1291.07", "page": "focus",
                "state": "start"}),
            json!({"Event": "PlateSolve", "Timestamp": "1293.17", "page": "focus",
                "star_number": 817, "state": "solving"}),
            json!({"Event": "PlateSolve", "Timestamp": "1293.83", "page": "focus",
                "result": {"angle": 0.779999, "duration_ms": 2768, "focal_len": 149.809067,
                    "fov": [1.1987, 2.12953], "image_id": 65535,
                    "ra_dec": [0.164201, 29.244796], "star_number": 817},
                "state": "complete"}),
            json!({"Event": "PlateSolve", "Timestamp": "1293.84", "lapse_ms": 2771,
                "route": [], "state": "complete"}),
        ] {
            activity.note(&event(value));
        }
        match &*lock(&activity.solve) {
            Run::Done(solution) => {
                assert!((solution.center.ra.as_hours() - 0.164201).abs() < 1e-9);
                assert!((solution.center.dec.as_degrees() - 29.244796).abs() < 1e-9);
                assert_eq!(solution.field_of_view_deg, Some((1.1987, 2.12953)));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_solve_that_completes_with_no_position_at_all_has_failed() {
        let activity = Activity::default();
        *lock(&activity.solve) = Run::Running;
        activity.note(&event(json!({"Event": "PlateSolve", "Timestamp": "1.0",
            "lapse_ms": 2771, "route": [], "state": "complete"})));
        assert!(matches!(*lock(&activity.solve), Run::Failed(_)));
    }

    #[test]
    fn a_failure_is_taken_once() {
        let run = Mutex::new(Run::<()>::Failed("no stars".into()));
        assert_eq!(take_failure(&run), Some("no stars".into()));
        assert_eq!(take_failure(&run), None);
        assert_eq!(*lock(&run), Run::Idle);

        let done = Mutex::new(Run::Done(()));
        assert_eq!(take_failure(&done), None);
        assert_eq!(*lock(&done), Run::Done(()));
    }
}
