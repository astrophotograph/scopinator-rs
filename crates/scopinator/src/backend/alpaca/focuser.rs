use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use scopinator_alpaca::AlpacaFocuser;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, FocuserPosition};

use super::{neutral, optional};
use crate::device::capabilities::FocuserCapabilities;
use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, Focuser};
use crate::error::ScopinatorError;

/// An Alpaca `focuser` device as a neutral focuser.
pub struct AlpacaFocuserDevice {
    focuser: AlpacaFocuser,
    id: DeviceId,
    connected: AtomicBool,
    profile: RwLock<Profile>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    capabilities: FocuserCapabilities,
    name: Option<String>,
    driver: Option<String>,
}

impl AlpacaFocuserDevice {
    /// The focuser `focuser` on the server whose backend id is
    /// `backend_id`. Nothing is sent until [`connect`](Device::connect).
    pub fn new(focuser: AlpacaFocuser, backend_id: &DeviceId) -> Self {
        let id = DeviceId::new(format!("{backend_id}:focuser:{}", focuser.device_number()));
        Self {
            focuser,
            id,
            connected: AtomicBool::new(false),
            profile: RwLock::new(Profile::default()),
        }
    }

    /// The typed Alpaca client.
    pub fn focuser(&self) -> &AlpacaFocuser {
        &self.focuser
    }

    fn profile(&self) -> Profile {
        self.profile
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn read_profile(&self) -> Result<Profile, ScopinatorError> {
        let focuser = &self.focuser;
        let capabilities = FocuserCapabilities {
            absolute: optional(focuser.absolute().await)?.unwrap_or(false),
            max_position: optional(focuser.max_step().await)?,
            max_increment: optional(focuser.max_increment().await)?,
            step_size_um: optional(focuser.step_size().await)?,
            has_temperature: optional(focuser.temperature().await)?.is_some(),
            can_temp_compensate: optional(focuser.temp_comp_available().await)?.unwrap_or(false),
        };
        Ok(Profile {
            capabilities,
            name: optional(focuser.name().await)?,
            driver: optional(focuser.driver_info().await)?,
        })
    }
}

#[async_trait]
impl Device for AlpacaFocuserDevice {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let profile = self.profile();
        let mut info = DeviceInfo::new(
            self.id.clone(),
            DeviceKind::Focuser,
            profile.name.unwrap_or_else(|| "Alpaca focuser".to_string()),
        );
        info.driver = profile.driver;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.focuser.set_connected(true).await.map_err(neutral)?;
        let profile = self.read_profile().await?;
        *self
            .profile
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        self.connected.store(false, Ordering::SeqCst);
        self.focuser.set_connected(false).await.map_err(neutral)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        let profile = self.profile();
        Ok(DeviceStatus {
            connected: self.focuser.connected().await.map_err(neutral)?,
            name: profile.name,
            description: profile.driver,
        })
    }
}

#[async_trait]
impl Focuser for AlpacaFocuserDevice {
    fn capabilities(&self) -> FocuserCapabilities {
        self.profile().capabilities
    }

    async fn focuser_position(&self) -> Result<FocuserPosition, ScopinatorError> {
        let caps = self.profile().capabilities;
        Ok(FocuserPosition {
            // A relative focuser has no position; Alpaca makes reading it an
            // error.
            position: if caps.absolute {
                self.focuser.position().await.map_err(neutral)?
            } else {
                0
            },
            max_position: caps.max_position,
            temperature: optional(self.focuser.temperature().await)?,
            is_moving: self.focuser.is_moving().await.map_err(neutral)?,
        })
    }

    async fn move_to(&self, position: i32) -> Result<(), ScopinatorError> {
        let caps = self.profile().capabilities;
        if !caps.absolute {
            return Err(ScopinatorError::NotSupported(
                "an absolute move on a relative focuser".into(),
            ));
        }
        if position < 0 || caps.max_position.is_some_and(|max| position > max) {
            return Err(ScopinatorError::OutOfRange(format!(
                "focuser position {position} (travel is 0 to {})",
                caps.max_position
                    .map_or_else(|| "unknown".to_string(), |max| max.to_string())
            )));
        }
        self.focuser.move_to(position).await.map_err(neutral)
    }

    /// On an absolute focuser, a move to the current position plus
    /// `offset`; on a relative one, a move by `offset` steps.
    async fn move_relative(&self, offset: i32) -> Result<(), ScopinatorError> {
        let caps = self.profile().capabilities;
        if caps.absolute {
            let position = self.focuser.position().await.map_err(neutral)?;
            let target = position.checked_add(offset).ok_or_else(|| {
                ScopinatorError::OutOfRange(format!("focuser move of {offset} from {position}"))
            })?;
            return self.move_to(target).await;
        }
        if caps
            .max_increment
            .is_some_and(|max| offset.unsigned_abs() > max.unsigned_abs())
        {
            return Err(ScopinatorError::OutOfRange(format!(
                "focuser move of {offset} steps (at most {} at a time)",
                caps.max_increment.unwrap_or_default()
            )));
        }
        self.focuser.move_to(offset).await.map_err(neutral)
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        self.focuser.is_moving().await.map_err(neutral)
    }

    async fn halt(&self) -> Result<(), ScopinatorError> {
        self.focuser.halt().await.map_err(neutral)
    }
}
