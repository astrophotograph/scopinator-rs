#[derive(Debug, thiserror::Error)]
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

    #[cfg(feature = "seestar")]
    #[error("seestar error: {0}")]
    Seestar(#[from] scopinator_seestar::SeestarError),
}
