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
