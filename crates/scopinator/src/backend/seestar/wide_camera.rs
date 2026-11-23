use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_seestar::command::params::{StartViewParams, ViewMode};
use scopinator_seestar::command::{Command, ImagingCommand};
use scopinator_types::{
    BayerPattern, DeviceId, DeviceInfo, DeviceKind, ExposureSettings, ImageData,
};

use crate::device::capabilities::CameraCapabilities;
use crate::device::status::{CameraStatus, DeviceStatus};
use crate::device::traits::{Camera, Device};
use crate::error::ScopinatorError;

/// How long `get_image` waits for the next stream frame. The wide camera
/// delivered about one frame per second in star mode on an S30.
const FRAME_TIMEOUT: Duration = Duration::from_secs(15);

/// The wide camera of a two-camera Seestar, obtained from
/// [`SeestarBackend::second_camera`](super::SeestarBackend::second_camera).
///
/// It is a streaming camera: an exposure is a running view whose frames are
/// pushed over the wide image port, and `get_image` returns the next one.
/// Starting it does not move the mount. Stills, stacking and capture from
/// both cameras at once exist on Pro models and are not implemented here.
pub struct SeestarWideCamera {
    /// Connection to the wide camera's own process (ports 4702 and 4804).
    client: Arc<SeestarClient>,
    /// The main connection, which knows the device state.
    main: Arc<SeestarClient>,
    device_id: DeviceId,
}

impl SeestarWideCamera {
    pub(crate) fn new(
        client: Arc<SeestarClient>,
        main: Arc<SeestarClient>,
        device_id: DeviceId,
    ) -> Self {
        Self {
            client,
            main,
            device_id,
        }
    }

    /// The wide camera's own client, for anything this adapter does not wrap.
    pub fn client(&self) -> &Arc<SeestarClient> {
        &self.client
    }

    async fn ensure_connected(&self) -> Result<(), ScopinatorError> {
        self.client
            .wait_for_connection(Duration::from_secs(10))
            .await?;
        Ok(())
    }
}

#[async_trait]
impl Device for SeestarWideCamera {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.main.device_info();
        let mut info = DeviceInfo::new(
            self.device_id.clone(),
            DeviceKind::Camera,
            "Seestar wide camera",
        );
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.ensure_connected().await
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
            description: Some("Seestar wide camera".into()),
        })
    }
}

#[async_trait]
impl Camera for SeestarWideCamera {
    /// Start the wide view and its frame stream. Exposure settings are not
    /// applied: the wide camera runs its own auto exposure.
    async fn start_exposure(&self, _settings: &ExposureSettings) -> Result<(), ScopinatorError> {
        self.ensure_connected().await?;
        let view = Command::IscopeStartView(StartViewParams {
            mode: Some(ViewMode::Star),
            target_name: None,
            target_ra_dec: None,
            target_type: None,
            lp_filter: None,
        });
        self.client.send_and_validate(view).await?;
        self.client.begin_streaming().await?;
        Ok(())
    }

    async fn abort_exposure(&self) -> Result<(), ScopinatorError> {
        self.client
            .send_imaging(ImagingCommand::StopStreaming)
            .await?;
        self.client
            .send_and_validate(Command::IscopeStopView(None))
            .await?;
        Ok(())
    }

    /// The next frame of the stream. Fails with a timeout if no view is
    /// running.
    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        let mut frames = self.client.subscribe_frames();
        let deadline = tokio::time::Instant::now() + FRAME_TIMEOUT;
        let frame = loop {
            match tokio::time::timeout_at(deadline, frames.recv()).await {
                Err(_) => return Err(ScopinatorError::Timeout),
                Ok(Err(tokio::sync::broadcast::error::RecvError::Closed)) => {
                    return Err(ScopinatorError::Backend("frame channel closed".into()));
                }
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Ok(frame)) if frame.header.is_image() => break frame,
                Ok(Ok(_)) => continue,
            }
        };

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
        let bayer = !is_color && header.can_debayer != 0;
        let info = self.main.device_info();
        let pattern = match info
            .second_camera
            .as_ref()
            .and_then(|s| s.debayer_pattern.as_deref())
        {
            Some("GR") => BayerPattern::Grbg,
            Some("BG") => BayerPattern::Bggr,
            Some("GB") => BayerPattern::Gbrg,
            // The S30's wide camera reports "RG".
            _ => BayerPattern::Rggb,
        };
        Ok(ImageData {
            width: header.width as u32,
            height: header.height as u32,
            data: frame.image_bytes()?,
            bit_depth,
            is_color: is_color || bayer,
            bayer_pattern: bayer.then_some(pattern),
            frame_kind: frame.kind,
        })
    }

    async fn is_exposing(&self) -> Result<bool, ScopinatorError> {
        let response = self.client.send_and_validate(Command::GetViewState).await?;
        let state = response
            .get("SecondView")
            .and_then(|v| v.get("state"))
            .and_then(|v| v.as_str())
            .unwrap_or("idle");
        Ok(state == "working" || state == "start")
    }

    fn capabilities(&self) -> CameraCapabilities {
        let sensor = self.main.device_info().second_camera.unwrap_or_default();
        CameraCapabilities {
            can_expose: true,
            can_abort_exposure: true,
            can_stream: true,
            max_width: sensor.chip_size.map(|(width, _)| width),
            max_height: sensor.chip_size.map(|(_, height)| height),
            pixel_size_um: sensor.pixel_size_um,
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
