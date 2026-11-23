/// Alpaca backend errors.
#[derive(Debug, thiserror::Error)]
pub enum AlpacaError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("device error ({code}): {message}")]
    Device { code: i32, message: String },

    /// The device does not have this property or method (Alpaca error
    /// 0x400, or 0x40C for an action).
    #[error("not implemented by the device: {0}")]
    NotImplemented(String),

    #[error("connection error: {0}")]
    Connection(#[source] std::io::Error),

    #[error("timeout")]
    Timeout,

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    /// A reply that is well-formed JSON but not a value this client can use.
    #[error("unusable value: {0}")]
    Value(String),

    #[error("no devices found")]
    NoDevices,
}

/// Alpaca error numbers (ASCOM reserved range 0x400 to 0x4FF).
pub mod codes {
    pub const NOT_IMPLEMENTED: i32 = 0x400;
    pub const INVALID_VALUE: i32 = 0x401;
    pub const VALUE_NOT_SET: i32 = 0x402;
    pub const NOT_CONNECTED: i32 = 0x407;
    pub const INVALID_WHILE_PARKED: i32 = 0x408;
    pub const INVALID_WHILE_SLAVED: i32 = 0x409;
    pub const INVALID_OPERATION: i32 = 0x40B;
    pub const ACTION_NOT_IMPLEMENTED: i32 = 0x40C;
}

impl AlpacaError {
    /// The error for a non-zero `ErrorNumber` in a reply.
    pub(crate) fn from_reply(code: i32, message: String) -> Self {
        match code {
            codes::NOT_IMPLEMENTED | codes::ACTION_NOT_IMPLEMENTED => Self::NotImplemented(message),
            _ => Self::Device { code, message },
        }
    }

    /// The Alpaca error number, for an error the device reported.
    pub fn device_code(&self) -> Option<i32> {
        match self {
            Self::Device { code, .. } => Some(*code),
            Self::NotImplemented(_) => Some(codes::NOT_IMPLEMENTED),
            _ => None,
        }
    }

    /// True if the device does not have what was asked for.
    pub fn is_not_implemented(&self) -> bool {
        matches!(self, Self::NotImplemented(_))
    }
}
