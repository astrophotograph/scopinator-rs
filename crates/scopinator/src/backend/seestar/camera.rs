use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{StartViewParams, ViewMode};
use scopinator_types::{BayerPattern, DeviceId, ExposureSettings, ImageData};

use crate::device::capabilities::CameraCapabilities;
use crate::device::status::{CameraStatus, DeviceStatus};
use crate::device::traits::{Camera, Device};
use crate::error::ScopinatorError;

/// Seestar camera adapter.
pub struct SeestarCamera {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
}

impl SeestarCamera {
    pub(crate) fn new(client: Arc<SeestarClient>, device_id: DeviceId) -> Self {
        Self { client, device_id }
    }
}

#[async_trait]
impl Device for SeestarCamera {
    async fn connect(&self) -> Result<(), ScopinatorError> {
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
            description: Some("Seestar camera".into()),
        })
    }
}

#[async_trait]
impl Camera for SeestarCamera {
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError> {
        // Seestar uses iscope_start_view to begin observing,
        // then iscope_start_stack for stacking exposures.
        let cmd = Command::IscopeStartView(StartViewParams {
            mode: Some(ViewMode::Star),
            target_name: None,
            target_ra_dec: None,
            target_type: None,
            lp_filter: None,
        });
        self.client.send_and_validate(cmd).await?;

        // Set gain if specified
        if let Some(gain) = settings.gain {
            self.client
                .send_and_validate(Command::SetControlValue("gain".into(), gain))
                .await?;
        }

        // Start stacking
        self.client
            .send_and_validate(Command::IscopeStartStack(None))
            .await?;

        Ok(())
    }

    async fn abort_exposure(&self) -> Result<(), ScopinatorError> {
        use scopinator_seestar::command::params::{StopStage, StopViewParams};
        let cmd = Command::IscopeStopView(StopViewParams {
            stage: StopStage::Stack,
        });
        self.client.send_and_validate(cmd).await?;
        Ok(())
    }

    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        // Get the latest stacked image via the imaging port
        let mut rx = self.client.subscribe_frames();

        // Request the stacked image
        self.client.send_command(Command::GetStackedImage).await?;

        // Wait for the next frame
        match tokio::time::timeout(std::time::Duration::from_secs(10), rx.recv()).await {
            Ok(Ok(frame)) => Ok(ImageData {
                width: frame.header.width as u32,
                height: frame.header.height as u32,
                data: frame.data.clone(),
                bit_depth: 16,
                is_color: true,
                bayer_pattern: Some(BayerPattern::Grbg),
                frame_kind: frame.kind,
            }),
            Ok(Err(_)) => Err(ScopinatorError::Backend("frame channel closed".into())),
            Err(_) => Err(ScopinatorError::Timeout),
        }
    }

    async fn is_exposing(&self) -> Result<bool, ScopinatorError> {
        let response = self.client.send_and_validate(Command::GetViewState).await?;

        // Check if the view state indicates active stacking
        let state = response
            .get("View")
            .and_then(|v| v.get("state"))
            .and_then(|v| v.as_str())
            .unwrap_or("idle");

        Ok(state == "working" || state == "start")
    }

    fn capabilities(&self) -> CameraCapabilities {
        CameraCapabilities {
            can_expose: true,
            can_abort_exposure: true,
            can_stream: true,
            // Seestar S50 specs
            max_width: Some(1920),
            max_height: Some(1080),
            pixel_size_um: Some(3.0),
            bit_depth: Some(12),
        }
    }

    async fn get_camera_status(&self) -> Result<CameraStatus, ScopinatorError> {
        use scopinator_types::CameraState;

        let is_exposing = self.is_exposing().await.unwrap_or(false);

        Ok(CameraStatus {
            state: if is_exposing {
                CameraState::Exposing
            } else {
                CameraState::Idle
            },
            temperature: None,
            gain: None,
            stacked_frames: None,
            dropped_frames: None,
        })
    }
}
