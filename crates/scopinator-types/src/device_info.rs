use serde::{Deserialize, Serialize};
use std::fmt;

/// Unique identifier for a device.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DeviceId(String);

impl DeviceId {
    pub fn new(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// What kind of device something is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum DeviceKind {
    Mount,
    Camera,
    Focuser,
    FilterWheel,
    Switch,
    Other,
}

impl fmt::Display for DeviceKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mount => "mount",
            Self::Camera => "camera",
            Self::Focuser => "focuser",
            Self::FilterWheel => "filter wheel",
            Self::Switch => "switch",
            Self::Other => "other",
        })
    }
}

/// Who a device is: enough to tell it from the other devices of an
/// instrument and to show it to a person.
///
/// Fields may be added; build one with [`DeviceInfo::new`] and set the
/// optional fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct DeviceInfo {
    /// Unique among the devices a program has open, and stable for as long
    /// as the device stays at the same address.
    pub id: DeviceId,
    pub kind: DeviceKind,
    /// A name to show a person.
    pub name: String,
    /// The product model, e.g. `Seestar S50`, when the device reports one.
    pub model: Option<String>,
    /// The firmware version as the device writes it, e.g. `6.70`.
    pub firmware: Option<String>,
    /// The driver behind the device, for backends that have one (an ASCOM
    /// driver's name and version, an INDI driver).
    pub driver: Option<String>,
}

impl DeviceInfo {
    pub fn new(id: DeviceId, kind: DeviceKind, name: impl Into<String>) -> Self {
        Self {
            id,
            kind,
            name: name.into(),
            model: None,
            firmware: None,
            driver: None,
        }
    }
}

/// Human-readable device name.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceName(String);

impl DeviceName {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for DeviceName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// Firmware version (integer form as reported by Seestar).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct FirmwareVersion(pub u32);

impl FirmwareVersion {
    /// Firmware versions above this threshold require "verify" in command params.
    pub const VERIFY_THRESHOLD: u32 = 2582;

    pub fn requires_verify(&self) -> bool {
        self.0 > Self::VERIFY_THRESHOLD
    }
}

impl fmt::Display for FirmwareVersion {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    #[test]
    fn device_kind_roundtrip() {
        for kind in [
            DeviceKind::Mount,
            DeviceKind::Camera,
            DeviceKind::Focuser,
            DeviceKind::FilterWheel,
            DeviceKind::Switch,
            DeviceKind::Other,
        ] {
            let json = serde_json::to_string(&kind).unwrap();
            let back: DeviceKind = serde_json::from_str(&json).unwrap();
            assert_eq!(kind, back);
        }
    }

    proptest! {
        #[test]
        fn device_id_roundtrip(s in ".{0,64}") {
            let id = DeviceId::new(s.clone());
            prop_assert_eq!(id.as_str(), &s);
            let json = serde_json::to_string(&id).unwrap();
            let back: DeviceId = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(id, back);
        }

        #[test]
        fn device_name_roundtrip(s in ".{0,64}") {
            let name = DeviceName::new(s.clone());
            let json = serde_json::to_string(&name).unwrap();
            let back: DeviceName = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(name, back);
        }

        #[test]
        fn device_info_roundtrip(id in ".{0,32}", name in ".{0,32}", model in proptest::option::of(".{0,16}")) {
            let mut info = DeviceInfo::new(DeviceId::new(id), DeviceKind::Camera, name);
            info.model = model;
            let json = serde_json::to_string(&info).unwrap();
            let back: DeviceInfo = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(info, back);
        }

        #[test]
        fn firmware_version_roundtrip(v in any::<u32>()) {
            let fw = FirmwareVersion(v);
            let json = serde_json::to_string(&fw).unwrap();
            let back: FirmwareVersion = serde_json::from_str(&json).unwrap();
            prop_assert_eq!(fw, back);
        }

        #[test]
        fn requires_verify_matches_threshold(v in any::<u32>()) {
            prop_assert_eq!(
                FirmwareVersion(v).requires_verify(),
                v > FirmwareVersion::VERIFY_THRESHOLD
            );
        }
    }
}
