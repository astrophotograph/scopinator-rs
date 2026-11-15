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
    ExposureStarted,
    ExposureCompleted,
    StackFrameCompleted,
    FocusChanged,
    StatusUpdate,
    Error,
}

/// A unified event from any backend, normalized to a common format.
#[derive(Debug, Clone)]
pub struct UnifiedEvent {
    /// Which device emitted this event.
    pub device_id: DeviceId,
    /// The event kind.
    pub event_type: EventType,
    /// Event-specific payload.
    pub payload: EventPayload,
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
    Error { code: i32, message: String },
    /// Status update with free-form data.
    Status(serde_json::Value),
}

/// Which camera of a device an event refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ViewCamera {
    /// The main (telephoto) camera.
    Main,
    /// The wide-angle camera, on devices that have one.
    Wide,
}
