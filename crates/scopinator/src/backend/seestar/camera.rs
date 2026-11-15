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
        let cmd = Command::IscopeStopView(Some(StopViewParams {
            stage: StopStage::Stack,
        }));
        self.client.send_and_validate(cmd).await?;
        Ok(())
    }

    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        use scopinator_seestar::protocol::frame::PayloadKind;

        // The stacked image is requested on the imaging port and matched to
        // this request, so a stream frame or heartbeat echo is never returned.
        let frame = self.client.get_stacked_image().await?;
        if frame.payload_kind() == PayloadKind::Jpeg {
            return Err(ScopinatorError::Backend(
                "the telescope sent a JPEG (image_transfer_mode is Jpg); \
                 raw pixel data needs image_transfer_mode Raw"
                    .into(),
            ));
        }
        let data = frame.image_bytes()?;

        // Pixel layout comes from the frame header, not from constants.
        let header = &frame.header;
        let (bit_depth, is_color) = match header.bytes_per_pixel() {
            Some(1) => (8, false),
            Some(2) => (16, false),
            Some(3) => (8, true),
            Some(6) => (16, true),
            _ => {
                return Err(ScopinatorError::Backend(format!(
                    "unsupported image type {} in frame header",
                    header.img_type
                )));
            }
        };
        // One value per pixel with the debayer flag set is a Bayer mosaic.
        // The header does not name the pattern; the Seestar sensors are GRBG.
        let bayer = !is_color && header.can_debayer != 0;
        Ok(ImageData {
            width: header.width as u32,
            height: header.height as u32,
            data,
            bit_depth,
            is_color: is_color || bayer,
            bayer_pattern: bayer.then_some(BayerPattern::Grbg),
            frame_kind: frame.kind,
        })
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
