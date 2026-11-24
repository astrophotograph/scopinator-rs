//! Backends: one per kind of telescope or protocol.
//!
//! A [`Backend`] is one instrument a program is connected to, and hands out
//! its devices as the vendor-neutral traits of [`crate::device`].

#[cfg(feature = "alpaca")]
pub mod alpaca;
#[cfg(feature = "seestar")]
pub mod seestar;

use std::any::Any;
use std::sync::Arc;

use async_trait::async_trait;
use scopinator_types::DeviceId;

use crate::device::{
    AutoFocus, Camera, FilterWheel, Focuser, LiveStacker, Mount, Observer, PlateSolver,
};
use crate::error::ScopinatorError;

/// One connected instrument: a smart telescope, or a server that fronts a
/// mount, cameras and accessories.
///
/// Code written against this trait works with any telescope scopinator
/// supports. What the instrument does not have comes back as `None` or an
/// empty list.
#[async_trait]
pub trait Backend: Send + Sync {
    /// The instrument's id, e.g. `seestar:192.168.1.100`. It is the
    /// `device_id` of the events the instrument emits and the prefix of its
    /// devices' ids.
    fn id(&self) -> DeviceId;

    /// Which backend this is: `seestar`, `alpaca`, ...
    fn kind(&self) -> &'static str;

    /// The mount, if the instrument has one.
    fn mount(&self) -> Option<Arc<dyn Mount>>;

    /// The cameras, main camera first. This can need a round trip to the
    /// instrument (to learn whether it has a second camera, or to open a
    /// connection to it), so it is async and can fail.
    async fn cameras(&self) -> Result<Vec<Arc<dyn Camera>>, ScopinatorError>;

    /// The focuser, if the instrument has one that can be driven.
    fn focuser(&self) -> Option<Arc<dyn Focuser>>;

    /// The filter wheel, if the instrument has one.
    fn filter_wheel(&self) -> Option<Arc<dyn FilterWheel>>;

    /// Target finding, on a smart telescope: readiness, and pointing at a
    /// target and centring it.
    fn observer(&self) -> Option<Arc<dyn Observer>> {
        None
    }

    /// Stacking on the instrument itself, on a smart telescope.
    fn live_stacker(&self) -> Option<Arc<dyn LiveStacker>> {
        None
    }

    /// The instrument focusing itself.
    fn auto_focus(&self) -> Option<Arc<dyn AutoFocus>> {
        None
    }

    /// The instrument working out where it points.
    fn plate_solver(&self) -> Option<Arc<dyn PlateSolver>> {
        None
    }

    /// Close the connection to the instrument.
    async fn disconnect(&self) -> Result<(), ScopinatorError>;

    /// The concrete backend, for what the neutral traits do not cover:
    /// `backend.as_any().downcast_ref::<SeestarBackend>()`.
    fn as_any(&self) -> &dyn Any;
}

/// The main camera of a backend, if it has any camera.
pub async fn main_camera(
    backend: &dyn Backend,
) -> Result<Option<Arc<dyn Camera>>, ScopinatorError> {
    Ok(backend.cameras().await?.into_iter().next())
}
