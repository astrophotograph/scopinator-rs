use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, RwLock};

use async_trait::async_trait;
use scopinator_alpaca::AlpacaCamera;
use scopinator_alpaca::camera::{CameraState as AlpacaState, SensorType as AlpacaSensor};
use scopinator_types::{
    BayerPattern, CameraState, DeviceId, DeviceInfo, DeviceKind, ExposureSettings, FrameKind,
    FrameMetadata, ImageData, PixelFormat, SensorType,
};

use super::{neutral, optional};
use crate::device::capabilities::CameraCapabilities;
use crate::device::status::{CameraStatus, DeviceStatus};
use crate::device::traits::{Camera, Device};
use crate::error::ScopinatorError;

/// An Alpaca `camera` device as a neutral camera: single exposures with a
/// duration, gain, offset, binning and subframe.
pub struct AlpacaCameraDevice {
    camera: AlpacaCamera,
    id: DeviceId,
    connected: AtomicBool,
    /// Read at connect.
    profile: RwLock<Profile>,
    /// What the last exposure was started with, for the image's metadata.
    last_exposure: Mutex<Option<FrameMetadata>>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    capabilities: CameraCapabilities,
    name: Option<String>,
    driver: Option<String>,
}

impl AlpacaCameraDevice {
    /// The camera `camera` on the server whose backend id is `backend_id`.
    /// Nothing is sent until [`connect`](Device::connect).
    pub fn new(camera: AlpacaCamera, backend_id: &DeviceId) -> Self {
        let id = DeviceId::new(format!("{backend_id}:camera:{}", camera.device_number()));
        Self {
            camera,
            id,
            connected: AtomicBool::new(false),
            profile: RwLock::new(Profile::default()),
            last_exposure: Mutex::new(None),
        }
    }

    /// The typed Alpaca client, for what the neutral trait does not cover.
    pub fn camera(&self) -> &AlpacaCamera {
        &self.camera
    }

    fn profile(&self) -> Profile {
        self.profile
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn read_profile(&self) -> Result<Profile, ScopinatorError> {
        let camera = &self.camera;
        let range = |low: Option<i32>, high: Option<i32>| low.zip(high);

        let sensor_type = match optional(camera.sensor_type().await)? {
            Some(AlpacaSensor::Monochrome) => Some(SensorType::Mono),
            Some(AlpacaSensor::Color) => Some(SensorType::Color),
            Some(AlpacaSensor::Rggb) => {
                let x = optional(camera.bayer_offset_x().await)?.unwrap_or(0);
                let y = optional(camera.bayer_offset_y().await)?.unwrap_or(0);
                Some(SensorType::Bayer(rggb_pattern(x, y)))
            }
            // CMYG and LRGB mosaics have no neutral description yet.
            Some(_) | None => None,
        };
        let max_bin = optional(camera.max_bin_x().await)?
            .zip(optional(camera.max_bin_y().await)?)
            .map(|(x, y)| x.min(y));
        let exposure_range_s =
            optional(camera.exposure_min().await)?.zip(optional(camera.exposure_max().await)?);

        let capabilities = CameraCapabilities {
            can_expose: true,
            can_abort_exposure: optional(camera.can_abort_exposure().await)?.unwrap_or(false),
            max_width: optional(camera.camera_x_size().await)?,
            max_height: optional(camera.camera_y_size().await)?,
            pixel_size_x_um: optional(camera.pixel_size_x().await)?,
            pixel_size_y_um: optional(camera.pixel_size_y().await)?,
            bit_depth: optional(camera.max_adu().await)?.and_then(bits_for_max_adu),
            sensor_type,
            gain_range: range(
                optional(camera.gain_min().await)?,
                optional(camera.gain_max().await)?,
            ),
            offset_range: range(
                optional(camera.offset_min().await)?,
                optional(camera.offset_max().await)?,
            ),
            exposure_range_s,
            max_bin,
            // Every Alpaca camera has StartX, StartY, NumX and NumY.
            can_set_roi: true,
            has_cooler: optional(camera.cooler_on().await)?.is_some(),
            can_set_temperature: optional(camera.can_set_ccd_temperature().await)?.unwrap_or(false),
            full_well_e: optional(camera.full_well_capacity().await)?,
            e_per_adu: optional(camera.electrons_per_adu().await)?,
            ..CameraCapabilities::default()
        };

        Ok(Profile {
            capabilities,
            name: optional(camera.name().await)?,
            driver: optional(camera.driver_info().await)?,
        })
    }
}

/// The mosaic of an RGGB sensor whose first pixel is offset by (`x`, `y`)
/// into the pattern: shifting right swaps the columns, shifting down swaps
/// the rows.
fn rggb_pattern(offset_x: u32, offset_y: u32) -> BayerPattern {
    match (offset_x % 2, offset_y % 2) {
        (0, 0) => BayerPattern::Rggb,
        (1, 0) => BayerPattern::Grbg,
        (0, 1) => BayerPattern::Gbrg,
        _ => BayerPattern::Bggr,
    }
}

/// Bits needed to hold values up to `max_adu` (4095 is 12 bits).
fn bits_for_max_adu(max_adu: i64) -> Option<u32> {
    (max_adu > 0).then(|| u64::BITS - (max_adu as u64).leading_zeros())
}

/// Check `value` against a range the camera reported, if it reported one.
fn in_range<T: PartialOrd + std::fmt::Display>(
    what: &str,
    value: T,
    range: Option<(T, T)>,
) -> Result<(), ScopinatorError> {
    match range {
        Some((low, high)) if value < low || value > high => Err(ScopinatorError::OutOfRange(
            format!("{what} {value} (the camera takes {low} to {high})"),
        )),
        _ => Ok(()),
    }
}

#[async_trait]
impl Device for AlpacaCameraDevice {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let profile = self.profile();
        let mut info = DeviceInfo::new(
            self.id.clone(),
            DeviceKind::Camera,
            profile.name.unwrap_or_else(|| "Alpaca camera".to_string()),
        );
        info.driver = profile.driver;
        info
    }

    /// Set the device's `Connected` property and read what it can do.
    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.camera.set_connected(true).await.map_err(neutral)?;
        let profile = self.read_profile().await?;
        *self
            .profile
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        self.connected.store(false, Ordering::SeqCst);
        self.camera.set_connected(false).await.map_err(neutral)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        let profile = self.profile();
        Ok(DeviceStatus {
            connected: self.camera.connected().await.map_err(neutral)?,
            name: profile.name,
            description: profile.driver,
        })
    }
}

#[async_trait]
impl Camera for AlpacaCameraDevice {
    /// Apply the settings (binning, subframe, gain, offset), then start the
    /// exposure. Everything is checked against what the camera reported
    /// before anything is sent.
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError> {
        let caps = self.profile().capabilities;
        if settings.readout_mode.is_some() {
            return Err(ScopinatorError::NotSupported(
                "choosing a read-out mode".into(),
            ));
        }
        if settings.bin_x == 0 || settings.bin_y == 0 {
            return Err(ScopinatorError::InvalidArgument("binning of zero".into()));
        }
        in_range(
            "binning",
            settings.bin_x.max(settings.bin_y),
            caps.max_bin.map(|max| (1, max)),
        )?;
        in_range("exposure", settings.duration_seconds, caps.exposure_range_s)?;
        if let Some(gain) = settings.gain {
            in_range("gain", gain, caps.gain_range)?;
        }
        if let Some(offset) = settings.offset {
            in_range("offset", offset, caps.offset_range)?;
        }
        let sensor = caps.max_width.zip(caps.max_height);
        // The subframe is given in sensor pixels; Alpaca takes it in binned
        // pixels. With no subframe, read the whole sensor.
        let (start_x, start_y, width, height) = match (settings.roi, sensor) {
            (Some(roi), Some((sensor_width, sensor_height))) => {
                if !roi.fits(sensor_width, sensor_height) {
                    return Err(ScopinatorError::OutOfRange(format!(
                        "a {}x{} region at ({}, {}) on a {sensor_width}x{sensor_height} sensor",
                        roi.width(),
                        roi.height(),
                        roi.x(),
                        roi.y()
                    )));
                }
                (roi.x(), roi.y(), roi.width(), roi.height())
            }
            (Some(roi), None) => (roi.x(), roi.y(), roi.width(), roi.height()),
            (None, Some((sensor_width, sensor_height))) => (0, 0, sensor_width, sensor_height),
            (None, None) => {
                return Err(ScopinatorError::InvalidState(
                    "the camera did not report its sensor size".into(),
                ));
            }
        };

        let camera = &self.camera;
        camera.set_bin_x(settings.bin_x).await.map_err(neutral)?;
        camera.set_bin_y(settings.bin_y).await.map_err(neutral)?;
        camera
            .set_start_x(start_x / settings.bin_x)
            .await
            .map_err(neutral)?;
        camera
            .set_start_y(start_y / settings.bin_y)
            .await
            .map_err(neutral)?;
        camera
            .set_num_x(width / settings.bin_x)
            .await
            .map_err(neutral)?;
        camera
            .set_num_y(height / settings.bin_y)
            .await
            .map_err(neutral)?;
        if let Some(gain) = settings.gain {
            camera.set_gain(gain).await.map_err(neutral)?;
        }
        if let Some(offset) = settings.offset {
            camera.set_offset(offset).await.map_err(neutral)?;
        }

        let mut metadata = FrameMetadata::default();
        metadata.timestamp = Some(chrono::Utc::now());
        metadata.exposure_seconds = Some(settings.duration_seconds);
        metadata.gain = settings.gain;
        metadata.bin_x = settings.bin_x;
        metadata.bin_y = settings.bin_y;
        camera
            .start_exposure(settings.duration_seconds, settings.frame_type.needs_light())
            .await
            .map_err(neutral)?;
        *self
            .last_exposure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(metadata);
        Ok(())
    }

    async fn abort_exposure(&self) -> Result<(), ScopinatorError> {
        self.camera.abort_exposure().await.map_err(neutral)
    }

    async fn image_ready(&self) -> Result<bool, ScopinatorError> {
        self.camera.image_ready().await.map_err(neutral)
    }

    /// Download the image of the last exposure as 16-bit samples.
    /// `InvalidState` if there is none yet.
    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        if !self.camera.image_ready().await.map_err(neutral)? {
            return Err(ScopinatorError::InvalidState(
                "the camera has no image ready".into(),
            ));
        }
        let image = self.camera.image_bytes().await.map_err(neutral)?;
        let format = match (image.planes, self.profile().capabilities.sensor_type) {
            (1, Some(SensorType::Bayer(pattern))) => PixelFormat::Bayer16(pattern),
            (1, _) => PixelFormat::Mono16,
            (3, _) => PixelFormat::Rgb16,
            (planes, _) => {
                return Err(ScopinatorError::Backend(format!(
                    "an image with {planes} planes"
                )));
            }
        };
        let samples = image.to_row_major_u16().map_err(neutral)?;
        let bytes: Vec<u8> = samples.iter().flat_map(|s| s.to_le_bytes()).collect();
        let mut data = ImageData::new(
            image.width,
            image.height,
            bytes.into(),
            format,
            FrameKind::Preview,
        );
        let last = self
            .last_exposure
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(mut metadata) = last {
            metadata.sensor_temperature_c = optional(self.camera.ccd_temperature().await)?;
            data.metadata = metadata;
        }
        Ok(data)
    }

    async fn is_exposing(&self) -> Result<bool, ScopinatorError> {
        Ok(!matches!(
            self.camera.camera_state().await.map_err(neutral)?,
            AlpacaState::Idle | AlpacaState::Error
        ))
    }

    async fn set_cooler(&self, on: bool) -> Result<(), ScopinatorError> {
        self.camera.set_cooler_on(on).await.map_err(neutral)
    }

    async fn set_target_temperature(&self, celsius: f64) -> Result<(), ScopinatorError> {
        self.camera
            .set_ccd_temperature(celsius)
            .await
            .map_err(neutral)
    }

    async fn cooler_power(&self) -> Result<f64, ScopinatorError> {
        self.camera.cooler_power().await.map_err(neutral)
    }

    fn capabilities(&self) -> CameraCapabilities {
        self.profile().capabilities
    }

    async fn get_camera_status(&self) -> Result<CameraStatus, ScopinatorError> {
        let camera = &self.camera;
        let state = match camera.camera_state().await.map_err(neutral)? {
            AlpacaState::Idle => CameraState::Idle,
            AlpacaState::Waiting => CameraState::Waiting,
            AlpacaState::Exposing => CameraState::Exposing,
            AlpacaState::Reading => CameraState::Reading,
            AlpacaState::Download => CameraState::Downloading,
            AlpacaState::Error => CameraState::Error,
        };
        Ok(CameraStatus {
            state,
            temperature: optional(camera.ccd_temperature().await)?,
            cooler_on: optional(camera.cooler_on().await)?,
            cooler_power: optional(camera.cooler_power().await)?,
            target_temperature: optional(camera.set_ccd_temperature_target().await)?,
            gain: optional(camera.gain().await)?,
            ..CameraStatus::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// RGGB is
    ///
    /// ```text
    ///   R G
    ///   G B
    /// ```
    ///
    /// Starting one column in, the first row reads G R; one row down, G B.
    #[test]
    fn bayer_offsets_shift_the_rggb_mosaic() {
        assert_eq!(rggb_pattern(0, 0), BayerPattern::Rggb);
        assert_eq!(rggb_pattern(1, 0), BayerPattern::Grbg);
        assert_eq!(rggb_pattern(0, 1), BayerPattern::Gbrg);
        assert_eq!(rggb_pattern(1, 1), BayerPattern::Bggr);
    }

    #[test]
    fn bit_depth_is_the_bits_needed_for_max_adu() {
        assert_eq!(bits_for_max_adu(255), Some(8));
        assert_eq!(bits_for_max_adu(4095), Some(12));
        assert_eq!(bits_for_max_adu(16383), Some(14));
        assert_eq!(bits_for_max_adu(65535), Some(16));
        assert_eq!(bits_for_max_adu(0), None);
        assert_eq!(bits_for_max_adu(-1), None);
    }

    #[test]
    fn a_value_outside_a_reported_range_is_out_of_range() {
        assert!(in_range("gain", 50, Some((0, 100))).is_ok());
        assert!(in_range("gain", 0, Some((0, 100))).is_ok());
        assert!(in_range("gain", 100, Some((0, 100))).is_ok());
        assert!(matches!(
            in_range("gain", 101, Some((0, 100))),
            Err(ScopinatorError::OutOfRange(_))
        ));
        assert!(in_range("gain", -1, Some((0, 100))).is_err());
        // No reported range, nothing to check against.
        assert!(in_range("gain", 9999, None).is_ok());
    }
}
