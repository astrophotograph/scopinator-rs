//! Types for smart telescopes: instruments that find, centre, focus and
//! stack by themselves.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::coordinates::Coordinates;

/// A body of the solar system that a telescope can find by name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum SolarSystemBody {
    Sun,
    Moon,
    Mercury,
    Venus,
    Mars,
    Jupiter,
    Saturn,
    Uranus,
    Neptune,
}

impl SolarSystemBody {
    /// True for the planets, as opposed to the Sun and the Moon.
    pub fn is_planet(self) -> bool {
        !matches!(self, Self::Sun | Self::Moon)
    }
}

/// What to observe.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Target {
    /// A position on the sky, in any equatorial system, with the name to
    /// show for it.
    Sky {
        name: Option<String>,
        coordinates: Coordinates,
    },
    /// A body the telescope finds by itself.
    SolarSystem(SolarSystemBody),
    /// Wherever the telescope points now, as a daytime or terrestrial view.
    Scenery,
}

impl Target {
    /// A named position on the sky.
    pub fn named(name: impl Into<String>, coordinates: Coordinates) -> Self {
        Self::Sky {
            name: Some(name.into()),
            coordinates,
        }
    }

    /// A position on the sky with no name.
    pub fn at(coordinates: Coordinates) -> Self {
        Self::Sky {
            name: None,
            coordinates,
        }
    }
}

/// Something that has to be true before a telescope can find a target.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Prerequisite {
    /// The telescope does not know where on Earth it is.
    SiteLocation,
    /// The telescope's clock is unset or wrong.
    Clock,
    /// The telescope has not worked out how it is oriented (its calibration
    /// or alignment).
    Calibration,
    /// The telescope is not in focus, and needs to be for the next step.
    Focus,
    /// The telescope has no dark frames for the settings in use.
    DarkFrames,
    Other(String),
}

impl std::fmt::Display for Prerequisite {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SiteLocation => f.write_str("site location"),
            Self::Clock => f.write_str("clock"),
            Self::Calibration => f.write_str("calibration"),
            Self::Focus => f.write_str("focus"),
            Self::DarkFrames => f.write_str("dark frames"),
            Self::Other(what) => f.write_str(what),
        }
    }
}

/// Whether a telescope is ready to find targets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Readiness {
    Ready,
    /// Not ready until these are dealt with.
    Needs(Vec<Prerequisite>),
    /// The telescope cannot say.
    Unknown,
}

/// How far a telescope has got in finding a target.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AcquisitionState {
    /// Not finding anything.
    Idle,
    /// Moving to the target.
    Slewing,
    /// At the target's rough position: plate solving and re-centring.
    Centring,
    /// The target is in view.
    OnTarget,
}

/// How to stack. Everything is optional: a `None` leaves the telescope's own
/// setting. Start from [`Default`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StackSettings {
    /// Length of each sub-exposure.
    pub exposure: Option<Duration>,
    pub gain: Option<i32>,
    /// Stop by itself after this many frames, on a telescope that can.
    pub frame_limit: Option<u32>,
    /// Which of the telescope's filters to use, by name.
    pub filter: Option<String>,
    /// Throw away an existing stack of this target instead of adding to it.
    pub restart: bool,
}

/// What a stacker is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub enum StackState {
    #[default]
    Idle,
    Stacking,
    /// Reached its frame limit and stopped by itself.
    Finished,
}

/// How far a stack has got. Start from [`Default`].
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[non_exhaustive]
pub struct StackProgress {
    pub state: StackState,
    /// Frames in the stack.
    pub stacked: u32,
    /// Frames taken and rejected.
    pub dropped: u32,
    /// The number of frames the stack will stop at, if it has a limit.
    pub frame_limit: Option<u32>,
    /// Total exposure in the stack.
    pub integration: Option<Duration>,
}

/// What a telescope's autofocus is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum AutoFocusState {
    /// Has not run.
    Idle,
    Running,
    /// Finished, at this focuser position if the telescope reports one.
    Done {
        position: Option<i32>,
    },
}

/// Where a plate solve found the telescope to be pointing. Build one with
/// [`PlateSolution::new`] and set what else is known.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct PlateSolution {
    /// The centre of the field.
    pub center: Coordinates,
    /// Rotation of the field in degrees, north through east.
    pub rotation_deg: Option<f64>,
    /// Arcseconds per pixel.
    pub pixel_scale_arcsec: Option<f64>,
    /// Width and height of the field in degrees.
    pub field_of_view_deg: Option<(f64, f64)>,
}

impl PlateSolution {
    pub fn new(center: Coordinates) -> Self {
        Self {
            center,
            rotation_deg: None,
            pixel_scale_arcsec: None,
            field_of_view_deg: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m31() -> Coordinates {
        Coordinates::from_hours(0.712, 41.27).unwrap()
    }

    fn round_trip<T>(value: &T) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        serde_json::from_str(&serde_json::to_string(value).unwrap()).unwrap()
    }

    #[test]
    fn targets_round_trip() {
        for target in [
            Target::named("M31", m31()),
            Target::at(m31()),
            Target::SolarSystem(SolarSystemBody::Jupiter),
            Target::Scenery,
        ] {
            assert_eq!(round_trip(&target), target);
        }
    }

    #[test]
    fn the_sun_and_moon_are_not_planets() {
        assert!(!SolarSystemBody::Sun.is_planet());
        assert!(!SolarSystemBody::Moon.is_planet());
        assert!(SolarSystemBody::Mercury.is_planet());
        assert!(SolarSystemBody::Neptune.is_planet());
    }

    #[test]
    fn readiness_and_states_round_trip() {
        let needs = Readiness::Needs(vec![
            Prerequisite::Clock,
            Prerequisite::Other("level the tripod".into()),
        ]);
        assert_eq!(round_trip(&needs), needs);
        assert_eq!(round_trip(&Readiness::Ready), Readiness::Ready);
        for state in [
            AcquisitionState::Idle,
            AcquisitionState::Slewing,
            AcquisitionState::Centring,
            AcquisitionState::OnTarget,
        ] {
            assert_eq!(round_trip(&state), state);
        }
        let done = AutoFocusState::Done {
            position: Some(1580),
        };
        assert_eq!(round_trip(&done), done);
    }

    #[test]
    fn prerequisites_read_as_plain_words() {
        assert_eq!(Prerequisite::SiteLocation.to_string(), "site location");
        assert_eq!(Prerequisite::Clock.to_string(), "clock");
        assert_eq!(Prerequisite::Other("level".into()).to_string(), "level");
    }

    #[test]
    fn default_stack_settings_leave_everything_to_the_telescope() {
        let settings = StackSettings::default();
        assert_eq!(settings.exposure, None);
        assert_eq!(settings.gain, None);
        assert_eq!(settings.frame_limit, None);
        assert_eq!(settings.filter, None);
        assert!(!settings.restart);

        let chosen = StackSettings {
            exposure: Some(Duration::from_secs(10)),
            frame_limit: Some(60),
            ..StackSettings::default()
        };
        assert_eq!(round_trip(&chosen), chosen);
    }

    #[test]
    fn default_progress_is_an_idle_empty_stack() {
        let progress = StackProgress::default();
        assert_eq!(progress.state, StackState::Idle);
        assert_eq!((progress.stacked, progress.dropped), (0, 0));
        assert_eq!(round_trip(&progress), progress);
    }

    #[test]
    fn a_solution_starts_with_only_its_centre() {
        let solution = PlateSolution::new(m31());
        assert_eq!(solution.rotation_deg, None);
        assert_eq!(solution.field_of_view_deg, None);
        assert_eq!(round_trip(&solution), solution);
    }
}
