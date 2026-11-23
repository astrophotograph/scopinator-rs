//! Scripted devices for tests of code written against the device traits.

use std::collections::VecDeque;
use std::sync::Mutex;

use async_trait::async_trait;
use scopinator_types::{
    Coordinates, DeviceId, DeviceInfo, DeviceKind, ExposureSettings, FrameKind, ImageData,
    PixelFormat,
};

use super::capabilities::{CameraCapabilities, MountCapabilities};
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
}

mock_device!(MockFocuser, DeviceKind::Focuser, "focuser");

#[async_trait]
impl Focuser for MockFocuser {
    async fn get_position(&self) -> Result<i32, ScopinatorError> {
        Ok(*self.position.lock().unwrap())
    }

    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError> {
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

/// A filter wheel that answers `is_moving` from a script.
#[derive(Debug, Default)]
pub(crate) struct MockFilterWheel {
    pub(crate) moving: Script,
    pub(crate) position: Mutex<u32>,
}

mock_device!(MockFilterWheel, DeviceKind::FilterWheel, "filter wheel");

#[async_trait]
impl FilterWheel for MockFilterWheel {
    async fn get_position(&self) -> Result<u32, ScopinatorError> {
        Ok(*self.position.lock().unwrap())
    }

    async fn set_position(&self, position: u32) -> Result<(), ScopinatorError> {
        *self.position.lock().unwrap() = position;
        Ok(())
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        self.moving.next()
    }

    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError> {
        Ok(vec!["L".into(), "R".into(), "G".into(), "B".into()])
    }
}
