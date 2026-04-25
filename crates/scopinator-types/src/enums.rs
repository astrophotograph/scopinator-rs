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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SlewState {
    Idle,
    Slewing,
    Tracking,
    Parked,
    Homing,
    Error,
}

/// Current state of a camera.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum CameraState {
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
