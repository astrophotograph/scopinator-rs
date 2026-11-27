use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::params::{StartViewParams, ViewMode};
use scopinator_seestar::command::{Command, ImagingCommand};
use scopinator_types::{
    BayerPattern, DeviceId, DeviceInfo, DeviceKind, ExposureSettings, ImageData, SensorType,
};

use crate::device::capabilities::CameraCapabilities;
use crate::device::status::{CameraStatus, DeviceStatus};
use crate::device::traits::{Camera, Device};
use crate::error::ScopinatorError;

use super::wide_camera::FRAME_TIMEOUT;

/// How long `get_image` waits for a frame of a stream it takes to be
/// running before it asks for the stream again.
const STREAM_RESTART: Duration = Duration::from_secs(3);

/// A Seestar's main camera: a live view of the sky, delivered as a stream of
/// frames. Stacking those frames is the telescope's
/// [`LiveStacker`](crate::device::LiveStacker).
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
    /// Start the star view, if none is running, and its frame stream.
    ///
    /// A Seestar's main camera has no single exposures: it shows a live
    /// view, and `get_image` returns frames of it. Of the settings only the
    /// gain is applied; the telescope chooses the exposure of its view.
    /// Stacking is [`LiveStacker`](crate::device::LiveStacker), not this.
    ///
    /// A view that is already running (after a goto, say) is left as it is,
    /// target included. Starting one does not move the mount.
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError> {
        super::check_exposure(settings)?;
        if !self.is_exposing().await? {
            let view = Command::IscopeStartView(StartViewParams {
                mode: Some(ViewMode::Star),
                target_name: None,
                target_ra_dec: None,
                target_type: None,
                lp_filter: None,
            });
            self.client.send_and_validate(view).await?;
        }
        if let Some(gain) = settings.gain {
            self.client
                .send_and_validate(Command::SetControlValue("gain".into(), gain))
                .await?;
        }
        self.client.begin_streaming().await?;
        Ok(())
    }

    /// Stop the frame stream and leave the view. This ends a stack running
    /// in that view as well.
    async fn abort_exposure(&self) -> Result<(), ScopinatorError> {
        self.client
            .send_imaging(ImagingCommand::StopStreaming)
            .await?;
        self.client
            .send_and_validate(Command::IscopeStopView(None))
            .await?;
        Ok(())
    }

    /// The camera streams: there is a frame to fetch whenever a view is
    /// running.
    async fn image_ready(&self) -> Result<bool, ScopinatorError> {
        self.is_exposing().await
    }

    /// The next frame of the live view, as the sensor delivered it. Fails
    /// with a timeout if no view is running.
    ///
    /// If no frame comes within a few seconds the stream is asked for
    /// again: a view started by a goto, or by another program, has none
    /// until someone asks.
    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        let mut frames = self.client.subscribe_frames();
        let start = tokio::time::Instant::now();
        let mut asked = false;
        let frame = loop {
            let deadline = if asked {
                start + FRAME_TIMEOUT
            } else {
                start + STREAM_RESTART
            };
            match tokio::time::timeout_at(deadline, frames.recv()).await {
                Err(_) if !asked => {
                    asked = true;
                    self.client.begin_streaming().await?;
                }
                Err(_) => return Err(ScopinatorError::Timeout),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(ScopinatorError::Backend("frame channel closed".into()));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Ok(frame)) if frame.header.is_image() => break frame,
                Ok(Ok(_)) => continue,
            }
        };
        // The header does not name the Bayer pattern; the device state does
        // (`camera.debayer_pattern`, "GR" on the S50 and S30). GRBG is the
        // fallback until that is known.
        let pattern = super::bayer_pattern(self.client.device_info().debayer_pattern.as_deref())
            .unwrap_or(BayerPattern::Grbg);
        super::image_from_frame(&frame, pattern)
    }

    /// True while a view of the main camera is running.
    async fn is_exposing(&self) -> Result<bool, ScopinatorError> {
        let response = self.client.send_and_validate(Command::GetViewState).await?;
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
            pixel_size_x_um: info.pixel_size_um,
            pixel_size_y_um: info.pixel_size_um,
            bit_depth: Some(12),
            sensor_type: super::bayer_pattern(info.debayer_pattern.as_deref())
                .map(SensorType::Bayer),
            ..CameraCapabilities::default()
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
            ..CameraStatus::default()
        })
    }
}
