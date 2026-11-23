pub mod bus;

pub use bus::UnifiedEventBus;

use scopinator_types::{Coordinates, DeviceId};

/// Types of unified events emitted across all backends.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum EventType {
    Connected,
    Disconnected,
    SlewStarted,
    SlewCompleted,
    TrackingChanged,
    /// A camera's live view started, stopped or changed mode.
    ViewChanged,
    /// A slew that was accepted has failed or was cancelled. The payload is
    /// an [`EventPayload::Error`].
    SlewFailed,
    /// The mount has reached its park position.
    Parked,
    /// The mount has left its park position.
    Unparked,
    /// The mount has changed its side of the pier (a meridian flip).
    PierSideChanged,
    ExposureStarted,
    ExposureCompleted,
    /// A camera has an image to fetch.
    ImageReady,
    StackFrameCompleted,
    FocusChanged,
    /// A focuser move has finished.
    FocuserMoveCompleted,
    /// The filter in the light path has changed.
    FilterChanged,
    /// A step of a longer operation (autofocus, plate solving). The payload
    /// is an [`EventPayload::Progress`].
    OperationProgress,
    StatusUpdate,
    Error,
}

/// A unified event from any backend, normalized to a common format.
///
/// Fields may be added; build one with [`UnifiedEvent::new`].
#[derive(Debug, Clone)]
#[non_exhaustive]
pub struct UnifiedEvent {
    /// The instrument that emitted this event: the id of its
    /// [`Backend`](crate::Backend).
    pub device_id: DeviceId,
    /// The device of that instrument the event is about (its
    /// [`Device::id`](crate::Device::id)), when it is about one.
    pub source: Option<DeviceId>,
    /// The event kind.
    pub event_type: EventType,
    /// Event-specific payload.
    pub payload: EventPayload,
}

impl UnifiedEvent {
    /// An event from the instrument `device_id`, about no device in
    /// particular.
    pub fn new(device_id: DeviceId, event_type: EventType, payload: EventPayload) -> Self {
        Self {
            device_id,
            source: None,
            event_type,
            payload,
        }
    }

    /// The same event, about the instrument's device `suffix` (`mount`,
    /// `camera`, ...): its id is the instrument's id, a colon and the
    /// suffix.
    pub fn about(mut self, suffix: &str) -> Self {
        self.source = Some(DeviceId::new(format!("{}:{suffix}", self.device_id)));
        self
    }
}

/// Event-specific data.
#[derive(Debug, Clone)]
#[non_exhaustive]
pub enum EventPayload {
    /// No additional data.
    None,
    /// Coordinates (e.g., slew target, current position).
    Coordinates(Coordinates),
    /// Tracking state changed.
    Tracking(bool),
    /// Stack progress.
    StackProgress {
        stacked: i32,
        dropped: i32,
        total: i32,
    },
    /// The state of a camera's live view.
    View {
        /// Which camera the view belongs to.
        camera: ViewCamera,
        /// Whether the view is running.
        active: bool,
        /// The backend's name for the view mode (for a Seestar: `star`,
        /// `moon`, `sun`, `scenery`, ...), when it reports one.
        mode: Option<String>,
    },
    /// An error occurred.
    Error {
        /// The backend's own error code.
        code: i32,
        message: String,
        /// What failed, in backend-neutral words (`slew`), when known.
        kind: Option<String>,
    },
    /// A focuser position, in steps.
    FocuserPosition(i32),
    /// A filter position, counted from zero, with its name if known.
    FilterPosition { position: u32, name: Option<String> },
    /// A step of a longer operation.
    Progress {
        /// What is running: `autofocus`, `plate solve`.
        operation: String,
        /// The step it has reached, in the backend's words.
        stage: Option<String>,
        /// How far along it is, from 0 to 1, when the backend can tell.
        fraction: Option<f32>,
    },
    /// Status update with free-form data.
    Status(serde_json::Value),
}

/// Which camera of a device an event refers to. [`UnifiedEvent::source`]
/// says the same with the camera's id and works for any number of cameras.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewCamera {
    /// The main (telephoto) camera.
    Main,
    /// The wide-angle camera, on devices that have one.
    Wide,
}
