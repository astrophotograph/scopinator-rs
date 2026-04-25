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
    /// An error occurred.
    Error { code: i32, message: String },
    /// Status update with free-form data.
    Status(serde_json::Value),
}
