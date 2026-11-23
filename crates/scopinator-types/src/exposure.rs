use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// What an exposure is for, which decides whether the shutter opens and
/// how the frame is labelled.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum FrameType {
    /// A picture of the sky.
    #[default]
    Light,
    /// The same exposure with no light reaching the sensor.
    Dark,
    /// The shortest exposure with no light: the read-out offset alone.
    Bias,
    /// An evenly lit field, to measure vignetting and dust.
    Flat,
}

impl FrameType {
    /// True if light should reach the sensor (a light or flat frame).
    pub fn needs_light(self) -> bool {
        matches!(self, Self::Light | Self::Flat)
    }
}

/// A rectangle of the sensor to read out, in unbinned sensor pixels from the
/// top left corner.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(try_from = "RawRoi")]
pub struct Roi {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

#[derive(Deserialize)]
struct RawRoi {
    x: u32,
    y: u32,
    width: u32,
    height: u32,
}

impl TryFrom<RawRoi> for Roi {
    type Error = crate::error::TypeError;

    fn try_from(raw: RawRoi) -> Result<Self, Self::Error> {
        Self::new(raw.x, raw.y, raw.width, raw.height)
    }
}

impl Roi {
    /// A region `width` by `height` pixels with its top left corner at
    /// (`x`, `y`). It must not be empty.
    pub fn new(x: u32, y: u32, width: u32, height: u32) -> Result<Self, crate::error::TypeError> {
        if width == 0 || height == 0 {
            return Err(crate::error::TypeError::EmptyRoi { width, height });
        }
        Ok(Self {
            x,
            y,
            width,
            height,
        })
    }

    pub fn x(&self) -> u32 {
        self.x
    }

    pub fn y(&self) -> u32 {
        self.y
    }

    pub fn width(&self) -> u32 {
        self.width
    }

    pub fn height(&self) -> u32 {
        self.height
    }

    /// True if the whole region lies on a sensor of this size.
    pub fn fits(&self, sensor_width: u32, sensor_height: u32) -> bool {
        self.x
            .checked_add(self.width)
            .is_some_and(|right| right <= sensor_width)
            && self
                .y
                .checked_add(self.height)
                .is_some_and(|bottom| bottom <= sensor_height)
    }
}

/// Settings for a camera exposure.
///
/// Fields may be added; start from [`ExposureSettings::new`] or
/// [`Default`] and use the `with_` methods or set fields.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct ExposureSettings {
    pub duration_seconds: f64,
    /// `None` leaves the camera's gain as it is.
    pub gain: Option<i32>,
    /// `None` leaves the camera's offset as it is.
    pub offset: Option<i32>,
    pub bin_x: u32,
    pub bin_y: u32,
    #[serde(default)]
    pub frame_type: FrameType,
    /// The part of the sensor to read; `None` is all of it.
    #[serde(default)]
    pub roi: Option<Roi>,
    /// One of the camera's read-out modes, by name; `None` leaves it.
    #[serde(default)]
    pub readout_mode: Option<String>,
}

impl Default for ExposureSettings {
    /// A one-second light frame of the whole sensor, unbinned, with the
    /// camera's gain and offset left alone.
    fn default() -> Self {
        Self {
            duration_seconds: 1.0,
            gain: None,
            offset: None,
            bin_x: 1,
            bin_y: 1,
            frame_type: FrameType::Light,
            roi: None,
            readout_mode: None,
        }
    }
}

impl ExposureSettings {
    /// The defaults with this exposure time.
    pub fn new(duration_seconds: f64) -> Self {
        Self {
            duration_seconds,
            ..Self::default()
        }
    }

    pub fn with_gain(mut self, gain: i32) -> Self {
        self.gain = Some(gain);
        self
    }

    pub fn with_offset(mut self, offset: i32) -> Self {
        self.offset = Some(offset);
        self
    }

    /// The same binning on both axes.
    pub fn with_binning(mut self, bin: u32) -> Self {
        self.bin_x = bin;
        self.bin_y = bin;
        self
    }

    pub fn with_frame_type(mut self, frame_type: FrameType) -> Self {
        self.frame_type = frame_type;
        self
    }

    pub fn with_roi(mut self, roi: Roi) -> Self {
        self.roi = Some(roi);
        self
    }
}

/// What kind of sensor a camera has.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SensorType {
    /// No colour filters: every pixel sees all the light.
    Mono,
    /// A colour sensor that delivers finished colour pixels.
    Color,
    /// A colour sensor that delivers the raw mosaic.
    Bayer(BayerPattern),
}

/// Kind of frame received on the imaging connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FrameKind {
    /// Live view / preview image frame.
    Preview,
    /// Stacked result.
    Stack,
    /// A non-image frame on the imaging socket — e.g. a `test_connection`
    /// heartbeat echo or other status/control frame. Carries no image payload;
    /// consumers decoding images should skip these (see [`FrameKind::is_image`]).
    Status,
}

impl FrameKind {
    /// Whether this frame carries decodable image data ([`Preview`](Self::Preview)
    /// or [`Stack`](Self::Stack)), as opposed to a [`Status`](Self::Status) frame.
    pub fn is_image(self) -> bool {
        matches!(self, FrameKind::Preview | FrameKind::Stack)
    }
}

/// Bayer pattern for raw sensor data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BayerPattern {
    Grbg,
    Rggb,
    Bggr,
    Gbrg,
}

/// How the bytes of an image are laid out.
///
/// For every format but [`Jpeg`](Self::Jpeg), pixels run left to right in
/// rows from the top of the image down, with no padding, and 16-bit samples
/// are little-endian.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PixelFormat {
    /// One 8-bit sample per pixel.
    Mono8,
    /// One 16-bit sample per pixel.
    Mono16,
    /// One 8-bit sample per pixel behind a colour filter mosaic; needs
    /// debayering to become a colour image.
    Bayer8(BayerPattern),
    /// One 16-bit sample per pixel behind a colour filter mosaic.
    Bayer16(BayerPattern),
    /// Three 8-bit samples per pixel: red, green, blue.
    Rgb8,
    /// Three 16-bit samples per pixel: red, green, blue.
    Rgb16,
    /// A JPEG file. Its dimensions are in the file; `width` and `height` of
    /// the [`ImageData`] are what the camera said they are.
    Jpeg,
}

impl PixelFormat {
    /// Bytes one pixel takes, or `None` for a compressed format.
    pub fn bytes_per_pixel(self) -> Option<usize> {
        match self {
            Self::Mono8 | Self::Bayer8(_) => Some(1),
            Self::Mono16 | Self::Bayer16(_) => Some(2),
            Self::Rgb8 => Some(3),
            Self::Rgb16 => Some(6),
            Self::Jpeg => None,
        }
    }

    /// Bits per sample, or `None` for a compressed format. A 12-bit sensor
    /// delivering 16-bit samples is 16 here.
    pub fn bits_per_sample(self) -> Option<u32> {
        match self {
            Self::Mono8 | Self::Bayer8(_) | Self::Rgb8 => Some(8),
            Self::Mono16 | Self::Bayer16(_) | Self::Rgb16 => Some(16),
            Self::Jpeg => None,
        }
    }

    /// The colour filter mosaic, for the two Bayer formats.
    pub fn bayer_pattern(self) -> Option<BayerPattern> {
        match self {
            Self::Bayer8(pattern) | Self::Bayer16(pattern) => Some(pattern),
            _ => None,
        }
    }
}

/// What is known about how a frame was taken. Everything is optional: a
/// backend fills in what its device tells it.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct FrameMetadata {
    /// When the exposure started.
    pub timestamp: Option<chrono::DateTime<chrono::Utc>>,
    pub exposure_seconds: Option<f64>,
    pub gain: Option<i32>,
    pub bin_x: u32,
    pub bin_y: u32,
    /// Sensor temperature in degrees Celsius.
    pub sensor_temperature_c: Option<f64>,
}

impl Default for FrameMetadata {
    /// Nothing known, unbinned.
    fn default() -> Self {
        Self {
            timestamp: None,
            exposure_seconds: None,
            gain: None,
            bin_x: 1,
            bin_y: 1,
            sensor_temperature_c: None,
        }
    }
}

/// Image data from a camera or imaging connection.
///
/// Build one with [`ImageData::new`] and set `metadata` fields as known.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    /// The pixels, laid out as [`format`](Self::format) says.
    pub data: Bytes,
    pub format: PixelFormat,
    pub frame_kind: FrameKind,
    pub metadata: FrameMetadata,
}

impl ImageData {
    pub fn new(
        width: u32,
        height: u32,
        data: Bytes,
        format: PixelFormat,
        frame_kind: FrameKind,
    ) -> Self {
        Self {
            width,
            height,
            data,
            format,
            frame_kind,
            metadata: FrameMetadata::default(),
        }
    }

    /// How many bytes `data` must hold for these dimensions and this format,
    /// or `None` for a compressed format (or dimensions too large to count).
    pub fn expected_len(&self) -> Option<usize> {
        let pixels = (self.width as usize).checked_mul(self.height as usize)?;
        pixels.checked_mul(self.format.bytes_per_pixel()?)
    }

    /// True if `data` has exactly the length its dimensions and format call
    /// for. Always true for a compressed format.
    pub fn has_expected_len(&self) -> bool {
        self.expected_len().is_none_or(|len| len == self.data.len())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn frame_kinds() -> impl Strategy<Value = FrameKind> {
        prop_oneof![
            Just(FrameKind::Preview),
            Just(FrameKind::Stack),
            Just(FrameKind::Status),
        ]
    }

    fn bayer_patterns() -> impl Strategy<Value = BayerPattern> {
        prop_oneof![
            Just(BayerPattern::Grbg),
            Just(BayerPattern::Rggb),
            Just(BayerPattern::Bggr),
            Just(BayerPattern::Gbrg),
        ]
    }

    #[test]
    fn default_exposure_is_a_one_second_unbinned_light_frame() {
        let exp = ExposureSettings::default();
        assert_eq!(exp.duration_seconds, 1.0);
        assert_eq!((exp.bin_x, exp.bin_y), (1, 1));
        assert_eq!(exp.frame_type, FrameType::Light);
        assert_eq!((exp.gain, exp.offset, exp.roi), (None, None, None));
        assert_eq!(exp.readout_mode, None);
    }

    #[test]
    fn exposure_builders_set_what_they_name() {
        let roi = Roi::new(100, 200, 640, 480).unwrap();
        let exp = ExposureSettings::new(30.0)
            .with_gain(120)
            .with_offset(10)
            .with_binning(2)
            .with_frame_type(FrameType::Dark)
            .with_roi(roi);
        assert_eq!(exp.duration_seconds, 30.0);
        assert_eq!((exp.gain, exp.offset), (Some(120), Some(10)));
        assert_eq!((exp.bin_x, exp.bin_y), (2, 2));
        assert_eq!(exp.frame_type, FrameType::Dark);
        assert_eq!(exp.roi, Some(roi));
    }

    #[test]
    fn settings_written_before_frame_types_existed_still_load() {
        let json = r#"{"duration_seconds":2.0,"gain":80,"offset":null,"bin_x":1,"bin_y":1}"#;
        let exp: ExposureSettings = serde_json::from_str(json).unwrap();
        assert_eq!(exp.frame_type, FrameType::Light);
        assert_eq!(exp.roi, None);
    }

    #[test]
    fn only_light_and_flat_frames_need_light() {
        assert!(FrameType::Light.needs_light());
        assert!(FrameType::Flat.needs_light());
        assert!(!FrameType::Dark.needs_light());
        assert!(!FrameType::Bias.needs_light());
    }

    #[test]
    fn a_roi_must_not_be_empty_and_knows_if_it_fits() {
        assert!(Roi::new(0, 0, 0, 10).is_err());
        assert!(Roi::new(0, 0, 10, 0).is_err());
        assert!(serde_json::from_str::<Roi>(r#"{"x":0,"y":0,"width":0,"height":5}"#).is_err());

        let roi = Roi::new(100, 200, 640, 480).unwrap();
        assert_eq!(
            (roi.x(), roi.y(), roi.width(), roi.height()),
            (100, 200, 640, 480)
        );
        assert!(roi.fits(740, 680));
        assert!(!roi.fits(739, 680));
        assert!(!roi.fits(740, 679));
        // No overflow at the far corner of the number range.
        assert!(!Roi::new(u32::MAX, 0, 2, 1).unwrap().fits(u32::MAX, 10));
    }

    #[test]
    fn bytes_per_pixel_and_sample_size_for_every_format() {
        let p = BayerPattern::Rggb;
        let cases = [
            (PixelFormat::Mono8, Some(1), Some(8)),
            (PixelFormat::Mono16, Some(2), Some(16)),
            (PixelFormat::Bayer8(p), Some(1), Some(8)),
            (PixelFormat::Bayer16(p), Some(2), Some(16)),
            (PixelFormat::Rgb8, Some(3), Some(8)),
            (PixelFormat::Rgb16, Some(6), Some(16)),
            (PixelFormat::Jpeg, None, None),
        ];
        for (format, bytes, bits) in cases {
            assert_eq!(format.bytes_per_pixel(), bytes, "{format:?}");
            assert_eq!(format.bits_per_sample(), bits, "{format:?}");
        }
        assert_eq!(PixelFormat::Bayer16(p).bayer_pattern(), Some(p));
        assert_eq!(PixelFormat::Mono16.bayer_pattern(), None);
        assert_eq!(PixelFormat::Rgb8.bayer_pattern(), None);
    }

    #[test]
    fn expected_len_is_pixels_times_bytes_per_pixel() {
        let image = |format, len: usize| {
            ImageData::new(4, 3, Bytes::from(vec![0; len]), format, FrameKind::Preview)
        };
        assert_eq!(image(PixelFormat::Mono8, 12).expected_len(), Some(12));
        assert_eq!(image(PixelFormat::Mono16, 24).expected_len(), Some(24));
        assert_eq!(
            image(PixelFormat::Bayer16(BayerPattern::Grbg), 24).expected_len(),
            Some(24)
        );
        assert_eq!(image(PixelFormat::Rgb8, 36).expected_len(), Some(36));
        assert_eq!(image(PixelFormat::Rgb16, 72).expected_len(), Some(72));
        assert_eq!(image(PixelFormat::Jpeg, 500).expected_len(), None);

        assert!(image(PixelFormat::Mono16, 24).has_expected_len());
        assert!(!image(PixelFormat::Mono16, 23).has_expected_len());
        // A compressed image has no expected length to miss.
        assert!(image(PixelFormat::Jpeg, 500).has_expected_len());
    }

    #[test]
    fn new_image_has_empty_unbinned_metadata() {
        let image = ImageData::new(
            1,
            1,
            Bytes::from_static(&[0]),
            PixelFormat::Mono8,
            FrameKind::Stack,
        );
        assert_eq!(image.metadata, FrameMetadata::default());
        assert_eq!((image.metadata.bin_x, image.metadata.bin_y), (1, 1));
        assert_eq!(image.metadata.timestamp, None);
    }

    proptest! {
        #[test]
        fn pixel_format_roundtrip(pat in bayer_patterns(), which in 0usize..7) {
            let format = [
                PixelFormat::Mono8,
                PixelFormat::Mono16,
                PixelFormat::Bayer8(pat),
                PixelFormat::Bayer16(pat),
                PixelFormat::Rgb8,
                PixelFormat::Rgb16,
                PixelFormat::Jpeg,
            ][which];
            let s = serde_json::to_string(&format).unwrap();
            let back: PixelFormat = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(format, back);
        }

        #[test]
        fn frame_kind_roundtrip(kind in frame_kinds()) {
            let s = serde_json::to_string(&kind).unwrap();
            let back: FrameKind = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(kind, back);
        }

        #[test]
        fn bayer_pattern_roundtrip(pat in bayer_patterns()) {
            let s = serde_json::to_string(&pat).unwrap();
            let back: BayerPattern = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(pat, back);
        }

        #[test]
        fn exposure_settings_roundtrip(
            // Millisecond precision in [0, 3600s]: exactly representable as f64.
            duration_ms in 0u32..=3_600_000,
            gain in proptest::option::of(any::<i32>()),
            offset in proptest::option::of(any::<i32>()),
            bin_x in 1u32..=16,
            bin_y in 1u32..=16,
            which in 0usize..4,
            roi in proptest::option::of((any::<u32>(), any::<u32>(), 1u32..=10_000, 1u32..=10_000)),
        ) {
            let mut exp = ExposureSettings::new(f64::from(duration_ms) / 1000.0);
            exp.gain = gain;
            exp.offset = offset;
            exp.bin_x = bin_x;
            exp.bin_y = bin_y;
            exp.frame_type =
                [FrameType::Light, FrameType::Dark, FrameType::Bias, FrameType::Flat][which];
            exp.roi = roi.map(|(x, y, w, h)| Roi::new(x, y, w, h).unwrap());
            let s = serde_json::to_string(&exp).unwrap();
            let back: ExposureSettings = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(exp, back);
        }
    }
}
