use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_types::{
    Coordinates, DecDegrees, DeviceId, DeviceInfo, DeviceKind, Epoch, RaDegrees,
};

use crate::device::capabilities::MountCapabilities;
use crate::device::status::{DeviceStatus, MountStatus};
use crate::device::traits::{Device, Mount};
use crate::error::ScopinatorError;

use super::GotoTracker;

/// How long after a goto is accepted the mount is taken to be slewing even
/// though the telescope does not say so yet. An S30 took 6 s to start
/// moving. A goto the telescope accepts and then silently ignores (seen with
/// an unset clock) stops counting as a slew when this runs out.
const GOTO_START_GRACE: Duration = Duration::from_secs(15);

/// The equatorial system a Seestar takes goto targets in and reports its
/// position in: the equinox of date, according to the protocol
/// specification. This has not been checked against a telescope; plate-solve
/// centring hides the difference from J2000 (about 0.37 degrees in 2026) on
/// the device itself.
const SEESTAR_EPOCH: Epoch = Epoch::JNow;

/// Seestar mount adapter.
pub struct SeestarMount {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
    goto: Arc<GotoTracker>,
}

/// The `mount` section of the device state.
struct MountState {
    /// `move_type`: what is moving the mount (`ScopeGoto`, ...), or `none`.
    moving: bool,
    tracking: bool,
    /// `close`: the arm is folded, which is the park position.
    parked: bool,
}

impl SeestarMount {
    pub(crate) fn new(
        client: Arc<SeestarClient>,
        device_id: DeviceId,
        goto: Arc<GotoTracker>,
    ) -> Self {
        Self {
            client,
            device_id,
            goto,
        }
    }

    /// Ask the telescope for the `mount` section of its state. A reply
    /// without one of the members is an error, not "false": a mount that is
    /// not known to be parked must not be reported as unparked.
    async fn mount_state(&self) -> Result<MountState, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate(
                "get_device_state",
                Some(serde_json::json!({"keys": ["mount"]})),
            )
            .await?;
        let member = |name: &str| {
            state.pointer(&format!("/mount/{name}")).ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "get_device_state reply has no `mount.{name}`: {state}"
                ))
            })
        };
        let flag = |name: &str| {
            member(name)?.as_bool().ok_or_else(|| {
                ScopinatorError::Backend(format!("`mount.{name}` is not a boolean: {state}"))
            })
        };
        Ok(MountState {
            moving: member("move_type")?.as_str() != Some("none"),
            tracking: flag("tracking")?,
            parked: flag("close")?,
        })
    }

    /// Where the goto stands according to the view state, which nests its
    /// stages by name: `View` > `AutoGoto` > `ScopeGoto`.
    async fn goto_stage(&self) -> Result<GotoStage, ScopinatorError> {
        let view = self.client.send_and_validate(Command::GetViewState).await?;
        let Some(auto_goto) = view.pointer("/View/AutoGoto") else {
            return Ok(GotoStage::None);
        };
        // A failed inner stage can sit under an outer one that still says
        // `working`, so look for a failure anywhere below first.
        if let Some(failure) = find_failure(auto_goto, "AutoGoto") {
            return Ok(GotoStage::Failed(failure));
        }
        Ok(match auto_goto.get("state").and_then(|s| s.as_str()) {
            Some("start" | "working") => GotoStage::Running,
            _ => GotoStage::None,
        })
    }
}

enum GotoStage {
    /// No goto in the view state, or one that has ended without failing.
    None,
    /// Slewing, plate solving or re-centring.
    Running,
    Failed(String),
}

/// The first stage at or below `value` that reports `state: fail`, as
/// "stage: error (code n)".
fn find_failure(value: &serde_json::Value, name: &str) -> Option<String> {
    let object = value.as_object()?;
    if object.get("state").and_then(|s| s.as_str()) == Some("fail") {
        let error = object
            .get("error")
            .and_then(|e| e.as_str())
            .unwrap_or("no reason given");
        return Some(match object.get("code").and_then(|c| c.as_i64()) {
            Some(code) => format!("{name}: {error} (code {code})"),
            None => format!("{name}: {error}"),
        });
    }
    object
        .iter()
        .find_map(|(key, child)| find_failure(child, key))
}

#[async_trait]
impl Device for SeestarMount {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info = DeviceInfo::new(self.device_id.clone(), DeviceKind::Mount, "Seestar mount");
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        // Connection is managed by the backend
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.is_control_connected()
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        Ok(DeviceStatus {
            connected: self.is_connected(),
            name: Some(self.device_id.to_string()),
            description: Some("Seestar mount".into()),
        })
    }
}

#[async_trait]
impl Mount for SeestarMount {
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError> {
        let response = self
            .client
            .send_and_validate(Command::ScopeGetEquCoord)
            .await?;

        // Response result contains ra (hours) and dec (degrees). A reply
        // without them is an error: (0, 0) is a real position, not "unknown".
        let coord = |name: &str| {
            response.get(name).and_then(|v| v.as_f64()).ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "scope_get_equ_coord reply has no numeric `{name}`: {response}"
                ))
            })
        };
        let ra_hours = coord("ra")?;
        let dec_deg = coord("dec")?;

        let ra =
            RaDegrees::from_hours(ra_hours).map_err(|e| ScopinatorError::Backend(e.to_string()))?;
        let dec = DecDegrees::new(dec_deg).map_err(|e| ScopinatorError::Backend(e.to_string()))?;

        // The telescope reports the equinox of date, as it takes it.
        Ok(Coordinates::with_epoch(ra, dec, SEESTAR_EPOCH))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        // Name the target after what was asked for; send what the
        // telescope understands.
        let name = format!(
            "RA {:.4} Dec {:.4}",
            coords.ra.as_hours(),
            coords.dec.as_degrees()
        );
        let of_date = coords.to_epoch(SEESTAR_EPOCH, chrono::Utc::now());
        let (ra_hours, dec_deg) = (of_date.ra.as_hours(), of_date.dec.as_degrees());
        self.client
            .goto(&name, ra_hours, dec_deg)
            .await?
            .into_result()
            .map_err(|e| scopinator_seestar::SeestarError::CommandFailed {
                code: e.code,
                message: e.message,
            })?;
        self.goto.accepted();
        Ok(())
    }

    async fn abort_slew(&self) -> Result<(), ScopinatorError> {
        use scopinator_seestar::command::params::{StopStage, StopViewParams};
        // Cancel the goto. On its own this stops the slew but leaves the
        // scope in star view taking exposures (firmware 6.70 and 9.75).
        let cancel_goto = Command::IscopeStopView(Some(StopViewParams {
            stage: StopStage::AutoGoto,
        }));
        self.client.send_and_validate(cancel_goto).await?;
        // Then leave the view, so that abort returns the scope to idle.
        self.client
            .send_and_validate(Command::IscopeStopView(None))
            .await?;
        // The cancelled goto shows as failed in the view state; it was
        // asked for, so it is not reported as a failed slew.
        self.goto.clear();
        Ok(())
    }

    async fn is_slewing(&self) -> Result<bool, ScopinatorError> {
        match self.goto_stage().await? {
            GotoStage::Running => {
                self.goto.running();
                return Ok(true);
            }
            GotoStage::Failed(failure) => {
                // Report the failure of a goto sent through this backend
                // once; the stage stays in the view state after that.
                if self.goto.is_pending() {
                    self.goto.clear();
                    return Err(ScopinatorError::Backend(format!(
                        "goto failed at {failure}"
                    )));
                }
            }
            GotoStage::None => {}
        }
        if self.mount_state().await?.moving {
            self.goto.running();
            return Ok(true);
        }
        // Accepted, and the telescope has not started on it yet. A goto
        // stage that is already over belongs to an earlier goto.
        if self.goto.awaiting_start(GOTO_START_GRACE) {
            return Ok(true);
        }
        self.goto.clear();
        Ok(false)
    }

    async fn is_parked(&self) -> Result<bool, ScopinatorError> {
        Ok(self.mount_state().await?.parked)
    }

    async fn park(&self) -> Result<(), ScopinatorError> {
        self.client.send_and_validate(Command::ScopePark).await?;
        Ok(())
    }

    async fn set_tracking(&self, enabled: bool) -> Result<(), ScopinatorError> {
        self.client
            .send_and_validate(Command::ScopeSetTrackState(enabled))
            .await?;
        Ok(())
    }

    async fn is_tracking(&self) -> Result<bool, ScopinatorError> {
        Ok(self.mount_state().await?.tracking)
    }

    fn capabilities(&self) -> MountCapabilities {
        MountCapabilities {
            can_slew: true,
            can_sync: true,
            can_park: true,
            can_track: true,
            can_move_axis: true,
            equatorial_system: SEESTAR_EPOCH,
        }
    }

    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError> {
        use scopinator_types::SlewState;

        let coords = self.get_coordinates().await.ok();
        let state = self.mount_state().await?;
        // A failed goto is reported by `is_slewing`; here it only means the
        // mount is not slewing.
        let slewing = state.moving || matches!(self.goto_stage().await, Ok(GotoStage::Running));
        Ok(MountStatus {
            slew_state: if state.parked {
                SlewState::Parked
            } else if slewing {
                SlewState::Slewing
            } else if state.tracking {
                SlewState::Tracking
            } else {
                SlewState::Idle
            },
            tracking_rate: None,
            coordinates: coords,
            is_tracking: state.tracking,
        })
    }
}
