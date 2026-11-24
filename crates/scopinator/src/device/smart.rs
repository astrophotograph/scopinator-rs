//! Traits for smart telescopes: instruments that run the observing workflow
//! themselves.
//!
//! A classic rig is a mount, a camera and a focuser, and the computer does
//! the rest. A smart telescope is asked for outcomes instead: "find this
//! target and centre it", "stack", "focus yourself". These traits are those
//! requests. Each is optional: a [`Backend`](crate::Backend) hands out the
//! ones its instrument has, and a plain mount-and-camera server has none.
//!
//! They follow the same rule as the device traits (see
//! [`traits`](super::traits)): a request that takes time returns when the
//! telescope has accepted it, a state query says where it has got to, a
//! failure after acceptance comes back from that query once, and waiting is
//! done by the helpers in [`wait`](super::wait).

use async_trait::async_trait;
use scopinator_types::{
    AcquisitionState, AutoFocusState, ImageData, PlateSolution, Readiness, SolarSystemBody,
    StackProgress, StackSettings, Target,
};
use std::time::Duration;

use crate::error::ScopinatorError;

/// What an [`Observer`] can be asked for. Start from [`Default`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct ObserverCapabilities {
    /// Takes [`Target::Sky`].
    pub sky_targets: bool,
    /// The solar-system bodies it can find by name.
    pub solar_system: Vec<SolarSystemBody>,
    /// Takes [`Target::Scenery`].
    pub scenery: bool,
    /// Centres a target by plate solving after it has slewed.
    pub centres_by_plate_solve: bool,
    /// [`Observer::prepare`] does something.
    pub can_prepare: bool,
}

/// Finding targets: readiness, and pointing at a target and centring it.
///
/// This is the smart-telescope counterpart of
/// [`Mount::slew_to_coordinates`](super::Mount::slew_to_coordinates). The
/// mount points; the observer finds, which also covers targets that are not
/// a pair of coordinates and the centring that follows the slew.
#[async_trait]
pub trait Observer: Send + Sync {
    fn capabilities(&self) -> ObserverCapabilities;

    /// What stands between the telescope and finding a target: an unset
    /// clock, an unknown site, a calibration not yet done.
    async fn readiness(&self) -> Result<Readiness, ScopinatorError>;

    /// Have the telescope deal with what [`readiness`](Self::readiness)
    /// lists, as far as it can by itself. Returns when the telescope has
    /// accepted the request; `readiness` says when it is done. What it
    /// cannot supply (a site it was never told) is a
    /// [`NotReady`](ScopinatorError::NotReady) error.
    async fn prepare(&self) -> Result<(), ScopinatorError>;

    /// Point at `target` and centre it. Returns when the telescope has
    /// accepted the request; [`acquisition`](Self::acquisition) says how far
    /// it has got.
    async fn acquire(&self, target: &Target) -> Result<(), ScopinatorError>;

    /// How far the telescope has got in finding the target. If finding it
    /// failed after the request was accepted, the first call after the
    /// failure returns the error and later calls return `Idle`.
    async fn acquisition(&self) -> Result<AcquisitionState, ScopinatorError>;

    /// Stop observing: cancel a search for a target and end the view.
    async fn stop(&self) -> Result<(), ScopinatorError>;
}

/// What a [`LiveStacker`] accepts. Start from [`Default`].
#[derive(Debug, Clone, Default, PartialEq)]
#[non_exhaustive]
pub struct StackerCapabilities {
    /// The shortest and longest sub-exposure, on a stacker whose exposure
    /// can be set to anything in a range.
    pub exposure_range: Option<(Duration, Duration)>,
    /// The sub-exposures on offer, on a stacker that has a fixed list.
    pub exposure_choices: Vec<Duration>,
    pub gain_range: Option<(i32, i32)>,
    /// Can stop by itself after a number of frames.
    pub can_limit_frames: bool,
    /// The filters it can stack through, by name.
    pub filters: Vec<String>,
}

/// Stacking on the telescope: start, stop, how far it has got, and the stack
/// so far.
///
/// A setting the telescope cannot honour is an error
/// ([`NotSupported`](ScopinatorError::NotSupported) or
/// [`OutOfRange`](ScopinatorError::OutOfRange)) before anything starts. It
/// is never ignored.
#[async_trait]
pub trait LiveStacker: Send + Sync {
    fn capabilities(&self) -> StackerCapabilities;

    /// Start stacking what the telescope is looking at. Returns when the
    /// telescope has accepted the request.
    async fn start_stacking(&self, settings: &StackSettings) -> Result<(), ScopinatorError>;

    /// Stop stacking. The stack so far stays available.
    async fn stop_stacking(&self) -> Result<(), ScopinatorError>;

    /// How far the stack has got. A stack that fails returns its error
    /// once.
    async fn progress(&self) -> Result<StackProgress, ScopinatorError>;

    /// The stack so far.
    /// [`InvalidState`](ScopinatorError::InvalidState) before the first
    /// frame.
    async fn stacked_image(&self) -> Result<ImageData, ScopinatorError>;
}

/// The telescope focusing itself.
#[async_trait]
pub trait AutoFocus: Send + Sync {
    /// Start an autofocus run. Returns when the telescope has accepted the
    /// request.
    async fn start_autofocus(&self) -> Result<(), ScopinatorError>;

    async fn stop_autofocus(&self) -> Result<(), ScopinatorError>;

    /// Whether a run is under way or done. A run that fails returns its
    /// error once.
    async fn autofocus_state(&self) -> Result<AutoFocusState, ScopinatorError>;
}

/// The telescope working out where it points from the stars in view.
#[async_trait]
pub trait PlateSolver: Send + Sync {
    /// Start a plate solve of the current view. Returns when the telescope
    /// has accepted the request.
    async fn start_solve(&self) -> Result<(), ScopinatorError>;

    /// The solution, or `None` while the solve is running. A solve that
    /// fails returns its error once.
    async fn solution(&self) -> Result<Option<PlateSolution>, ScopinatorError>;
}
