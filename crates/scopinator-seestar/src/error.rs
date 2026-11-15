use std::time::Duration;

#[derive(Debug, thiserror::Error)]
pub enum SeestarError {
    #[error("connection failed: {0}")]
    Connection(#[source] std::io::Error),

    #[error("connection timed out after {0:?}")]
    Timeout(Duration),

    #[error("protocol error: {0}")]
    Protocol(String),

    #[error("command failed (code {code}): {message}")]
    CommandFailed { code: i32, message: String },

    #[error("device disconnected")]
    Disconnected,

    #[error("frame too large: {size} bytes (limit: {limit})")]
    FrameTooLarge { size: u32, limit: u32 },

    #[error("invalid frame header")]
    InvalidFrame,

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("line too long: {size} bytes (limit: {limit})")]
    LineTooLong { size: usize, limit: usize },

    /// The device answered an image-socket request with an error message
    /// instead of an image, e.g. `there is no image now`.
    #[error("image request refused by the telescope: {0}")]
    ImageRequest(String),

    /// An image payload could not be decoded (e.g. a damaged zip archive).
    #[error("invalid image payload: {0}")]
    ImagePayload(String),

    #[error("authentication failed: {0}")]
    AuthFailed(String),

    /// The device has the verification handshake, no interop key is
    /// configured, and it does not serve requests carrying the `verify`
    /// marker. Every command would be silently consumed.
    #[error(
        "this telescope's firmware requires client verification: \
         configure an interop key (SeestarConfig::interop_key)"
    )]
    VerificationRequired,

    /// The device refused the connection because too many clients are
    /// connected (Alert 429). It admits two distinct client addresses, or
    /// eight with guest mode on. The client keeps retrying with backoff.
    #[error(
        "the telescope refused the connection: too many clients are connected \
         (close another app or enable guest mode)"
    )]
    ConnectionLimit,

    #[error("failed to load interoperability key: {0}")]
    InteropKeyLoad(String),
}

/// Defines [`ErrorCode`]: the device's numeric error codes, by name.
macro_rules! error_codes {
    ($( $(#[$meta:meta])* $variant:ident = $code:literal, )*) => {
        /// A device error code, as carried in the `code` member of responses
        /// and events.
        ///
        /// Only codes that have been seen on hardware, or that this crate
        /// acts on, have names; the device has about two hundred. Any other
        /// value is kept in [`ErrorCode::Other`], so matching on this enum
        /// never loses a code. The device reuses some codes for different
        /// situations (207 in particular), so the accompanying error text is
        /// still worth reading.
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
        pub enum ErrorCode {
            $( $(#[$meta])* $variant, )*
            /// A code without a name here.
            Other(i32),
        }

        impl ErrorCode {
            /// The numeric code.
            pub fn code(self) -> i32 {
                match self {
                    $( Self::$variant => $code, )*
                    Self::Other(code) => code,
                }
            }
        }

        impl From<i32> for ErrorCode {
            fn from(code: i32) -> Self {
                match code {
                    $( $code => Self::$variant, )*
                    other => Self::Other(other),
                }
            }
        }

        #[cfg(test)]
        const NAMED_ERROR_CODES: &[(ErrorCode, i32)] = &[ $( (ErrorCode::$variant, $code), )* ];
    };
}

error_codes! {
    /// "method not found": the method does not exist on this firmware, or is
    /// not permitted for a guest client.
    MethodNotFound = 103,
    /// "expected int param".
    ExpectedIntParam = 104,
    /// "expected object param".
    ExpectedObjectParam = 107,
    /// "unexpected param": an unknown member. `set_setting` answers this to
    /// the `verify` marker.
    UnexpectedParam = 109,
    /// "equipment is moving" (from the protocol specification; not yet seen
    /// on hardware).
    EquipmentMoving = 203,
    /// "fail to operate": a general failure. Seen for a bad `verify_client`
    /// signature, for `get_annotate_result` with nothing annotated, and on a
    /// cancelled `ScopeGoto`.
    FailToOperate = 207,
    /// "no solve data".
    NoSolveData = 215,
    /// "aborted": a cancelled `AutoGoto`.
    Aborted = 253,
    /// "file not exist".
    FileNotExist = 255,
    /// "below horizon".
    BelowHorizon = 270,
    /// The `Alert` sent to a connection refused for exceeding the client
    /// limit (from the protocol specification).
    ConnectionLimit = 429,
    /// "mount goto failed".
    MountGotoFailed = 501,
    /// "function conflict" (from the protocol specification; not yet seen on
    /// hardware).
    FunctionConflict = 524,
}

impl ErrorCode {
    /// The code of a response or event, or `None` for success (0).
    pub fn from_response_code(code: i32) -> Option<Self> {
        (code != 0).then(|| Self::from(code))
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Other(code) => write!(f, "{code}"),
            named => write!(f, "{named:?} ({})", named.code()),
        }
    }
}

impl SeestarError {
    /// The device's error code, if this error is a command the device
    /// rejected ([`SeestarError::CommandFailed`]).
    pub fn device_code(&self) -> Option<ErrorCode> {
        match self {
            Self::CommandFailed { code, .. } => Some(ErrorCode::from(*code)),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_named_code_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for &(named, code) in NAMED_ERROR_CODES {
            assert_eq!(named.code(), code);
            assert_eq!(ErrorCode::from(code), named);
            assert!(seen.insert(code), "code {code} is listed twice");
            assert!(!matches!(named, ErrorCode::Other(_)));
        }
        assert!(NAMED_ERROR_CODES.len() >= 13);
    }

    #[test]
    fn unnamed_codes_are_kept() {
        for code in [1, 102, 218, 318, 536, 570, -1, i32::MAX] {
            let parsed = ErrorCode::from(code);
            assert_eq!(parsed, ErrorCode::Other(code));
            assert_eq!(parsed.code(), code);
        }
        assert_eq!(ErrorCode::from_response_code(0), None);
        assert_eq!(
            ErrorCode::from_response_code(109),
            Some(ErrorCode::UnexpectedParam)
        );
    }

    #[test]
    fn command_failed_exposes_its_code() {
        let err = SeestarError::CommandFailed {
            code: 103,
            message: "method not found".into(),
        };
        assert_eq!(err.device_code(), Some(ErrorCode::MethodNotFound));
        assert_eq!(SeestarError::Disconnected.device_code(), None);
        assert_eq!(
            ErrorCode::MethodNotFound.to_string(),
            "MethodNotFound (103)"
        );
        assert_eq!(ErrorCode::Other(999).to_string(), "999");
    }
}
