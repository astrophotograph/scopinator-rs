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

    /// The property has no member of that name.
    #[error("{device}.{name} has no member {member}")]
    MemberNotFound {
        device: String,
        name: String,
        member: String,
    },

    /// The property can only be read.
    #[error("{device}.{name} is read-only")]
    ReadOnly { device: String, name: String },

    /// The property is not of the kind the request is for (a number written
    /// to a switch, say).
    #[error("{device}.{name} is a {actual} property, not a {wanted} one")]
    WrongKind {
        device: String,
        name: String,
        actual: &'static str,
        wanted: &'static str,
    },

    /// A number outside the limits its member was defined with.
    #[error("{device}.{name}.{member} takes {min} to {max}, not {value}")]
    OutOfRange {
        device: String,
        name: String,
        member: String,
        value: f64,
        min: f64,
        max: f64,
    },

    /// Something the server sent is not the INDI it should be.
    #[error("malformed INDI message: {0}")]
    Malformed(String),

    /// One element grew past the size this client will hold.
    #[error("an INDI element is larger than {limit} bytes")]
    TooLarge { limit: usize },
}
