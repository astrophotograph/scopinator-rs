/// Alpaca backend errors.
#[derive(Debug, thiserror::Error)]
pub enum AlpacaError {
    #[error("HTTP error: {0}")]
    Http(#[from] reqwest::Error),

    #[error("device error ({code}): {message}")]
    Device { code: i32, message: String },

    #[error("connection error: {0}")]
    Connection(#[source] std::io::Error),

    #[error("timeout")]
    Timeout,

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("no devices found")]
    NoDevices,
}
