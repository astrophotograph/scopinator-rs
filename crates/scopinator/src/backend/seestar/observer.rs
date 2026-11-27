use std::sync::Arc;

use async_trait::async_trait;
use chrono::Utc;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{
    SolarTarget, StartViewParams, StopStage, StopViewParams, ViewMode,
};
use scopinator_types::{AcquisitionState, Prerequisite, Readiness, SolarSystemBody, Target};
use serde_json::Value;

use super::GotoTracker;
use super::mount::{
    DeviceClock, GOTO_START_GRACE, SEESTAR_EPOCH, command_ok, goto_failure, time_in_zone,
};
use crate::device::smart::{Observer, ObserverCapabilities};
use crate::error::ScopinatorError;

/// A clock further than this from the computer's is taken to be unset. A
/// time zone moves a clock by a day at most; an unset one is years off.
const CLOCK_TOLERANCE: chrono::Duration = chrono::Duration::days(2);

/// Finding targets with a Seestar: `iscope_start_view`, which slews, plate
/// solves and re-centres by itself, and the stages it reports in the view
/// state.
pub struct SeestarObserver {
    client: Arc<SeestarClient>,
    /// Shared with the mount: either can start a goto.
    goto: Arc<GotoTracker>,
}

/// Whether the clock or the site is known to be wrong. `None` means the
/// telescope could not be asked.
struct Checks {
    clock_wrong: Option<bool>,
    site_unset: Option<bool>,
    /// The telescope's time zone, to keep when the clock is set.
    zone: Option<chrono_tz::Tz>,
}

impl SeestarObserver {
    pub(crate) fn new(client: Arc<SeestarClient>, goto: Arc<GotoTracker>) -> Self {
        Self { client, goto }
    }

    async fn checks(&self) -> Checks {
        let now = Utc::now();
        let clock = match self.client.send_and_validate(Command::PiGetTime).await {
            Ok(reply) => DeviceClock::parse(&reply).ok(),
            Err(_) => None,
        };
        let clock_wrong = clock.as_ref().map(|clock| match clock.utc() {
            Ok(theirs) => (theirs - now).abs() > CLOCK_TOLERANCE,
            // A zone this library does not know: compare the wall clock
            // with UTC, which is within the tolerance for any real zone.
            Err(_) => (clock.local - now.naive_utc()).abs() > CLOCK_TOLERANCE,
        });
        let site_unset = match self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["location_lon_lat"]})),
            )
            .await
        {
            Ok(state) => Some(site_is_unset(&state)),
            Err(_) => None,
        };
        Checks {
            clock_wrong,
            site_unset,
            zone: clock.and_then(|clock| clock.zone),
        }
    }

    async fn start_view(&self, params: StartViewParams) -> Result<(), ScopinatorError> {
        command_ok(
            self.client
                .send_command(Command::IscopeStartView(params))
                .await?,
        )
    }
}

/// True if the device state has no site, or the (0, 0) a Seestar reports
/// before it has been told one.
fn site_is_unset(state: &Value) -> bool {
    let degrees = |index: usize| {
        state
            .pointer(&format!("/location_lon_lat/{index}"))
            .and_then(Value::as_f64)
    };
    match (degrees(0), degrees(1)) {
        (Some(longitude), Some(latitude)) => longitude == 0.0 && latitude == 0.0,
        _ => true,
    }
}

fn readiness_of(checks: &Checks) -> Readiness {
    let mut needs = Vec::new();
    if checks.clock_wrong == Some(true) {
        needs.push(Prerequisite::Clock);
    }
    if checks.site_unset == Some(true) {
        needs.push(Prerequisite::SiteLocation);
    }
    if !needs.is_empty() {
        Readiness::Needs(needs)
    } else if checks.clock_wrong.is_none() && checks.site_unset.is_none() {
        Readiness::Unknown
    } else {
        Readiness::Ready
    }
}

/// How far a goto has got according to a `get_view_state` reply, leaving
/// out what only the mount and the tracker know. `Err` is a failed stage.
fn stage_of(view: &Value) -> Result<Stage, String> {
    let Some(auto_goto) = view.pointer("/View/AutoGoto") else {
        return Ok(Stage::NoGoto);
    };
    if let Some(failure) = goto_failure(auto_goto) {
        return Err(failure);
    }
    let running = |value: Option<&Value>| {
        matches!(
            value.and_then(|v| v.get("state")).and_then(Value::as_str),
            Some("start" | "working")
        )
    };
    // Before the first stage appears the telescope is opening its arm.
    let has_stage = auto_goto
        .as_object()
        .is_some_and(|members| members.values().any(Value::is_object));
    Ok(if !running(Some(auto_goto)) {
        Stage::NoGoto
    } else if running(auto_goto.get("ScopeGoto")) || !has_stage {
        Stage::Slewing
    } else {
        Stage::Centring
    })
}

#[derive(Debug, PartialEq)]
enum Stage {
    /// No goto in the view state, or one that has ended.
    NoGoto,
    Slewing,
    /// A goto is running and the mount is not being slewed: plate solving
    /// and re-centring.
    Centring,
}

#[async_trait]
impl Observer for SeestarObserver {
    fn capabilities(&self) -> ObserverCapabilities {
        use SolarSystemBody::*;
        ObserverCapabilities {
            sky_targets: true,
            // The Sun and the Moon by name; the planets as one kind of
            // target that the telescope picks among.
            solar_system: vec![
                Sun, Moon, Mercury, Venus, Mars, Jupiter, Saturn, Uranus, Neptune,
            ],
            scenery: true,
            centres_by_plate_solve: true,
            can_prepare: true,
        }
    }

    /// A Seestar needs its clock and its site. With either wrong it can
    /// accept a goto and not move, without saying why.
    async fn readiness(&self) -> Result<Readiness, ScopinatorError> {
        Ok(readiness_of(&self.checks().await))
    }

    /// Set the clock from this computer if it is wrong, keeping the
    /// telescope's time zone. The site cannot be made up: if it is unset,
    /// this is a `NotReady` error and the site has to be given with
    /// [`Mount::set_site`](crate::Mount::set_site).
    async fn prepare(&self) -> Result<(), ScopinatorError> {
        let checks = self.checks().await;
        if checks.clock_wrong == Some(true) {
            self.client
                .send_and_validate(Command::PiSetTime(time_in_zone(Utc::now(), checks.zone)))
                .await?;
        }
        if checks.site_unset == Some(true) {
            return Err(ScopinatorError::NotReady(vec![Prerequisite::SiteLocation]));
        }
        Ok(())
    }

    async fn acquire(&self, target: &Target) -> Result<(), ScopinatorError> {
        match target {
            Target::Sky { name, coordinates } => {
                let of_date = coordinates.to_epoch(SEESTAR_EPOCH, Utc::now());
                let name = name.clone().unwrap_or_else(|| {
                    format!(
                        "RA {:.4} Dec {:.4}",
                        coordinates.ra.as_hours(),
                        coordinates.dec.as_degrees()
                    )
                });
                self.start_view(StartViewParams::goto(
                    name,
                    of_date.ra.as_hours(),
                    of_date.dec.as_degrees(),
                ))
                .await?;
                self.goto.accepted();
            }
            Target::SolarSystem(body) => {
                let target_type = match body {
                    SolarSystemBody::Sun => SolarTarget::Sun,
                    SolarSystemBody::Moon => SolarTarget::Moon,
                    _ => SolarTarget::Planet,
                };
                self.start_view(StartViewParams {
                    mode: Some(ViewMode::SolarSys),
                    target_name: None,
                    target_ra_dec: None,
                    target_type: Some(target_type),
                    lp_filter: None,
                })
                .await?;
                self.goto.accepted();
            }
            Target::Scenery => {
                // The view of wherever the telescope points: no goto.
                self.start_view(StartViewParams {
                    mode: Some(ViewMode::Scenery),
                    target_name: None,
                    target_ra_dec: None,
                    target_type: None,
                    lp_filter: None,
                })
                .await?;
                self.goto.clear();
            }
            other => {
                return Err(ScopinatorError::NotSupported(format!(
                    "the target {other:?}"
                )));
            }
        }
        Ok(())
    }

    async fn acquisition(&self) -> Result<AcquisitionState, ScopinatorError> {
        if let Some(failure) = self.goto.take_failure() {
            return Err(ScopinatorError::Backend(format!(
                "goto failed at {failure}"
            )));
        }
        let view = self.client.send_and_validate(Command::GetViewState).await?;
        match stage_of(&view) {
            Ok(Stage::Slewing) => {
                self.goto.running();
                return Ok(AcquisitionState::Slewing);
            }
            Ok(Stage::Centring) => {
                self.goto.running();
                return Ok(AcquisitionState::Centring);
            }
            Ok(Stage::NoGoto) => {}
            // The failed stage stays in the view state: report the failure
            // of a goto sent through this backend once.
            Err(failure) => {
                if self.goto.is_pending() {
                    self.goto.clear();
                    return Err(ScopinatorError::Backend(format!(
                        "goto failed at {failure}"
                    )));
                }
                return Ok(AcquisitionState::Idle);
            }
        }
        // No goto stage: the mount may still be on its way, or the
        // telescope may not have started on a goto it has just accepted.
        let state = self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["mount"]})),
            )
            .await?;
        let moving = state
            .pointer("/mount/move_type")
            .and_then(Value::as_str)
            .is_some_and(|kind| kind != "none");
        if moving {
            self.goto.running();
            return Ok(AcquisitionState::Slewing);
        }
        if self.goto.awaiting_start(GOTO_START_GRACE) {
            return Ok(AcquisitionState::Slewing);
        }
        self.goto.finished();
        let viewing = matches!(
            view.pointer("/View/state").and_then(Value::as_str),
            Some("start" | "working")
        );
        // After a failed goto the telescope stays in the view it had
        // started, pointing where the mount got to unaided.
        Ok(if viewing && !self.goto.last_failed() {
            AcquisitionState::OnTarget
        } else {
            AcquisitionState::Idle
        })
    }

    /// Cancel a goto and leave the view.
    async fn stop(&self) -> Result<(), ScopinatorError> {
        self.client
            .send_and_validate(Command::IscopeStopView(Some(StopViewParams {
                stage: StopStage::AutoGoto,
            })))
            .await?;
        self.client
            .send_and_validate(Command::IscopeStopView(None))
            .await?;
        // The cancelled goto shows as failed; it was asked for.
        self.goto.clear();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn checks(clock_wrong: Option<bool>, site_unset: Option<bool>) -> Checks {
        Checks {
            clock_wrong,
            site_unset,
            zone: None,
        }
    }

    #[test]
    fn readiness_names_what_is_wrong() {
        assert_eq!(
            readiness_of(&checks(Some(false), Some(false))),
            Readiness::Ready
        );
        assert_eq!(
            readiness_of(&checks(Some(true), Some(false))),
            Readiness::Needs(vec![Prerequisite::Clock])
        );
        assert_eq!(
            readiness_of(&checks(Some(false), Some(true))),
            Readiness::Needs(vec![Prerequisite::SiteLocation])
        );
        assert_eq!(
            readiness_of(&checks(Some(true), Some(true))),
            Readiness::Needs(vec![Prerequisite::Clock, Prerequisite::SiteLocation])
        );
    }

    #[test]
    fn what_could_not_be_asked_is_not_held_against_the_telescope() {
        // Neither could be read: unknown, not ready and not "needs".
        assert_eq!(readiness_of(&checks(None, None)), Readiness::Unknown);
        // One read and fine, the other unreadable: ready as far as is known.
        assert_eq!(readiness_of(&checks(Some(false), None)), Readiness::Ready);
        // One read and wrong: that is needed whatever the other says.
        assert_eq!(
            readiness_of(&checks(None, Some(true))),
            Readiness::Needs(vec![Prerequisite::SiteLocation])
        );
    }

    #[test]
    fn a_site_of_zero_zero_or_none_is_unset() {
        assert!(site_is_unset(&json!({"location_lon_lat": [0.0, 0.0]})));
        assert!(site_is_unset(&json!({})));
        assert!(site_is_unset(&json!({"location_lon_lat": [1.0]})));
        assert!(!site_is_unset(
            &json!({"location_lon_lat": [-87.63, 41.88]})
        ));
        // On the equator, or on the Greenwich meridian, is somewhere.
        assert!(!site_is_unset(&json!({"location_lon_lat": [36.8, 0.0]})));
        assert!(!site_is_unset(&json!({"location_lon_lat": [0.0, 51.48]})));
    }

    #[test]
    fn the_goto_stage_is_slewing_while_scope_goto_runs_and_centring_after() {
        let slewing = json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "ScopeGoto": {"state": "working"}}}});
        assert_eq!(stage_of(&slewing), Ok(Stage::Slewing));

        let solving = json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working", "ScopeGoto": {"state": "complete"},
                "PlateSolve": {"state": "working"}}}});
        assert_eq!(stage_of(&solving), Ok(Stage::Centring));

        let done = json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "complete"}}});
        assert_eq!(stage_of(&done), Ok(Stage::NoGoto));
        assert_eq!(stage_of(&json!({})), Ok(Stage::NoGoto));
        assert_eq!(
            stage_of(&json!({"View": {"state": "working", "mode": "scenery"}})),
            Ok(Stage::NoGoto)
        );
    }

    #[test]
    fn a_failed_stage_under_a_working_goto_is_a_failure() {
        // As seen on an S30 with an unset clock.
        let view = json!({"View": {"state": "working", "mode": "star",
            "AutoGoto": {"state": "working",
                "ScopeGoto": {"state": "fail", "code": 501, "error": "mount goto failed"}}}});
        let failure = stage_of(&view).unwrap_err();
        assert!(
            failure.contains("ScopeGoto") && failure.contains("501"),
            "{failure}"
        );
    }
    // The four shapes below are from an S30 (9.75) with no stars in view,
    // 2026-11-26: the goto went on for six minutes of failed plate solves.
    #[test]
    fn a_goto_with_no_stage_yet_is_slewing() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"count": 0, "hint": false, "lapse_ms": 323, "state": "working"}}});
        assert_eq!(stage_of(&view), Ok(Stage::Slewing));
    }

    #[test]
    fn a_failed_plate_solve_under_a_working_goto_is_still_centring() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"count": 3, "hint": true, "stage": "PlateSolve", "state": "working",
                "PlateSolve": {"code": 251, "error": "solve failed", "state": "fail"}}}});
        assert_eq!(stage_of(&view), Ok(Stage::Centring));
    }

    #[test]
    fn a_failed_autofocus_under_a_working_goto_is_still_centring() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"count": 2, "stage": "AutoFocus", "state": "working",
                "AutoFocus": {"code": 279, "error": "no star is detected", "state": "fail",
                    "stage": "FocuserMove",
                    "FocuserMove": {"position": 1253, "state": "complete"}}}}});
        assert_eq!(stage_of(&view), Ok(Stage::Centring));
    }

    #[test]
    fn a_mount_that_could_not_move_fails_a_goto_still_marked_working() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"state": "working", "stage": "ScopeGoto",
                "ScopeGoto": {"state": "fail", "error": "mount goto failed", "code": 501}}}});
        let failure = stage_of(&view).unwrap_err();
        assert!(failure.contains("501"), "{failure}");
    }

    #[test]
    fn a_goto_that_says_it_failed_has_failed() {
        let view = json!({"View": {"state": "working", "stage": "AutoGoto",
            "AutoGoto": {"state": "fail", "error": "aborted", "code": 253}}});
        let failure = stage_of(&view).unwrap_err();
        assert!(failure.contains("aborted"), "{failure}");
    }
}
