use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use scopinator_alpaca::AlpacaSwitch;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, SwitchInfo};

use super::{neutral, optional};
use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, Switch, check_switch_value};
use crate::error::ScopinatorError;

/// An Alpaca `switch` device as a neutral switch: its outputs and sensors
/// by the ids the device gives them, from zero.
pub struct AlpacaSwitchDevice {
    switch: AlpacaSwitch,
    id: DeviceId,
    connected: AtomicBool,
    profile: RwLock<Profile>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    name: Option<String>,
    driver: Option<String>,
    /// What each switch is, read when the device is connected.
    switches: Vec<SwitchInfo>,
}

impl AlpacaSwitchDevice {
    /// The switch device `switch` on the server whose backend id is
    /// `backend_id`. Nothing is sent until [`connect`](Device::connect).
    pub fn new(switch: AlpacaSwitch, backend_id: &DeviceId) -> Self {
        let id = DeviceId::new(format!("{backend_id}:switch:{}", switch.device_number()));
        Self {
            switch,
            id,
            connected: AtomicBool::new(false),
            profile: RwLock::new(Profile::default()),
        }
    }

    /// The typed Alpaca client.
    pub fn switch(&self) -> &AlpacaSwitch {
        &self.switch
    }

    fn profile(&self) -> Profile {
        self.profile
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    async fn read_profile(&self) -> Result<Profile, ScopinatorError> {
        let switch = &self.switch;
        let count = switch.max_switch().await.map_err(neutral)?;
        let mut switches = Vec::new();
        for id in 0..u32::try_from(count).unwrap_or(0) {
            let name = optional(switch.switch_name(id).await)?
                .filter(|name| !name.is_empty())
                .unwrap_or_else(|| format!("Switch {id}"));
            // A device that does not say is taken to be on/off.
            let min = optional(switch.min_switch_value(id).await)?.unwrap_or(0.0);
            let max = optional(switch.max_switch_value(id).await)?.unwrap_or(1.0);
            let step = optional(switch.switch_step(id).await)?.unwrap_or(1.0);
            let mut info = SwitchInfo::ranged(id, name, min, max, step);
            if let Some(description) =
                optional(switch.switch_description(id).await)?.filter(|text| !text.is_empty())
            {
                info = info.with_description(description);
            }
            if optional(switch.can_write(id).await)? == Some(false) {
                info = info.read_only();
            }
            switches.push(info);
        }
        Ok(Profile {
            name: optional(switch.name().await)?,
            driver: optional(switch.driver_info().await)?,
            switches,
        })
    }

    fn known(&self, index: u32) -> Result<SwitchInfo, ScopinatorError> {
        self.profile()
            .switches
            .into_iter()
            .find(|info| info.index == index)
            .ok_or_else(|| ScopinatorError::OutOfRange(format!("there is no switch {index}")))
    }
}

#[async_trait]
impl Device for AlpacaSwitchDevice {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let profile = self.profile();
        let mut info = DeviceInfo::new(
            self.id.clone(),
            DeviceKind::Switch,
            profile.name.unwrap_or_else(|| "Alpaca switch".to_string()),
        );
        info.driver = profile.driver;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.switch.set_connected(true).await.map_err(neutral)?;
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
        self.switch.set_connected(false).await.map_err(neutral)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        let profile = self.profile();
        Ok(DeviceStatus {
            connected: self.switch.connected().await.map_err(neutral)?,
            name: profile.name,
            description: profile.driver,
        })
    }
}

#[async_trait]
impl Switch for AlpacaSwitchDevice {
    /// The switches as they were when the device was connected. Names and
    /// ranges are not read again.
    async fn switches(&self) -> Result<Vec<SwitchInfo>, ScopinatorError> {
        Ok(self.profile().switches)
    }

    async fn get(&self, index: u32) -> Result<f64, ScopinatorError> {
        let info = self.known(index)?;
        match optional(self.switch.switch_value(index).await)? {
            Some(value) => Ok(value),
            // A device with only the on/off call.
            None => Ok(if self.switch.switch(index).await.map_err(neutral)? {
                info.max
            } else {
                info.min
            }),
        }
    }

    async fn set(&self, index: u32, value: f64) -> Result<(), ScopinatorError> {
        check_switch_value(&self.known(index)?, value)?;
        self.switch
            .set_switch_value(index, value)
            .await
            .map_err(neutral)
    }

    async fn switch_info(&self, index: u32) -> Result<SwitchInfo, ScopinatorError> {
        self.known(index)
    }
}
