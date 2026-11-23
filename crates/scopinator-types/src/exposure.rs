use bytes::Bytes;
use serde::{Deserialize, Serialize};

/// Settings for a camera exposure.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExposureSettings {
    pub duration_seconds: f64,
    pub gain: Option<i32>,
    pub offset: Option<i32>,
    pub bin_x: u32,
    pub bin_y: u32,
    pub light: bool,
}

impl Default for ExposureSettings {
    fn default() -> Self {
        Self {
            duration_seconds: 1.0,
            gain: None,
            offset: None,
            bin_x: 1,
            bin_y: 1,
            light: true,
        }
    }
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
            light in any::<bool>(),
        ) {
            let duration_seconds = f64::from(duration_ms) / 1000.0;
            let exp = ExposureSettings { duration_seconds, gain, offset, bin_x, bin_y, light };
            let s = serde_json::to_string(&exp).unwrap();
            let back: ExposureSettings = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(exp, back);
        }
    }
}
