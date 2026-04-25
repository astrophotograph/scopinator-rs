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

/// Kind of image frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum FrameKind {
    /// Live view / preview frame.
    Preview,
    /// Stacked result.
    Stack,
}

/// Bayer pattern for raw sensor data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum BayerPattern {
    Grbg,
    Rggb,
    Bggr,
    Gbrg,
}

/// Image data from a camera or imaging connection.
#[derive(Debug, Clone)]
pub struct ImageData {
    pub width: u32,
    pub height: u32,
    pub data: Bytes,
    pub bit_depth: u32,
    pub is_color: bool,
    pub bayer_pattern: Option<BayerPattern>,
    pub frame_kind: FrameKind,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn frame_kinds() -> impl Strategy<Value = FrameKind> {
        prop_oneof![Just(FrameKind::Preview), Just(FrameKind::Stack)]
    }

    fn bayer_patterns() -> impl Strategy<Value = BayerPattern> {
        prop_oneof![
            Just(BayerPattern::Grbg),
            Just(BayerPattern::Rggb),
            Just(BayerPattern::Bggr),
            Just(BayerPattern::Gbrg),
        ]
    }

    proptest! {
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
