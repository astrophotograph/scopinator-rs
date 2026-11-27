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
    use scopinator_seestar::event::EventState;

    let event_of = |event_type, payload| UnifiedEvent::new(device_id.clone(), event_type, payload);
    match event {
        SeestarEvent::AutoGoto(data) => {
            let (event_type, payload) = match data.state.as_ref() {
                Some(EventState::Start | EventState::Working) => {
                    (EventType::SlewStarted, EventPayload::None)
                }
                Some(EventState::Complete) => (EventType::SlewCompleted, EventPayload::None),
                Some(EventState::Fail) => (
                    EventType::SlewFailed,
                    EventPayload::Error {
                        code: data.code.unwrap_or(-1),
                        message: data.error.clone().unwrap_or_default(),
                        kind: Some("slew".into()),
                    },
                ),
                _ => return None,
            };
            Some(event_of(event_type, payload).about("mount"))
        }

        SeestarEvent::ScopeTrack(data) => Some(
            event_of(
                EventType::TrackingChanged,
                EventPayload::Tracking(data.tracking),
            )
            .about("mount"),
        ),

        // Parking is a `ScopeHome` that completes with the arm closed.
        SeestarEvent::ScopeHome(data) => (data.state == Some(EventState::Complete) && data.close)
            .then(|| event_of(EventType::Parked, EventPayload::None).about("mount")),

        SeestarEvent::View(data) => view_changed(data, ViewCamera::Main, device_id),
        SeestarEvent::SecondView(data) => view_changed(data, ViewCamera::Wide, device_id),

        SeestarEvent::Stack(data) => {
            if data.state.as_deref() == Some("frame_complete") {
                Some(
                    event_of(
                        EventType::StackFrameCompleted,
                        EventPayload::StackProgress {
                            stacked: data.stacked_frame,
                            dropped: data.dropped_frame,
                            total: data.total_frame,
                        },
                    )
                    .about("camera"),
                )
            } else {
                None
            }
        }

        SeestarEvent::FocuserMove(data) => (data.state == Some(EventState::Complete)).then(|| {
            event_of(
                EventType::FocuserMoveCompleted,
                EventPayload::FocuserPosition(data.position),
            )
        }),

        // The position counts from zero, as `get_wheel_position` does: a
        // view without the light pollution filter ends a move at 1, the
        // IR-cut filter of an S30. The names are the wheel's to ask for.
        SeestarEvent::WheelMove(data) => (data.state == Some(EventState::Complete)).then(|| {
            let payload = match u32::try_from(data.position) {
                Ok(position) => EventPayload::FilterPosition {
                    position,
                    name: None,
                },
                Err(_) => EventPayload::None,
            };
            event_of(EventType::FilterChanged, payload).about("filterwheel")
        }),

        SeestarEvent::AutoFocus(data) => data
            .state
            .as_ref()
            .map(|state| progress(device_id, "autofocus", state.as_str())),

        SeestarEvent::PlateSolve(value) => value
            .get("state")
            .and_then(|state| state.as_str())
            .map(|state| progress(device_id, "plate solve", state)),

        SeestarEvent::PiStatus(_) => {
            // Status updates are frequent, only emit as StatusUpdate
            trace!(device = %device_id, "PiStatus event (not translated)");
            None
        }

        SeestarEvent::Alert(data) => Some(event_of(
            EventType::Error,
            EventPayload::Error {
                code: data.code,
                message: data.error.clone(),
                kind: None,
            },
        )),

        _ => None,
    }
}

/// A step of a longer operation, named as the telescope names it (`start`,
/// `working`, `solving`, `complete`, `fail`).
fn progress(device_id: &DeviceId, operation: &str, stage: &str) -> UnifiedEvent {
    UnifiedEvent::new(
        device_id.clone(),
        EventType::OperationProgress,
        EventPayload::Progress {
            operation: operation.into(),
            stage: Some(stage.into()),
            fraction: None,
        },
    )
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
    let source = match camera {
        ViewCamera::Main => "camera",
        ViewCamera::Wide => "wide",
    };
    Some(
        UnifiedEvent::new(
            device_id.clone(),
            EventType::ViewChanged,
            EventPayload::View {
                camera,
                active,
                mode,
            },
        )
        .about(source),
    )
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

    fn source(event: &UnifiedEvent) -> Option<&str> {
        event.source.as_ref().map(|id| id.as_str())
    }

    #[test]
    fn view_events_name_their_camera_as_the_source() {
        let main = translated(
            json!({"Event": "View", "Timestamp": "1.0", "state": "working",
            "lapse_ms": 0, "mode": "star", "cam_id": 0}),
        )
        .unwrap();
        assert_eq!(main.device_id.as_str(), "test");
        assert_eq!(source(&main), Some("test:camera"));

        let wide = translated(json!({"Event": "SecondView", "Timestamp": "1.0",
            "state": "working", "lapse_ms": 0, "mode": "star", "cam_id": 1}))
        .unwrap();
        assert_eq!(source(&wide), Some("test:wide"));
    }

    #[test]
    fn a_failed_goto_is_a_failed_slew_from_the_mount() {
        let unified = translated(json!({"Event": "AutoGoto", "Timestamp": "1.0",
            "state": "fail", "lapse_ms": 9000, "error": "mount goto failed", "code": 501}))
        .unwrap();
        assert_eq!(unified.event_type, EventType::SlewFailed);
        assert_eq!(source(&unified), Some("test:mount"));
        match unified.payload {
            EventPayload::Error {
                code,
                message,
                kind,
            } => {
                assert_eq!(code, 501);
                assert_eq!(message, "mount goto failed");
                assert_eq!(kind.as_deref(), Some("slew"));
            }
            other => panic!("got {other:?}"),
        }
    }

    #[test]
    fn goto_start_and_completion_come_from_the_mount() {
        for (state, expected) in [
            ("start", EventType::SlewStarted),
            ("working", EventType::SlewStarted),
            ("complete", EventType::SlewCompleted),
        ] {
            let unified = translated(json!({"Event": "AutoGoto", "Timestamp": "1.0",
                "state": state, "lapse_ms": 0}))
            .unwrap();
            assert_eq!(unified.event_type, expected, "{state}");
            assert_eq!(source(&unified), Some("test:mount"));
        }
    }

    // `scope_park` gives `ScopeHome` working, then complete with `close`
    // true (docs/protocol/hardware-observations.md).
    #[test]
    fn a_completed_home_with_the_arm_closed_is_parked() {
        let parked = translated(json!({"Event": "ScopeHome", "Timestamp": "1.0",
            "state": "complete", "lapse_ms": 23000, "close": true}))
        .unwrap();
        assert_eq!(parked.event_type, EventType::Parked);
        assert_eq!(source(&parked), Some("test:mount"));

        // Still moving, or home without closing: not parked.
        assert!(
            translated(json!({"Event": "ScopeHome", "Timestamp": "1.0",
            "state": "working", "lapse_ms": 100, "close": false}))
            .is_none()
        );
        assert!(
            translated(json!({"Event": "ScopeHome", "Timestamp": "1.0",
            "state": "complete", "lapse_ms": 100, "close": false}))
            .is_none()
        );
    }

    // Shapes from the corpus.
    #[test]
    fn a_completed_wheel_move_is_a_filter_change() {
        assert!(
            translated(json!({"Event": "WheelMove", "Timestamp": "1.0", "state": "start"}))
                .is_none()
        );
        let changed = translated(json!({"Event": "WheelMove", "Timestamp": "1.0",
            "state": "complete", "position": 1}))
        .unwrap();
        assert_eq!(changed.event_type, EventType::FilterChanged);
        // Counted from zero, like the wheel's own positions.
        assert!(matches!(
            changed.payload,
            EventPayload::FilterPosition {
                position: 1,
                name: None
            }
        ));
        assert_eq!(
            changed.source.as_ref().map(ToString::to_string),
            Some("test:filterwheel".into())
        );
    }

    #[test]
    fn a_completed_focuser_move_carries_the_position() {
        let moved = translated(json!({"Event": "FocuserMove", "Timestamp": "1.0",
            "state": "complete", "lapse_ms": 400, "position": 1580}))
        .unwrap();
        assert_eq!(moved.event_type, EventType::FocuserMoveCompleted);
        assert!(matches!(moved.payload, EventPayload::FocuserPosition(1580)));
        assert!(
            translated(json!({"Event": "FocuserMove", "Timestamp": "1.0",
            "state": "working", "lapse_ms": 100, "position": 1500}))
            .is_none()
        );
    }

    #[test]
    fn plate_solve_and_autofocus_steps_are_progress() {
        let solving = translated(json!({"Event": "PlateSolve", "Timestamp": "1.0",
            "page": "stack", "state": "solving", "star_number": 1473}))
        .unwrap();
        assert_eq!(solving.event_type, EventType::OperationProgress);
        match solving.payload {
            EventPayload::Progress {
                operation,
                stage,
                fraction,
            } => {
                assert_eq!(operation, "plate solve");
                assert_eq!(stage.as_deref(), Some("solving"));
                assert_eq!(fraction, None);
            }
            other => panic!("got {other:?}"),
        }

        let focusing = translated(json!({"Event": "AutoFocus", "Timestamp": "1.0",
            "state": "working", "lapse_ms": 500}))
        .unwrap();
        match focusing.payload {
            EventPayload::Progress {
                operation, stage, ..
            } => {
                assert_eq!(operation, "autofocus");
                assert_eq!(stage.as_deref(), Some("working"));
            }
            other => panic!("got {other:?}"),
        }
    }
}
