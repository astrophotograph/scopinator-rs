use serde::{Deserialize, Serialize};

/// Tracking rate for a telescope mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum TrackingRate {
    Sidereal,
    Lunar,
    Solar,
    King,
    Custom,
    Off,
}

/// Current slew state of a mount.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum SlewState {
    #[default]
    Idle,
    Slewing,
    Tracking,
    Parked,
    Homing,
    Error,
}

/// Current state of a camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[non_exhaustive]
pub enum CameraState {
    #[default]
    Idle,
    Waiting,
    Exposing,
    Reading,
    Downloading,
    Error,
}

/// Pier side for German equatorial mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum PierSide {
    East,
    West,
    Unknown,
}

/// One of a mount's two axes, whatever kind of mount it is.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum MountAxis {
    /// Right ascension on an equatorial mount, azimuth on an alt-az mount.
    Primary,
    /// Declination on an equatorial mount, altitude on an alt-az mount.
    Secondary,
}

/// A direction for a guiding pulse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum GuideDirection {
    North,
    South,
    East,
    West,
}

/// How a mount's axes are arranged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AlignmentMode {
    /// Altitude and azimuth axes.
    AltAz,
    /// A polar axis, without a meridian flip: a fork or single arm on a
    /// wedge.
    Polar,
    /// A German equatorial mount, which flips at the meridian.
    GermanPolar,
}

#[cfg(test)]
mod tests {
    use super::*;
    use proptest::prelude::*;

    fn tracking_rates() -> impl Strategy<Value = TrackingRate> {
        prop_oneof![
            Just(TrackingRate::Sidereal),
            Just(TrackingRate::Lunar),
            Just(TrackingRate::Solar),
            Just(TrackingRate::King),
            Just(TrackingRate::Custom),
            Just(TrackingRate::Off),
        ]
    }

    fn slew_states() -> impl Strategy<Value = SlewState> {
        prop_oneof![
            Just(SlewState::Idle),
            Just(SlewState::Slewing),
            Just(SlewState::Tracking),
            Just(SlewState::Parked),
            Just(SlewState::Homing),
            Just(SlewState::Error),
        ]
    }

    fn camera_states() -> impl Strategy<Value = CameraState> {
        prop_oneof![
            Just(CameraState::Idle),
            Just(CameraState::Waiting),
            Just(CameraState::Exposing),
            Just(CameraState::Reading),
            Just(CameraState::Downloading),
            Just(CameraState::Error),
        ]
    }

    fn pier_sides() -> impl Strategy<Value = PierSide> {
        prop_oneof![
            Just(PierSide::East),
            Just(PierSide::West),
            Just(PierSide::Unknown),
        ]
    }

    #[test]
    fn guide_direction_and_alignment_mode_roundtrip() {
        for direction in [
            GuideDirection::North,
            GuideDirection::South,
            GuideDirection::East,
            GuideDirection::West,
        ] {
            let json = serde_json::to_string(&direction).unwrap();
            assert_eq!(
                serde_json::from_str::<GuideDirection>(&json).unwrap(),
                direction
            );
        }
        for mode in [
            AlignmentMode::AltAz,
            AlignmentMode::Polar,
            AlignmentMode::GermanPolar,
        ] {
            let json = serde_json::to_string(&mode).unwrap();
            assert_eq!(serde_json::from_str::<AlignmentMode>(&json).unwrap(), mode);
        }
    }

    #[test]
    fn mount_axis_roundtrip() {
        for axis in [MountAxis::Primary, MountAxis::Secondary] {
            let json = serde_json::to_string(&axis).unwrap();
            let back: MountAxis = serde_json::from_str(&json).unwrap();
            assert_eq!(axis, back);
        }
    }

    proptest! {
        #[test]
        fn tracking_rate_roundtrip(rate in tracking_rates()) {
            let s = serde_json::to_string(&rate).unwrap();
            let back: TrackingRate = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(rate, back);
        }

        #[test]
        fn slew_state_roundtrip(state in slew_states()) {
            let s = serde_json::to_string(&state).unwrap();
            let back: SlewState = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(state, back);
        }

        #[test]
        fn camera_state_roundtrip(state in camera_states()) {
            let s = serde_json::to_string(&state).unwrap();
            let back: CameraState = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(state, back);
        }

        #[test]
        fn pier_side_roundtrip(side in pier_sides()) {
            let s = serde_json::to_string(&side).unwrap();
            let back: PierSide = serde_json::from_str(&s).unwrap();
            prop_assert_eq!(side, back);
        }
    }
}
