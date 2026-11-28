//! What an INDI server sends: property definitions, updates, deletions and
//! text messages.

use crate::protocol::property::{PropertyState, SwitchState};

/// The five kinds of property vector.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VectorKind {
    Number,
    Switch,
    Text,
    Light,
    Blob,
}

/// Whether a client may read a property, write it, or both.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Permission {
    ReadOnly,
    WriteOnly,
    ReadWrite,
}

impl Permission {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "ro" => Some(Self::ReadOnly),
            "wo" => Some(Self::WriteOnly),
            "rw" => Some(Self::ReadWrite),
            _ => None,
        }
    }

    pub fn can_write(self) -> bool {
        !matches!(self, Self::ReadOnly)
    }
}

/// How the members of a switch vector constrain each other.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SwitchRule {
    /// Exactly one is on: a selector.
    OneOfMany,
    /// One or none is on.
    AtMostOne,
    /// Each is independent.
    AnyOfMany,
}

impl SwitchRule {
    pub fn parse(text: &str) -> Option<Self> {
        match text {
            "OneOfMany" => Some(Self::OneOfMany),
            "AtMostOne" => Some(Self::AtMostOne),
            "AnyOfMany" => Some(Self::AnyOfMany),
            _ => None,
        }
    }
}

/// The contents of one BLOB member.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Blob {
    /// The format as a file name suffix, for example `.fits`. A trailing
    /// `.z` means the data is zlib-compressed.
    pub format: String,
    pub data: Vec<u8>,
}

/// The value of one member of a property vector.
#[derive(Debug, Clone, PartialEq)]
pub enum MemberValue {
    Number(f64),
    Switch(SwitchState),
    Text(String),
    Light(PropertyState),
    Blob(Blob),
    /// A BLOB member as it is defined: it has no value until one is sent.
    NoBlob,
}

/// The limits and display format of a number member.
#[derive(Debug, Clone, PartialEq)]
pub struct NumberLimits {
    /// A printf-style format, including INDI's own `%m` for sexagesimal.
    pub format: String,
    pub min: f64,
    /// Equal to `min` when the number has no limits.
    pub max: f64,
    pub step: f64,
}

/// One member of a property definition.
#[derive(Debug, Clone, PartialEq)]
pub struct MemberDefinition {
    pub name: String,
    pub label: Option<String>,
    pub value: MemberValue,
    /// For a number member.
    pub limits: Option<NumberLimits>,
}

/// A `def*Vector`: a property a device has, with its members and their
/// values at the time.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyDefinition {
    pub kind: VectorKind,
    pub device: String,
    pub name: String,
    pub label: Option<String>,
    pub group: Option<String>,
    pub state: PropertyState,
    pub permission: Permission,
    /// Only on a switch vector.
    pub rule: Option<SwitchRule>,
    /// Seconds a change may take; zero or absent for no particular time.
    pub timeout: Option<f64>,
    pub timestamp: Option<String>,
    pub message: Option<String>,
    pub members: Vec<MemberDefinition>,
}

/// A `set*Vector`: new values, a new state, or both, for a property that
/// has been defined.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyUpdate {
    pub kind: VectorKind,
    pub device: String,
    pub name: String,
    /// `None` leaves the state as it was.
    pub state: Option<PropertyState>,
    pub timeout: Option<f64>,
    pub timestamp: Option<String>,
    pub message: Option<String>,
    /// The members that were sent, which need not be all of them.
    pub members: Vec<(String, MemberValue)>,
}

/// One message from an INDI server.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum IndiMessage {
    Define(PropertyDefinition),
    Update(PropertyUpdate),
    /// A property is gone, or with no name the whole device.
    Delete {
        device: String,
        name: Option<String>,
    },
    /// Text for a person to read, from a device or from the server.
    Message {
        device: Option<String>,
        timestamp: Option<String>,
        text: String,
    },
    /// An element this library does not know. Not an error.
    Unknown {
        element: String,
    },
}
