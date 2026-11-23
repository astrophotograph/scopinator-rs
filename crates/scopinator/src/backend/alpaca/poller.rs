//! Events for a protocol that has none.
//!
//! Alpaca is polled: nothing tells a client that a slew has finished. The
//! poller reads the state of a backend's devices at an interval and puts an
//! event on the bus for each change, so that a client can follow an Alpaca
//! instrument the way it follows one that reports its own events.

use std::sync::Arc;
use std::time::Duration;

use scopinator_types::{Coordinates, DeviceId};

use super::{AlpacaCameraDevice, AlpacaFilterWheelDevice, AlpacaFocuserDevice, AlpacaMount};
use crate::device::traits::{Camera, Device, FilterWheel, Focuser, Mount};
use crate::error::ScopinatorError;
use crate::event::{EventPayload, EventType, UnifiedEvent, UnifiedEventBus};

/// The longest the poller waits between attempts while the server is not
/// answering.
const MAX_BACKOFF: Duration = Duration::from_secs(30);

/// How many polls in a row must fail before the instrument counts as
/// disconnected.
const FAILURES_TO_DISCONNECT: u32 = 3;

/// The devices one poller watches.
pub(crate) struct Watched {
    pub(crate) backend_id: DeviceId,
    pub(crate) mount: Option<Arc<AlpacaMount>>,
    pub(crate) cameras: Vec<Arc<AlpacaCameraDevice>>,
    pub(crate) focuser: Option<Arc<AlpacaFocuserDevice>>,
    pub(crate) filter_wheel: Option<Arc<AlpacaFilterWheelDevice>>,
}

/// What the poller compares from one poll to the next. A `None` is a state
/// the device cannot report.
#[derive(Debug, Clone, PartialEq, Default)]
struct Snapshot {
    mount: Option<MountSnapshot>,
    cameras: Vec<CameraSnapshot>,
    focuser: Option<FocuserSnapshot>,
    filter: Option<FilterSnapshot>,
}

#[derive(Debug, Clone, PartialEq)]
struct MountSnapshot {
    id: DeviceId,
    slewing: Option<bool>,
    parked: Option<bool>,
    tracking: Option<bool>,
    position: Option<Coordinates>,
}

#[derive(Debug, Clone, PartialEq)]
struct CameraSnapshot {
    id: DeviceId,
    exposing: Option<bool>,
    image_ready: Option<bool>,
}

#[derive(Debug, Clone, PartialEq)]
struct FocuserSnapshot {
    id: DeviceId,
    moving: Option<bool>,
    position: Option<i32>,
}

#[derive(Debug, Clone, PartialEq)]
struct FilterSnapshot {
    id: DeviceId,
    /// `Some(None)` while the wheel turns.
    position: Option<Option<(u32, Option<String>)>>,
}

/// A state the device cannot report is `None`; any other failure fails the
/// poll.
fn readable<T>(result: Result<T, ScopinatorError>) -> Result<Option<T>, ScopinatorError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.is_not_supported() => Ok(None),
        Err(e) => Err(e),
    }
}

impl Watched {
    async fn snapshot(&self) -> Result<Snapshot, ScopinatorError> {
        let mount = match &self.mount {
            Some(mount) => Some(MountSnapshot {
                id: mount.id(),
                slewing: readable(mount.is_slewing().await)?,
                parked: readable(mount.is_parked().await)?,
                tracking: readable(mount.is_tracking().await)?,
                position: readable(mount.get_coordinates().await)?,
            }),
            None => None,
        };
        let mut cameras = Vec::with_capacity(self.cameras.len());
        for camera in &self.cameras {
            cameras.push(CameraSnapshot {
                id: camera.id(),
                exposing: readable(camera.is_exposing().await)?,
                image_ready: readable(camera.image_ready().await)?,
            });
        }
        let focuser = match &self.focuser {
            Some(focuser) => {
                let position = readable(focuser.focuser_position().await)?;
                Some(FocuserSnapshot {
                    id: focuser.id(),
                    moving: position.as_ref().map(|p| p.is_moving),
                    position: position
                        .filter(|_| focuser.capabilities().absolute)
                        .map(|p| p.position),
                })
            }
            None => None,
        };
        let filter = match &self.filter_wheel {
            Some(wheel) => Some(FilterSnapshot {
                id: wheel.id(),
                position: readable(wheel.filter_position().await)?
                    .map(|position| position.map(|p| (p.position, p.name))),
            }),
            None => None,
        };
        Ok(Snapshot {
            mount,
            cameras,
            focuser,
            filter,
        })
    }
}

/// An event to emit: its type, its payload and the device it is about.
type Change = (EventType, EventPayload, DeviceId);

/// The events that tell `before` from `after`. Only a change from one known
/// state to another known state is an event.
fn changes(before: &Snapshot, after: &Snapshot) -> Vec<Change> {
    let mut events = Vec::new();
    let flipped = |old: Option<bool>, new: Option<bool>| match (old, new) {
        (Some(old), Some(new)) if old != new => Some(new),
        _ => None,
    };

    if let (Some(old), Some(new)) = (&before.mount, &after.mount) {
        let position = || {
            new.position
                .map_or(EventPayload::None, EventPayload::Coordinates)
        };
        match flipped(old.slewing, new.slewing) {
            Some(true) => events.push((EventType::SlewStarted, EventPayload::None, new.id.clone())),
            Some(false) => events.push((EventType::SlewCompleted, position(), new.id.clone())),
            None => {}
        }
        match flipped(old.parked, new.parked) {
            Some(true) => events.push((EventType::Parked, EventPayload::None, new.id.clone())),
            Some(false) => events.push((EventType::Unparked, EventPayload::None, new.id.clone())),
            None => {}
        }
        if let Some(tracking) = flipped(old.tracking, new.tracking) {
            events.push((
                EventType::TrackingChanged,
                EventPayload::Tracking(tracking),
                new.id.clone(),
            ));
        }
    }

    for (old, new) in before.cameras.iter().zip(&after.cameras) {
        if flipped(old.exposing, new.exposing) == Some(true) {
            events.push((
                EventType::ExposureStarted,
                EventPayload::None,
                new.id.clone(),
            ));
        }
        if flipped(old.image_ready, new.image_ready) == Some(true) {
            events.push((
                EventType::ExposureCompleted,
                EventPayload::None,
                new.id.clone(),
            ));
            events.push((EventType::ImageReady, EventPayload::None, new.id.clone()));
        }
    }

    if let (Some(old), Some(new)) = (&before.focuser, &after.focuser)
        && flipped(old.moving, new.moving) == Some(false)
    {
        let payload = new
            .position
            .map_or(EventPayload::None, EventPayload::FocuserPosition);
        events.push((EventType::FocuserMoveCompleted, payload, new.id.clone()));
    }

    if let (Some(old), Some(new)) = (&before.filter, &after.filter)
        && old.position != new.position
        && let Some(Some((position, name))) = &new.position
    {
        // Arriving at a filter is the change; leaving one is not.
        events.push((
            EventType::FilterChanged,
            EventPayload::FilterPosition {
                position: *position,
                name: name.clone(),
            },
            new.id.clone(),
        ));
    }
    events
}

/// Whether the server is answering, and what to tell the bus when that
/// changes.
#[derive(Debug)]
struct Health {
    interval: Duration,
    failures: u32,
    disconnected: bool,
}

/// What a poll's outcome means for the bus and for the next wait.
#[derive(Debug, PartialEq)]
enum HealthEvent {
    /// The poll failed: report the error.
    Error,
    /// Enough polls have failed: the instrument is gone.
    ErrorAndDisconnected,
    /// The server answers again after being gone.
    Reconnected,
    Nothing,
}

impl Health {
    fn new(interval: Duration) -> Self {
        Self {
            interval,
            failures: 0,
            disconnected: false,
        }
    }

    /// How long to wait before the next poll: the interval, doubled for each
    /// failure in a row, up to [`MAX_BACKOFF`].
    fn delay(&self) -> Duration {
        let factor = 2u32.saturating_pow(self.failures.min(16));
        self.interval
            .saturating_mul(factor)
            .min(MAX_BACKOFF.max(self.interval))
    }

    fn failed(&mut self) -> HealthEvent {
        self.failures = self.failures.saturating_add(1);
        if self.failures == FAILURES_TO_DISCONNECT && !self.disconnected {
            self.disconnected = true;
            HealthEvent::ErrorAndDisconnected
        } else if self.failures == 1 {
            HealthEvent::Error
        } else {
            // One error per outage is enough.
            HealthEvent::Nothing
        }
    }

    fn succeeded(&mut self) -> HealthEvent {
        self.failures = 0;
        if std::mem::take(&mut self.disconnected) {
            HealthEvent::Reconnected
        } else {
            HealthEvent::Nothing
        }
    }
}

/// Poll `watched` every `interval` and report changes on `bus`, until the
/// task is aborted.
pub(crate) async fn run(watched: Watched, bus: Arc<UnifiedEventBus>, interval: Duration) {
    let backend_event = |event_type, payload| {
        bus.emit(UnifiedEvent::new(
            watched.backend_id.clone(),
            event_type,
            payload,
        ));
    };
    let mut health = Health::new(interval);
    // The first good poll is the baseline: it says the instrument is there
    // and nothing else.
    let mut last: Option<Snapshot> = None;
    loop {
        match watched.snapshot().await {
            Ok(snapshot) => {
                let recovered = health.succeeded() == HealthEvent::Reconnected;
                match &last {
                    None => backend_event(EventType::Connected, EventPayload::None),
                    Some(_) if recovered => backend_event(EventType::Connected, EventPayload::None),
                    Some(before) => {
                        for (event_type, payload, source) in changes(before, &snapshot) {
                            let mut event =
                                UnifiedEvent::new(watched.backend_id.clone(), event_type, payload);
                            event.source = Some(source);
                            bus.emit(event);
                        }
                    }
                }
                last = Some(snapshot);
            }
            Err(e) => {
                let outcome = health.failed();
                if outcome != HealthEvent::Nothing {
                    tracing::warn!(backend = %watched.backend_id, error = %e, "poll failed");
                    backend_event(
                        EventType::Error,
                        EventPayload::Error {
                            code: -1,
                            message: e.to_string(),
                            kind: Some("poll".into()),
                        },
                    );
                }
                if outcome == HealthEvent::ErrorAndDisconnected {
                    backend_event(EventType::Disconnected, EventPayload::None);
                }
            }
        }
        tokio::time::sleep(health.delay()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn id(suffix: &str) -> DeviceId {
        DeviceId::new(format!("alpaca:test:{suffix}"))
    }

    fn mount(slewing: bool, parked: bool, tracking: bool) -> Snapshot {
        Snapshot {
            mount: Some(MountSnapshot {
                id: id("telescope:0"),
                slewing: Some(slewing),
                parked: Some(parked),
                tracking: Some(tracking),
                position: Some(Coordinates::from_hours(6.0, 20.0).unwrap()),
            }),
            ..Snapshot::default()
        }
    }

    fn types(events: &[Change]) -> Vec<EventType> {
        events.iter().map(|(t, _, _)| t.clone()).collect()
    }

    #[test]
    fn nothing_changed_nothing_emitted() {
        let state = mount(false, false, true);
        assert!(changes(&state, &state).is_empty());
    }

    #[test]
    fn a_slew_is_one_start_and_one_completion_with_the_position() {
        let idle = mount(false, false, true);
        let slewing = mount(true, false, true);
        assert_eq!(types(&changes(&idle, &slewing)), [EventType::SlewStarted]);
        assert!(changes(&slewing, &slewing).is_empty());

        let done = changes(&slewing, &idle);
        assert_eq!(types(&done), [EventType::SlewCompleted]);
        assert!(matches!(done[0].1, EventPayload::Coordinates(_)));
        assert_eq!(done[0].2, id("telescope:0"));
    }

    #[test]
    fn park_unpark_and_tracking_changes_are_events() {
        let unparked = mount(false, false, true);
        let parked = mount(false, true, false);
        let events = changes(&unparked, &parked);
        assert_eq!(
            types(&events),
            [EventType::Parked, EventType::TrackingChanged]
        );
        assert!(matches!(events[1].1, EventPayload::Tracking(false)));
        assert_eq!(
            types(&changes(&parked, &unparked)),
            [EventType::Unparked, EventType::TrackingChanged]
        );
    }

    #[test]
    fn a_state_the_mount_cannot_report_is_never_a_change() {
        let mut unknown = mount(false, false, true);
        unknown.mount.as_mut().unwrap().parked = None;
        let parked = mount(false, true, true);
        // Unknown to parked, and parked to unknown: neither is a transition.
        assert!(changes(&unknown, &parked).is_empty());
        assert!(changes(&parked, &unknown).is_empty());
    }

    fn camera(exposing: bool, image_ready: bool) -> Snapshot {
        Snapshot {
            cameras: vec![CameraSnapshot {
                id: id("camera:0"),
                exposing: Some(exposing),
                image_ready: Some(image_ready),
            }],
            ..Snapshot::default()
        }
    }

    #[test]
    fn an_exposure_starts_and_ends_with_an_image() {
        let idle = camera(false, false);
        let exposing = camera(true, false);
        let ready = camera(false, true);
        assert_eq!(
            types(&changes(&idle, &exposing)),
            [EventType::ExposureStarted]
        );
        assert_eq!(
            types(&changes(&exposing, &ready)),
            [EventType::ExposureCompleted, EventType::ImageReady]
        );
        // Starting the next exposure clears the image; that is not an event.
        assert_eq!(
            types(&changes(&ready, &exposing)),
            [EventType::ExposureStarted]
        );
        // An exposure shorter than the poll interval is still seen to end.
        assert_eq!(
            types(&changes(&idle, &ready)),
            [EventType::ExposureCompleted, EventType::ImageReady]
        );
    }

    fn focuser(moving: bool, position: i32) -> Snapshot {
        Snapshot {
            focuser: Some(FocuserSnapshot {
                id: id("focuser:0"),
                moving: Some(moving),
                position: Some(position),
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_focuser_move_is_reported_when_it_ends_with_where_it_ended() {
        assert!(changes(&focuser(false, 100), &focuser(true, 150)).is_empty());
        let done = changes(&focuser(true, 150), &focuser(false, 200));
        assert_eq!(types(&done), [EventType::FocuserMoveCompleted]);
        assert!(matches!(done[0].1, EventPayload::FocuserPosition(200)));
    }

    fn filter(position: Option<u32>) -> Snapshot {
        Snapshot {
            filter: Some(FilterSnapshot {
                id: id("filterwheel:0"),
                position: Some(position.map(|p| (p, Some(format!("F{p}"))))),
            }),
            ..Snapshot::default()
        }
    }

    #[test]
    fn a_filter_change_is_reported_on_arrival_not_departure() {
        // Leaving filter 0: the wheel has no position while it turns.
        assert!(changes(&filter(Some(0)), &filter(None)).is_empty());
        let arrived = changes(&filter(None), &filter(Some(2)));
        assert_eq!(types(&arrived), [EventType::FilterChanged]);
        match &arrived[0].1 {
            EventPayload::FilterPosition { position, name } => {
                assert_eq!(*position, 2);
                assert_eq!(name.as_deref(), Some("F2"));
            }
            other => panic!("got {other:?}"),
        }
        // A change quicker than the poll interval is still one event.
        assert_eq!(
            types(&changes(&filter(Some(0)), &filter(Some(3)))),
            [EventType::FilterChanged]
        );
        assert!(changes(&filter(Some(3)), &filter(Some(3))).is_empty());
    }

    #[test]
    fn one_error_per_outage_and_disconnected_after_three_failures() {
        let mut health = Health::new(Duration::from_secs(1));
        assert_eq!(health.failed(), HealthEvent::Error);
        assert_eq!(health.failed(), HealthEvent::Nothing);
        assert_eq!(health.failed(), HealthEvent::ErrorAndDisconnected);
        assert_eq!(health.failed(), HealthEvent::Nothing);
        assert_eq!(health.failed(), HealthEvent::Nothing);

        assert_eq!(health.succeeded(), HealthEvent::Reconnected);
        assert_eq!(health.succeeded(), HealthEvent::Nothing);
        // A short blip that never reached three failures is not a reconnect.
        assert_eq!(health.failed(), HealthEvent::Error);
        assert_eq!(health.succeeded(), HealthEvent::Nothing);
    }

    #[test]
    fn the_wait_doubles_with_each_failure_up_to_thirty_seconds_and_resets() {
        let mut health = Health::new(Duration::from_secs(1));
        assert_eq!(health.delay(), Duration::from_secs(1));
        let mut delays = Vec::new();
        for _ in 0..7 {
            health.failed();
            delays.push(health.delay().as_secs());
        }
        assert_eq!(delays, [2, 4, 8, 16, 30, 30, 30]);
        health.succeeded();
        assert_eq!(health.delay(), Duration::from_secs(1));
    }

    #[test]
    fn a_poll_interval_longer_than_the_cap_is_kept() {
        let mut health = Health::new(Duration::from_secs(60));
        health.failed();
        assert_eq!(health.delay(), Duration::from_secs(60));
    }
}
