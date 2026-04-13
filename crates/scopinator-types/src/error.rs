#[derive(Debug, thiserror::Error)]
pub enum TypeError {
    #[error("invalid RA: {0} degrees (must be 0.0..360.0)")]
    InvalidRa(f64),

    #[error("invalid RA: {0} hours (must be 0.0..24.0)")]
    InvalidRaHours(f64),

    #[error("invalid Dec: {0} degrees (must be -90.0..=90.0)")]
    InvalidDec(f64),
}
