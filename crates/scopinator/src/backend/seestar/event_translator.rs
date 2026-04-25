use std::sync::Arc;

use scopinator_seestar::SeestarClient;
use scopinator_seestar::event::SeestarEvent;
use scopinator_types::DeviceId;
use tracing::{debug, trace, warn};

use crate::event::{EventPayload, EventType, UnifiedEvent, UnifiedEventBus};

/// Translates Seestar-specific events into unified events.
pub struct SeestarEventTranslator;

impl SeestarEventTranslator {
    /// Start translating events from a SeestarClient in the background.
    pub fn start(client: Arc<SeestarClient>, device_id: DeviceId, event_bus: Arc<UnifiedEventBus>) {
        let mut rx = client.subscribe_events();
        tokio::spawn(async move {
            debug!(device = %device_id, "event translator started");
            loop {
                match rx.recv().await {
                    Ok(event) => {
                        if let Some(unified) = translate(&event, &device_id) {
                            event_bus.emit(unified);
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            device = %device_id,
                            skipped = n,
                            "event translator lagged"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!(device = %device_id, "event translator stopped (channel closed)");
                        break;
                    }
                }
            }
        });
    }
}

fn translate(event: &SeestarEvent, device_id: &DeviceId) -> Option<UnifiedEvent> {
    match event {
        SeestarEvent::AutoGoto(data) => {
            let (event_type, payload) = match data.state.as_ref() {
                Some(
                    scopinator_seestar::event::EventState::Start
                    | scopinator_seestar::event::EventState::Working,
                ) => (EventType::SlewStarted, EventPayload::None),
                Some(scopinator_seestar::event::EventState::Complete) => {
                    (EventType::SlewCompleted, EventPayload::None)
                }
                Some(scopinator_seestar::event::EventState::Fail) => (
                    EventType::Error,
                    EventPayload::Error {
                        code: data.code.unwrap_or(-1),
                        message: data.error.clone().unwrap_or_default(),
                    },
                ),
                _ => return None,
            };
            Some(UnifiedEvent {
                device_id: device_id.clone(),
                event_type,
                payload,
            })
        }

        SeestarEvent::ScopeTrack(data) => Some(UnifiedEvent {
            device_id: device_id.clone(),
            event_type: EventType::TrackingChanged,
            payload: EventPayload::Tracking(data.tracking),
        }),

        SeestarEvent::Stack(data) => {
            if data.state.as_deref() == Some("frame_complete") {
                Some(UnifiedEvent {
                    device_id: device_id.clone(),
                    event_type: EventType::StackFrameCompleted,
                    payload: EventPayload::StackProgress {
                        stacked: data.stacked_frame,
                        dropped: data.dropped_frame,
                        total: data.total_frame,
                    },
                })
            } else {
                None
            }
        }

        SeestarEvent::PiStatus(_) => {
            // Status updates are frequent, only emit as StatusUpdate
            trace!(device = %device_id, "PiStatus event (not translated)");
            None
        }

        SeestarEvent::Alert(data) => Some(UnifiedEvent {
            device_id: device_id.clone(),
            event_type: EventType::Error,
            payload: EventPayload::Error {
                code: data.code,
                message: data.error.clone(),
            },
        }),

        _ => None,
    }
}
