use serde::{Deserialize, Serialize};

/// INDI property state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PropertyState {
    Idle,
    Ok,
    Busy,
    Alert,
}

impl PropertyState {
    pub fn parse(s: &str) -> Self {
        match s {
            "Idle" => Self::Idle,
            "Ok" => Self::Ok,
            "Busy" => Self::Busy,
            "Alert" => Self::Alert,
            _ => Self::Idle,
        }
    }
}

/// INDI switch state.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SwitchState {
    On,
    Off,
}

impl SwitchState {
    pub fn parse(s: &str) -> Self {
        match s {
            "On" => Self::On,
            "Off" => Self::Off,
            _ => Self::Off,
        }
    }
}

/// An INDI property value.
#[derive(Debug, Clone)]
pub enum PropertyValue {
    Number {
        name: String,
        value: f64,
        min: f64,
        max: f64,
        step: f64,
    },
    Switch {
        name: String,
        state: SwitchState,
    },
    Text {
        name: String,
        value: String,
    },
    Light {
        name: String,
        state: PropertyState,
    },
    Blob {
        name: String,
        data: Vec<u8>,
        format: String,
        size: usize,
    },
}

/// An INDI property vector (group of related values).
#[derive(Debug, Clone)]
pub struct Property {
    pub device: String,
    pub name: String,
    pub label: Option<String>,
    pub group: Option<String>,
    pub state: PropertyState,
    pub values: Vec<PropertyValue>,
}

/// INDI device interface flags (bitmask).
pub mod interface {
    pub const TELESCOPE: u32 = 1 << 0;
    pub const CCD: u32 = 1 << 1;
    pub const GUIDER: u32 = 1 << 2;
    pub const FOCUSER: u32 = 1 << 3;
    pub const FILTER: u32 = 1 << 4;
    pub const DOME: u32 = 1 << 5;
    pub const GPS: u32 = 1 << 6;
    pub const WEATHER: u32 = 1 << 7;
    pub const AO: u32 = 1 << 8;
    pub const DUSTCAP: u32 = 1 << 9;
    pub const LIGHTBOX: u32 = 1 << 10;
    pub const DETECTOR: u32 = 1 << 11;
    pub const ROTATOR: u32 = 1 << 12;
    pub const AUX: u32 = 1 << 15;
}
