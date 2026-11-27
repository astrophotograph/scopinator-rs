//! Scripted devices for tests of code written against the device traits.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use scopinator_types::{
    AcquisitionState, AutoFocusState, Coordinates, DeviceId, DeviceInfo, DeviceKind,
    ExposureSettings, FilterPosition, FocuserPosition, FrameKind, ImageData, PixelFormat,
    PlateSolution, Readiness, StackProgress, StackSettings, Target,
};

use super::capabilities::{
    CameraCapabilities, FilterWheelCapabilities, FocuserCapabilities, MountCapabilities,
};
use super::smart::{
    AutoFocus, LiveStacker, Observer, ObserverCapabilities, PlateSolver, StackerCapabilities,
};
use super::status::{CameraStatus, DeviceStatus, MountStatus};
use super::traits::{Camera, Device, FilterWheel, Focuser, Mount};
use crate::error::ScopinatorError;

/// One answer of a state query.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Answer {
    Is(bool),
    /// `ScopinatorError::Backend` with this text.
    Fails(&'static str),
    NotSupported,
}

/// The answers a state query gives, in order. The last one repeats. With no
/// script the answer is `false`.
#[derive(Debug, Default)]
pub(crate) struct Script {
    answers: Mutex<VecDeque<Answer>>,
    asked: Mutex<usize>,
}

impl Script {
    pub(crate) fn script(&self, answers: impl IntoIterator<Item = Answer>) {
        *self.answers.lock().unwrap() = answers.into_iter().collect();
    }

    /// How many times the query has been asked.
    pub(crate) fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }

    fn next(&self) -> Result<bool, ScopinatorError> {
        *self.asked.lock().unwrap() += 1;
        let mut answers = self.answers.lock().unwrap();
        let answer = if answers.len() > 1 {
            answers.pop_front()
        } else {
            answers.front().copied()
        };
        match answer.unwrap_or(Answer::Is(false)) {
            Answer::Is(value) => Ok(value),
            Answer::Fails(text) => Err(ScopinatorError::Backend(text.into())),
            Answer::NotSupported => Err(ScopinatorError::NotSupported("mock".into())),
        }
    }
}

macro_rules! mock_device {
    ($type:ty, $kind:expr, $name:literal) => {
        #[async_trait]
        impl Device for $type {
            fn id(&self) -> DeviceId {
                DeviceId::new(concat!("mock:", $name))
            }

            fn info(&self) -> DeviceInfo {
                DeviceInfo::new(self.id(), $kind, $name)
            }

            async fn connect(&self) -> Result<(), ScopinatorError> {
                Ok(())
            }

            async fn disconnect(&self) -> Result<(), ScopinatorError> {
                Ok(())
            }

            fn is_connected(&self) -> bool {
                true
            }

            async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
                Ok(DeviceStatus {
                    connected: true,
                    ..DeviceStatus::default()
                })
            }
        }
    };
}

/// A mount that records the slews it is sent and answers its state queries
/// from scripts.
#[derive(Debug, Default)]
pub(crate) struct MockMount {
    pub(crate) slewing: Script,
    pub(crate) parked: Script,
    pub(crate) slews: Mutex<Vec<Coordinates>>,
    /// Refuse slews with this error text.
    pub(crate) refuse_slew: Mutex<Option<&'static str>>,
}

mock_device!(MockMount, DeviceKind::Mount, "mount");

#[async_trait]
impl Mount for MockMount {
    async fn get_coordinates(&self) -> Result<Coordinates, ScopinatorError> {
        self.slews
            .lock()
            .unwrap()
            .last()
            .copied()
            .ok_or_else(|| ScopinatorError::InvalidState("never slewed".into()))
    }

    async fn slew_to_coordinates(&self, coords: &Coordinates) -> Result<(), ScopinatorError> {
        if let Some(text) = *self.refuse_slew.lock().unwrap() {
            return Err(ScopinatorError::Backend(text.into()));
        }
        self.slews.lock().unwrap().push(*coords);
        Ok(())
    }

    async fn abort_slew(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn is_slewing(&self) -> Result<bool, ScopinatorError> {
        self.slewing.next()
    }

    async fn is_parked(&self) -> Result<bool, ScopinatorError> {
        self.parked.next()
    }

    async fn park(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn set_tracking(&self, _enabled: bool) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn is_tracking(&self) -> Result<bool, ScopinatorError> {
        Ok(false)
    }

    fn capabilities(&self) -> MountCapabilities {
        MountCapabilities {
            can_slew: true,
            can_park: true,
            ..MountCapabilities::default()
        }
    }

    async fn get_mount_status(&self) -> Result<MountStatus, ScopinatorError> {
        Ok(MountStatus::default())
    }
}

/// A camera that counts exposures and answers `image_ready` from a script.
#[derive(Debug, Default)]
pub(crate) struct MockCamera {
    pub(crate) ready: Script,
    pub(crate) exposures: Mutex<Vec<ExposureSettings>>,
    pub(crate) aborts: Mutex<usize>,
}

mock_device!(MockCamera, DeviceKind::Camera, "camera");

#[async_trait]
impl Camera for MockCamera {
    async fn start_exposure(&self, settings: &ExposureSettings) -> Result<(), ScopinatorError> {
        self.exposures.lock().unwrap().push(settings.clone());
        Ok(())
    }

    async fn abort_exposure(&self) -> Result<(), ScopinatorError> {
        *self.aborts.lock().unwrap() += 1;
        Ok(())
    }

    async fn image_ready(&self) -> Result<bool, ScopinatorError> {
        self.ready.next()
    }

    async fn get_image(&self) -> Result<ImageData, ScopinatorError> {
        Ok(ImageData::new(
            1,
            1,
            bytes::Bytes::from_static(&[0]),
            PixelFormat::Mono8,
            FrameKind::Preview,
        ))
    }

    async fn is_exposing(&self) -> Result<bool, ScopinatorError> {
        Ok(false)
    }

    fn capabilities(&self) -> CameraCapabilities {
        CameraCapabilities {
            can_expose: true,
            ..CameraCapabilities::default()
        }
    }

    async fn get_camera_status(&self) -> Result<CameraStatus, ScopinatorError> {
        Ok(CameraStatus::default())
    }
}

/// A focuser that answers `is_moving` from a script.
#[derive(Debug, Default)]
pub(crate) struct MockFocuser {
    pub(crate) moving: Script,
    pub(crate) position: Mutex<i32>,
    /// A focuser that only moves by relative amounts.
    pub(crate) relative_only: bool,
}

mock_device!(MockFocuser, DeviceKind::Focuser, "focuser");

#[async_trait]
impl Focuser for MockFocuser {
    fn capabilities(&self) -> FocuserCapabilities {
        FocuserCapabilities {
            absolute: !self.relative_only,
            max_position: (!self.relative_only).then_some(10_000),
            ..FocuserCapabilities::default()
        }
    }

    async fn focuser_position(&self) -> Result<FocuserPosition, ScopinatorError> {
        Ok(FocuserPosition {
            position: *self.position.lock().unwrap(),
            max_position: self.capabilities().max_position,
            temperature: None,
            is_moving: false,
        })
    }

    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError> {
        if self.relative_only {
            return Err(ScopinatorError::NotSupported("absolute move".into()));
        }
        *self.position.lock().unwrap() = position;
        Ok(())
    }

    async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError> {
        *self.position.lock().unwrap() += offset;
        Ok(())
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        self.moving.next()
    }

    async fn halt(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }
}

/// A four-slot filter wheel that answers `is_moving` from a script.
#[derive(Debug, Default)]
pub(crate) struct MockFilterWheel {
    pub(crate) moving: Script,
    pub(crate) position: Mutex<u32>,
    /// Report the wheel as between filters.
    pub(crate) turning: Mutex<bool>,
}

const MOCK_FILTERS: [&str; 4] = ["L", "R", "G", "B"];

mock_device!(MockFilterWheel, DeviceKind::FilterWheel, "filter wheel");

#[async_trait]
impl FilterWheel for MockFilterWheel {
    fn capabilities(&self) -> FilterWheelCapabilities {
        FilterWheelCapabilities {
            slot_count: MOCK_FILTERS.len() as u32,
            ..FilterWheelCapabilities::default()
        }
    }

    async fn filter_position(&self) -> Result<Option<FilterPosition>, ScopinatorError> {
        if *self.turning.lock().unwrap() {
            return Ok(None);
        }
        let position = *self.position.lock().unwrap();
        Ok(Some(FilterPosition {
            position,
            name: MOCK_FILTERS.get(position as usize).map(|n| n.to_string()),
        }))
    }

    async fn set_filter_position(&self, position: u32) -> Result<(), ScopinatorError> {
        if position as usize >= MOCK_FILTERS.len() {
            return Err(ScopinatorError::OutOfRange(format!(
                "filter {position} of {}",
                MOCK_FILTERS.len()
            )));
        }
        *self.position.lock().unwrap() = position;
        Ok(())
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        self.moving.next()
    }

    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError> {
        Ok(MOCK_FILTERS.iter().map(|n| n.to_string()).collect())
    }
}

/// The answers a state query gives, in order, for a query that returns a
/// value. The last one repeats.
#[derive(Debug)]
pub(crate) struct Sequence<T> {
    answers: Mutex<VecDeque<Result<T, &'static str>>>,
    asked: Mutex<usize>,
}

impl<T> Default for Sequence<T> {
    fn default() -> Self {
        Self {
            answers: Mutex::new(VecDeque::new()),
            asked: Mutex::new(0),
        }
    }
}

impl<T: Clone> Sequence<T> {
    pub(crate) fn script(&self, answers: impl IntoIterator<Item = Result<T, &'static str>>) {
        *self.answers.lock().unwrap() = answers.into_iter().collect();
    }

    pub(crate) fn asked(&self) -> usize {
        *self.asked.lock().unwrap()
    }

    /// The next answer, or `fallback` if nothing was scripted. An `Err` is a
    /// `ScopinatorError::Backend` with that text.
    fn next(&self, fallback: T) -> Result<T, ScopinatorError> {
        *self.asked.lock().unwrap() += 1;
        let mut answers = self.answers.lock().unwrap();
        let answer = if answers.len() > 1 {
            answers.pop_front()
        } else {
            answers.front().cloned()
        };
        answer
            .unwrap_or(Ok(fallback))
            .map_err(|text| ScopinatorError::Backend(text.into()))
    }
}

/// An observer that answers its state queries from scripts and records the
/// targets it is sent.
#[derive(Debug, Default)]
pub(crate) struct MockObserver {
    pub(crate) readiness: Sequence<Readiness>,
    pub(crate) acquisition: Sequence<AcquisitionState>,
    pub(crate) targets: Mutex<Vec<Target>>,
}

#[async_trait]
impl Observer for MockObserver {
    fn capabilities(&self) -> ObserverCapabilities {
        ObserverCapabilities {
            sky_targets: true,
            ..ObserverCapabilities::default()
        }
    }

    async fn readiness(&self) -> Result<Readiness, ScopinatorError> {
        self.readiness.next(Readiness::Ready)
    }

    async fn prepare(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn acquire(&self, target: &Target) -> Result<(), ScopinatorError> {
        self.targets.lock().unwrap().push(target.clone());
        Ok(())
    }

    async fn acquisition(&self) -> Result<AcquisitionState, ScopinatorError> {
        self.acquisition.next(AcquisitionState::OnTarget)
    }

    async fn stop(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }
}

/// A stacker that answers `progress` from a script and records how it was
/// started.
#[derive(Debug, Default)]
pub(crate) struct MockStacker {
    pub(crate) progress: Sequence<StackProgress>,
    pub(crate) started: Mutex<Vec<StackSettings>>,
    pub(crate) stops: Mutex<usize>,
    /// Refuse a start that asks for a sub-exposure, as `NotSupported`.
    pub(crate) refuse_exposure: Mutex<bool>,
    /// Claim to stop by itself after a number of frames.
    pub(crate) limits_frames: Mutex<bool>,
}

#[async_trait]
impl LiveStacker for MockStacker {
    fn capabilities(&self) -> StackerCapabilities {
        StackerCapabilities {
            can_limit_frames: *self.limits_frames.lock().unwrap(),
            ..StackerCapabilities::default()
        }
    }

    async fn start_stacking(&self, settings: &StackSettings) -> Result<(), ScopinatorError> {
        if settings.exposure.is_some() && *self.refuse_exposure.lock().unwrap() {
            return Err(ScopinatorError::NotSupported("sub-exposure".into()));
        }
        self.started.lock().unwrap().push(settings.clone());
        Ok(())
    }

    async fn stop_stacking(&self) -> Result<(), ScopinatorError> {
        *self.stops.lock().unwrap() += 1;
        Ok(())
    }

    async fn progress(&self) -> Result<StackProgress, ScopinatorError> {
        self.progress.next(StackProgress::default())
    }

    async fn stacked_image(&self) -> Result<ImageData, ScopinatorError> {
        Ok(ImageData::new(
            1,
            1,
            bytes::Bytes::from_static(&[0]),
            PixelFormat::Mono8,
            FrameKind::Stack,
        ))
    }
}

/// An autofocus that answers its state from a script.
#[derive(Debug, Default)]
pub(crate) struct MockAutoFocus {
    pub(crate) state: Sequence<AutoFocusState>,
}

#[async_trait]
impl AutoFocus for MockAutoFocus {
    async fn start_autofocus(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn stop_autofocus(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn autofocus_state(&self) -> Result<AutoFocusState, ScopinatorError> {
        self.state.next(AutoFocusState::Idle)
    }
}

/// A plate solver that answers `solution` from a script.
#[derive(Debug, Default)]
pub(crate) struct MockSolver {
    pub(crate) solution: Sequence<Option<PlateSolution>>,
}

#[async_trait]
impl PlateSolver for MockSolver {
    async fn start_solve(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn solution(&self) -> Result<Option<PlateSolution>, ScopinatorError> {
        self.solution.next(None)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn a_relative_only_focuser_refuses_an_absolute_move() {
        let focuser = MockFocuser {
            relative_only: true,
            ..MockFocuser::default()
        };
        assert!(!Focuser::capabilities(&focuser).absolute);
        assert!(focuser.move_to(100).await.unwrap_err().is_not_supported());
        focuser.move_relative(25).await.unwrap();
        focuser.move_relative(-5).await.unwrap();
        let position = focuser.focuser_position().await.unwrap();
        assert_eq!(position.position, 20);
        assert_eq!(position.max_position, None);
    }

    #[tokio::test]
    async fn an_absolute_focuser_reports_its_travel() {
        let focuser = MockFocuser::default();
        assert!(Focuser::capabilities(&focuser).absolute);
        focuser.move_to(1500).await.unwrap();
        let position = focuser.focuser_position().await.unwrap();
        assert_eq!(position.position, 1500);
        assert_eq!(position.max_position, Some(10_000));
    }

    #[tokio::test]
    async fn filter_positions_count_from_zero_and_are_none_while_turning() {
        let wheel = MockFilterWheel::default();
        assert_eq!(FilterWheel::capabilities(&wheel).slot_count, 4);
        let first = wheel.filter_position().await.unwrap().unwrap();
        assert_eq!((first.position, first.name.as_deref()), (0, Some("L")));

        wheel.set_filter_position(3).await.unwrap();
        let last = wheel.filter_position().await.unwrap().unwrap();
        assert_eq!((last.position, last.name.as_deref()), (3, Some("B")));

        let err = wheel.set_filter_position(4).await.unwrap_err();
        assert!(matches!(err, ScopinatorError::OutOfRange(_)), "{err:?}");

        *wheel.turning.lock().unwrap() = true;
        assert_eq!(wheel.filter_position().await.unwrap(), None);
    }

    #[tokio::test]
    async fn a_wheel_without_focus_offsets_says_not_supported() {
        let wheel = MockFilterWheel::default();
        assert!(!FilterWheel::capabilities(&wheel).has_focus_offsets);
        assert!(
            wheel
                .get_focus_offsets()
                .await
                .unwrap_err()
                .is_not_supported()
        );
    }

    /// One object can be both a focuser and a filter wheel (a smart
    /// telescope's optical train); the position methods do not clash.
    #[tokio::test]
    async fn one_type_can_be_a_focuser_and_a_filter_wheel() {
        #[derive(Default)]
        struct Both {
            focuser: MockFocuser,
            wheel: MockFilterWheel,
        }
        mock_device!(Both, DeviceKind::Other, "both");

        #[async_trait]
        impl Focuser for Both {
            fn capabilities(&self) -> FocuserCapabilities {
                Focuser::capabilities(&self.focuser)
            }
            async fn focuser_position(&self) -> Result<FocuserPosition, ScopinatorError> {
                self.focuser.focuser_position().await
            }
            async fn move_to(&self, position: i32) -> Result<(), ScopinatorError> {
                self.focuser.move_to(position).await
            }
            async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError> {
                self.focuser.move_relative(offset).await
            }
            async fn is_moving(&self) -> Result<bool, ScopinatorError> {
                Focuser::is_moving(&self.focuser).await
            }
            async fn halt(&self) -> Result<(), ScopinatorError> {
                self.focuser.halt().await
            }
        }

        #[async_trait]
        impl FilterWheel for Both {
            fn capabilities(&self) -> FilterWheelCapabilities {
                FilterWheel::capabilities(&self.wheel)
            }
            async fn filter_position(&self) -> Result<Option<FilterPosition>, ScopinatorError> {
                self.wheel.filter_position().await
            }
            async fn set_filter_position(&self, position: u32) -> Result<(), ScopinatorError> {
                self.wheel.set_filter_position(position).await
            }
            async fn is_moving(&self) -> Result<bool, ScopinatorError> {
                FilterWheel::is_moving(&self.wheel).await
            }
            async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError> {
                self.wheel.get_filter_names().await
            }
        }

        let both = Both::default();
        both.move_to(700).await.unwrap();
        both.set_filter_position(2).await.unwrap();
        assert_eq!(both.focuser_position().await.unwrap().position, 700);
        assert_eq!(both.filter_position().await.unwrap().unwrap().position, 2);
    }
}
