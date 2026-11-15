use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_types::{Coordinates, DecDegrees, DeviceId, RaDegrees};

use crate::device::capabilities::MountCapabilities;
use crate::device::status::{DeviceStatus, MountStatus};
use crate::device::traits::{Device, Mount};
use crate::error::ScopinatorError;

/// Seestar mount adapter.
pub struct SeestarMount {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
}

impl SeestarMount {
    pub(crate) fn new(client: Arc<SeestarClient>, device_id: DeviceId) -> Self {
        Self { client, device_id }
    }
}

#[async_trait]
impl Device for SeestarMount {
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

        Ok(Coordinates::new(ra, dec))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        let (ra_hours, dec_deg) = (coords.ra.as_hours(), coords.dec.as_degrees());
        let name = format!("RA {ra_hours:.4} Dec {dec_deg:.4}");
        self.client
            .goto(&name, ra_hours, dec_deg)
            .await?
            .into_result()
            .map_err(|e| scopinator_seestar::SeestarError::CommandFailed {
                code: e.code,
                message: e.message,
            })?;
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
        Ok(())
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
        let response = self
            .client
            .send_and_validate(Command::GetDeviceState)
            .await?;

        let tracking = response
            .get("mount")
            .and_then(|m| m.get("tracking"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        Ok(tracking)
    }

    fn capabilities(&self) -> MountCapabilities {
        MountCapabilities {
            can_slew: true,
            can_sync: true,
            can_park: true,
            can_track: true,
            can_move_axis: true,
        }
    }

    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError> {
        let coords = self.get_coordinates().await.ok();
        let tracking = self.is_tracking().await.unwrap_or(false);

        use scopinator_types::SlewState;
        Ok(MountStatus {
            slew_state: if tracking {
                SlewState::Tracking
            } else {
                SlewState::Idle
            },
            tracking_rate: None,
            coordinates: coords,
            is_tracking: tracking,
        })
    }
}
