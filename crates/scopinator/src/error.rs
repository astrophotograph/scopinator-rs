#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum ScopinatorError {
    #[error("not connected")]
    NotConnected,

    #[error("operation not supported: {0}")]
    NotSupported(String),

    #[error("operation timed out")]
    Timeout,

    #[error("backend error: {0}")]
    Backend(String),

    #[error("invalid argument: {0}")]
    InvalidArgument(String),

    /// The device is doing something else that excludes this request.
    #[error("device is busy: {0}")]
    Busy(String),

    /// The mount is parked and the request needs it unparked.
    #[error("mount is parked")]
    Parked,

    /// The request makes no sense in the device's current state (for
    /// example, fetching an image before one has been taken).
    #[error("invalid state: {0}")]
    InvalidState(String),

    /// A value is outside what the device accepts: a target below the
    /// horizon or beyond a slew limit, a gain or position out of range.
    #[error("out of range: {0}")]
    OutOfRange(String),

    /// The connection dropped while the device was in use.
    #[error("connection lost: {0}")]
    ConnectionLost(String),

    #[cfg(feature = "seestar")]
    #[error("seestar error: {0}")]
    Seestar(#[from] scopinator_seestar::SeestarError),

    #[cfg(feature = "alpaca")]
    #[error("alpaca error: {0}")]
    Alpaca(#[from] scopinator_alpaca::AlpacaError),

    #[cfg(feature = "indi")]
    #[error("indi error: {0}")]
    Indi(#[from] scopinator_indi::IndiError),
}

impl ScopinatorError {
    /// True when the device or backend does not have the requested
    /// operation at all, as opposed to having failed at it. Callers use this
    /// to skip an optional step instead of aborting.
    pub fn is_not_supported(&self) -> bool {
        matches!(self, Self::NotSupported(_))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn neutral_kinds_display() {
        let cases = [
            (
                ScopinatorError::Busy("slewing".into()),
                "device is busy: slewing",
            ),
            (ScopinatorError::Parked, "mount is parked"),
            (
                ScopinatorError::InvalidState("no image yet".into()),
                "invalid state: no image yet",
            ),
            (
                ScopinatorError::OutOfRange("gain 900 (0..=600)".into()),
                "out of range: gain 900 (0..=600)",
            ),
            (
                ScopinatorError::ConnectionLost("reset by peer".into()),
                "connection lost: reset by peer",
            ),
        ];
        for (err, text) in cases {
            assert_eq!(err.to_string(), text);
        }
    }

    #[test]
    fn only_not_supported_is_not_supported() {
        assert!(ScopinatorError::NotSupported("sync".into()).is_not_supported());
        assert!(!ScopinatorError::Parked.is_not_supported());
        assert!(!ScopinatorError::Timeout.is_not_supported());
        assert!(!ScopinatorError::Backend("x".into()).is_not_supported());
    }

    #[cfg(feature = "alpaca")]
    #[test]
    fn alpaca_errors_convert() {
        let err: ScopinatorError = scopinator_alpaca::AlpacaError::Timeout.into();
        assert_eq!(err.to_string(), "alpaca error: timeout");
    }

    #[cfg(feature = "indi")]
    #[test]
    fn indi_errors_convert() {
        let err: ScopinatorError = scopinator_indi::IndiError::NotConnected.into();
        assert_eq!(err.to_string(), "indi error: not connected");
    }
}
