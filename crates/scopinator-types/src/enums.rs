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
