use std::sync::Arc;

use scopinator_seestar::SeestarClient;
use scopinator_seestar::event::SeestarEvent;
use scopinator_types::DeviceId;
use tracing::{debug, trace, warn};

use crate::event::{EventPayload, EventType, UnifiedEvent, UnifiedEventBus, ViewCamera};

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

        SeestarEvent::View(data) => view_changed(data, ViewCamera::Main, device_id),
        SeestarEvent::SecondView(data) => view_changed(data, ViewCamera::Wide, device_id),

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

/// A `View` or `SecondView` event as a unified view change. The device
/// reports `working` (or `start`) while a view runs and `cancel`, `complete`,
/// `fail` or `idle` when it ends; a mode of `none` also means no view.
fn view_changed(
    data: &scopinator_seestar::event::ViewEventData,
    camera: ViewCamera,
    device_id: &DeviceId,
) -> Option<UnifiedEvent> {
    use scopinator_seestar::event::EventState;
    let mode = data.mode.clone().filter(|m| m != "none");
    let active = match data.state.as_ref()? {
        EventState::Start | EventState::Working => mode.is_some() || data.mode.is_none(),
        EventState::Cancel | EventState::Complete | EventState::Fail | EventState::Idle => false,
        EventState::Other(_) => return None,
    };
    Some(UnifiedEvent {
        device_id: device_id.clone(),
        event_type: EventType::ViewChanged,
        payload: EventPayload::View {
            camera,
            active,
            mode,
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn translated(event: serde_json::Value) -> Option<UnifiedEvent> {
        let event: SeestarEvent = serde_json::from_value(event).unwrap();
        translate(&event, &DeviceId::new("test"))
    }

    // The shapes are from the 6.70 corpus (conformance/sessions/s50_fw670_a).
    #[test]
    fn a_working_view_is_active() {
        let unified = translated(
            json!({"Event": "View", "Timestamp": "1.0", "state": "working",
            "lapse_ms": 0, "mode": "star", "cam_id": 0, "lp_filter": false, "gain": 80}),
        )
        .unwrap();
        assert_eq!(unified.event_type, EventType::ViewChanged);
        match unified.payload {
            EventPayload::View {
                camera,
                active,
                mode,
            } => {
                assert_eq!(camera, ViewCamera::Main);
                assert!(active);
                assert_eq!(mode.as_deref(), Some("star"));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn a_cancelled_view_is_inactive() {
        for mode in ["star", "none"] {
            let unified = translated(json!({"Event": "View", "Timestamp": "1.0",
                "state": "cancel", "mode": mode}))
            .unwrap();
            assert!(matches!(
                unified.payload,
                EventPayload::View { active: false, .. }
            ));
        }
    }

    #[test]
    fn second_view_is_the_wide_camera() {
        let unified = translated(json!({"Event": "SecondView", "Timestamp": "1.0",
            "state": "working", "mode": "scenery"}))
        .unwrap();
        assert!(matches!(
            unified.payload,
            EventPayload::View {
                camera: ViewCamera::Wide,
                active: true,
                ..
            }
        ));
    }

    #[test]
    fn a_view_event_without_a_state_is_not_translated() {
        assert!(translated(json!({"Event": "View", "Timestamp": "1.0"})).is_none());
    }
}
