use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::Command;
use scopinator_seestar::command::params::{StartViewParams, ViewMode};
use scopinator_types::{
    BayerPattern, DeviceId, DeviceInfo, DeviceKind, ExposureSettings, ImageData,
};

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
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info =
            DeviceInfo::new(self.device_id.clone(), DeviceKind::Camera, "Seestar camera");
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

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

    /// True once the running stack has at least one frame in it: the image
    /// `get_image` returns is the stack so far.
    async fn image_ready(&self) -> Result<bool, ScopinatorError> {
        let view = self.client.send_and_validate(Command::GetViewState).await?;
        // Stages nest by name in the view state; no `Stack` stage means
        // nothing is being stacked.
        Ok(view
            .pointer("/View/Stack/stacked_frame")
            .and_then(|frames| frames.as_i64())
            .is_some_and(|frames| frames > 0))
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
        // The header does not name the pattern; the device state does
        // (`camera.debayer_pattern`, "GR" on the S50 and S30). GRBG is the
        // fallback until that is known.
        let bayer = !is_color && header.can_debayer != 0;
        let pattern = match self.client.device_info().debayer_pattern.as_deref() {
            Some("RG") => BayerPattern::Rggb,
            Some("BG") => BayerPattern::Bggr,
            Some("GB") => BayerPattern::Gbrg,
            _ => BayerPattern::Grbg,
        };
        Ok(ImageData {
            width: header.width as u32,
            height: header.height as u32,
            data,
            bit_depth,
            is_color: is_color || bayer,
            bayer_pattern: bayer.then_some(pattern),
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
        // Sensor size and pixel size are what this device reported in
        // `get_device_state`, not constants for one model; they are `None`
        // until the device has been heard from.
        let info = self.client.device_info();
        CameraCapabilities {
            can_expose: true,
            can_abort_exposure: true,
            can_stream: true,
            max_width: info.chip_size.map(|(width, _)| width),
            max_height: info.chip_size.map(|(_, height)| height),
            pixel_size_um: info.pixel_size_um,
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
