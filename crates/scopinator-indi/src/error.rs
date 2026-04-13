/// INDI backend errors.
#[derive(Debug, thiserror::Error)]
pub enum IndiError {
    #[error("connection error: {0}")]
    Connection(#[source] std::io::Error),

    #[error("XML parse error: {0}")]
    Xml(#[from] quick_xml::Error),

    #[error("property not found: {device}.{name}")]
    PropertyNotFound { device: String, name: String },

    #[error("property state error: {0}")]
    PropertyState(String),

    #[error("device not found: {0}")]
    DeviceNotFound(String),

    #[error("timeout")]
    Timeout,

    #[error("not connected")]
    NotConnected,

    #[error("BLOB error: {0}")]
    Blob(String),
}
