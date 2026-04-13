use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::GotoTargetParams;
use scopinator_seestar::SeestarClient;
use scopinator_types::{Coordinates, DeviceId, RaDegrees, DecDegrees};

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

        // Response result contains ra (hours) and dec (degrees)
        let ra_hours = response
            .get("ra")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let dec_deg = response
            .get("dec")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);

        let ra = RaDegrees::from_hours(ra_hours)
            .map_err(|e| ScopinatorError::Backend(e.to_string()))?;
        let dec = DecDegrees::new(dec_deg)
            .map_err(|e| ScopinatorError::Backend(e.to_string()))?;

        Ok(Coordinates::new(ra, dec))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        let cmd = Command::GotoTarget(GotoTargetParams {
            target_name: format!(
                "RA {:.4} Dec {:.4}",
                coords.ra.as_hours(),
                coords.dec.as_degrees()
            ),
            is_j2000: true,
            ra: coords.ra.as_degrees(),
            dec: coords.dec.as_degrees(),
        });
        self.client.send_and_validate(cmd).await?;
        Ok(())
    }

    async fn abort_slew(&self) -> Result<(), ScopinatorError> {
        // Seestar uses iscope_stop_view to abort
        use scopinator_seestar::command::params::{StopStage, StopViewParams};
        let cmd = Command::IscopeStopView(StopViewParams {
            stage: StopStage::AutoGoto,
        });
        self.client.send_and_validate(cmd).await?;
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
