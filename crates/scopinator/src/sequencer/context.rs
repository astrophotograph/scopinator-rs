use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use scopinator_types::ImageData;
use tokio::task::JoinHandle;

use crate::backend::Backend;
use crate::device::smart::{AutoFocus, LiveStacker, Observer};
use crate::device::traits::{Camera, FilterWheel, Focuser, Mount};
use crate::error::ScopinatorError;

/// Where the frames of an imaging run go, on an instrument whose camera
/// takes them one at a time. A telescope that stacks keeps its own stack and
/// does not use this.
#[async_trait]
pub trait FrameSink: Send + Sync {
    /// Take one frame. An error ends the imaging run.
    async fn frame(&self, image: ImageData) -> Result<(), ScopinatorError>;
}

/// The devices a sequence works with.
///
/// Build one from a connected instrument with
/// [`from_backend`](Self::from_backend), or from separate devices with
/// [`new`](Self::new) and the `with_` methods. A command that needs a device
/// the context does not have fails with
/// [`NotSupported`](ScopinatorError::NotSupported).
#[non_exhaustive]
pub struct ExecutionContext {
    /// The mount to use for slewing.
    pub mount: Arc<dyn Mount>,
    /// The camera to use for imaging, on an instrument without a stacker.
    pub camera: Option<Arc<dyn Camera>>,
    /// Finds targets on a smart telescope. Used for `GoToTarget` in place of
    /// the mount when present.
    pub observer: Option<Arc<dyn Observer>>,
    /// Stacks on a smart telescope. Used for `StartImaging` in place of the
    /// camera when present.
    pub live_stacker: Option<Arc<dyn LiveStacker>>,
    pub auto_focus: Option<Arc<dyn AutoFocus>>,
    pub focuser: Option<Arc<dyn Focuser>>,
    pub filter_wheel: Option<Arc<dyn FilterWheel>>,
    /// Receives the frames of a camera imaging run.
    pub frame_sink: Option<Arc<dyn FrameSink>>,
    /// The camera imaging run going on in the background, if any.
    pub(super) imaging: Mutex<Option<JoinHandle<()>>>,
}

impl ExecutionContext {
    /// A context with a mount and nothing else.
    pub fn new(mount: Arc<dyn Mount>) -> Self {
        Self {
            mount,
            camera: None,
            observer: None,
            live_stacker: None,
            auto_focus: None,
            focuser: None,
            filter_wheel: None,
            frame_sink: None,
            imaging: Mutex::new(None),
        }
    }

    /// Everything a connected instrument offers: its mount, its main
    /// camera, and whichever of the other devices it has. Fails with
    /// `NotSupported` on an instrument without a mount.
    pub async fn from_backend(backend: &dyn Backend) -> Result<Self, ScopinatorError> {
        let mount = backend.mount().ok_or_else(|| {
            ScopinatorError::NotSupported("this instrument has no mount to sequence".into())
        })?;
        let mut context = Self::new(mount);
        context.camera = backend.cameras().await?.into_iter().next();
        context.observer = backend.observer();
        context.live_stacker = backend.live_stacker();
        context.auto_focus = backend.auto_focus();
        context.focuser = backend.focuser();
        context.filter_wheel = backend.filter_wheel();
        Ok(context)
    }

    pub fn with_camera(mut self, camera: Arc<dyn Camera>) -> Self {
        self.camera = Some(camera);
        self
    }

    pub fn with_observer(mut self, observer: Arc<dyn Observer>) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn with_live_stacker(mut self, stacker: Arc<dyn LiveStacker>) -> Self {
        self.live_stacker = Some(stacker);
        self
    }

    pub fn with_auto_focus(mut self, auto_focus: Arc<dyn AutoFocus>) -> Self {
        self.auto_focus = Some(auto_focus);
        self
    }

    pub fn with_focuser(mut self, focuser: Arc<dyn Focuser>) -> Self {
        self.focuser = Some(focuser);
        self
    }

    pub fn with_filter_wheel(mut self, wheel: Arc<dyn FilterWheel>) -> Self {
        self.filter_wheel = Some(wheel);
        self
    }

    pub fn with_frame_sink(mut self, sink: Arc<dyn FrameSink>) -> Self {
        self.frame_sink = Some(sink);
        self
    }

    /// End the background camera run, if one is going.
    pub(super) fn end_imaging(&self) -> bool {
        let run = self
            .imaging
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take();
        match run {
            Some(run) => {
                run.abort();
                true
            }
            None => false,
        }
    }
}

impl Drop for ExecutionContext {
    fn drop(&mut self) {
        self.end_imaging();
    }
}
