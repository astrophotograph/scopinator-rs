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
