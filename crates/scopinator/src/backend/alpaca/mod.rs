//! ASCOM Alpaca: any mount, camera, focuser or filter wheel behind an
//! Alpaca server, which covers most computer-controlled astronomy gear
//! (natively, or through ASCOM Remote in front of an ASCOM driver).

mod camera;
mod mount;

pub use camera::AlpacaCameraDevice;
pub use mount::AlpacaMount;

use scopinator_alpaca::AlpacaError;
use scopinator_alpaca::error::codes;

use crate::error::ScopinatorError;

/// An Alpaca error as the neutral error a caller can match on.
pub(crate) fn neutral(error: AlpacaError) -> ScopinatorError {
    match error.device_code() {
        _ if error.is_not_implemented() => ScopinatorError::NotSupported(error.to_string()),
        Some(codes::INVALID_WHILE_PARKED) => ScopinatorError::Parked,
        Some(codes::NOT_CONNECTED) => ScopinatorError::NotConnected,
        Some(codes::INVALID_VALUE) => ScopinatorError::OutOfRange(error.to_string()),
        Some(codes::INVALID_OPERATION) => ScopinatorError::InvalidState(error.to_string()),
        _ => ScopinatorError::Alpaca(error),
    }
}

/// The value of an optional property: `None` if the device does not
/// implement it, the error for anything else that goes wrong.
pub(crate) fn optional<T>(result: Result<T, AlpacaError>) -> Result<Option<T>, ScopinatorError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.is_not_implemented() => Ok(None),
        Err(e) => Err(neutral(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device(code: i32) -> AlpacaError {
        AlpacaError::Device {
            code,
            message: "m".into(),
        }
    }

    #[test]
    fn alpaca_error_numbers_become_neutral_kinds() {
        assert!(matches!(neutral(device(0x408)), ScopinatorError::Parked));
        assert!(matches!(
            neutral(device(0x407)),
            ScopinatorError::NotConnected
        ));
        assert!(matches!(
            neutral(device(0x401)),
            ScopinatorError::OutOfRange(_)
        ));
        assert!(matches!(
            neutral(device(0x40B)),
            ScopinatorError::InvalidState(_)
        ));
        assert!(neutral(AlpacaError::NotImplemented("x".into())).is_not_supported());
        // Anything else keeps the Alpaca error.
        assert!(matches!(neutral(device(0x500)), ScopinatorError::Alpaca(_)));
        assert!(matches!(
            neutral(AlpacaError::Timeout),
            ScopinatorError::Alpaca(_)
        ));
    }

    #[test]
    fn an_unimplemented_property_is_none_and_a_failure_is_an_error() {
        assert_eq!(optional(Ok(5)).unwrap(), Some(5));
        assert_eq!(
            optional::<i32>(Err(AlpacaError::NotImplemented("x".into()))).unwrap(),
            None
        );
        assert!(optional::<i32>(Err(AlpacaError::Timeout)).is_err());
    }
}
