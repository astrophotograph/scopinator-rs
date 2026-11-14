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

    #[error("failed to load interoperability key: {0}")]
    InteropKeyLoad(String),
}
