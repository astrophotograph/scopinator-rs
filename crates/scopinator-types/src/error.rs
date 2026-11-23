#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("invalid RA: {0} degrees (must be 0.0..360.0)")]
    InvalidRa(f64),

    #[error("invalid RA: {0} hours (must be 0.0..24.0)")]
    InvalidRaHours(f64),

    #[error("invalid Dec: {0} degrees (must be -90.0..=90.0)")]
    InvalidDec(f64),

    #[error("invalid latitude: {0} degrees (must be -90.0..=90.0)")]
    InvalidLatitude(f64),

    #[error("invalid longitude: {0} degrees (must be -180.0..=180.0, east positive)")]
    InvalidLongitude(f64),

    #[error("invalid altitude: {0} degrees (must be -90.0..=90.0)")]
    InvalidAltitude(f64),

    #[error("invalid azimuth: {0} degrees (must be 0.0..360.0)")]
    InvalidAzimuth(f64),

    #[error("empty region of interest: {width}x{height}")]
    EmptyRoi { width: u32, height: u32 },
}
