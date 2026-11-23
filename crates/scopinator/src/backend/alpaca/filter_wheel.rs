use std::sync::RwLock;
use std::sync::atomic::{AtomicBool, Ordering};

use async_trait::async_trait;
use scopinator_alpaca::AlpacaFilterWheel;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, FilterPosition};

use super::{neutral, optional};
use crate::device::capabilities::FilterWheelCapabilities;
use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, FilterWheel};
use crate::error::ScopinatorError;

/// An Alpaca `filterwheel` device as a neutral filter wheel. Alpaca counts
/// filter positions from zero, as the neutral trait does.
pub struct AlpacaFilterWheelDevice {
    wheel: AlpacaFilterWheel,
    id: DeviceId,
    connected: AtomicBool,
    profile: RwLock<Profile>,
}

#[derive(Debug, Clone, Default)]
struct Profile {
    capabilities: FilterWheelCapabilities,
    /// Filter names in position order, as read at connect.
    names: Vec<String>,
    name: Option<String>,
    driver: Option<String>,
}

impl AlpacaFilterWheelDevice {
    /// The filter wheel `wheel` on the server whose backend id is
    /// `backend_id`. Nothing is sent until [`connect`](Device::connect).
    pub fn new(wheel: AlpacaFilterWheel, backend_id: &DeviceId) -> Self {
        let id = DeviceId::new(format!(
            "{backend_id}:filterwheel:{}",
            wheel.device_number()
        ));
        Self {
            wheel,
            id,
            connected: AtomicBool::new(false),
            profile: RwLock::new(Profile::default()),
        }
    }

    /// The typed Alpaca client.
    pub fn filter_wheel(&self) -> &AlpacaFilterWheel {
        &self.wheel
    }

    fn profile(&self) -> Profile {
        self.profile
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl Device for AlpacaFilterWheelDevice {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let profile = self.profile();
        let mut info = DeviceInfo::new(
            self.id.clone(),
            DeviceKind::FilterWheel,
            profile
                .name
                .unwrap_or_else(|| "Alpaca filter wheel".to_string()),
        );
        info.driver = profile.driver;
        info
    }

    /// Set the device's `Connected` property and read its filter names.
    async fn connect(&self) -> Result<(), ScopinatorError> {
        self.wheel.set_connected(true).await.map_err(neutral)?;
        let names = self.wheel.names().await.map_err(neutral)?;
        let offsets = optional(self.wheel.focus_offsets().await)?;
        let profile = Profile {
            capabilities: FilterWheelCapabilities {
                slot_count: names.len() as u32,
                has_focus_offsets: offsets.is_some(),
            },
            names,
            name: optional(self.wheel.name().await)?,
            driver: optional(self.wheel.driver_info().await)?,
        };
        *self
            .profile
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = profile;
        self.connected.store(true, Ordering::SeqCst);
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        self.connected.store(false, Ordering::SeqCst);
        self.wheel.set_connected(false).await.map_err(neutral)
    }

    fn is_connected(&self) -> bool {
        self.connected.load(Ordering::SeqCst)
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        let profile = self.profile();
        Ok(DeviceStatus {
            connected: self.wheel.connected().await.map_err(neutral)?,
            name: profile.name,
            description: profile.driver,
        })
    }
}

#[async_trait]
impl FilterWheel for AlpacaFilterWheelDevice {
    fn capabilities(&self) -> FilterWheelCapabilities {
        self.profile().capabilities
    }

    async fn filter_position(&self) -> Result<Option<FilterPosition>, ScopinatorError> {
        let Some(position) = self.wheel.position().await.map_err(neutral)? else {
            return Ok(None);
        };
        Ok(Some(FilterPosition {
            position: u32::from(position),
            name: self.profile().names.get(usize::from(position)).cloned(),
        }))
    }

    async fn set_filter_position(&self, position: u32) -> Result<(), ScopinatorError> {
        let slots = self.profile().capabilities.slot_count;
        let out_of_range = || {
            ScopinatorError::OutOfRange(format!(
                "filter {position} (the wheel has positions 0 to {})",
                slots.saturating_sub(1)
            ))
        };
        if position >= slots {
            return Err(out_of_range());
        }
        let position = u16::try_from(position).map_err(|_| out_of_range())?;
        self.wheel.set_position(position).await.map_err(neutral)
    }

    /// Alpaca reports a turning wheel by having no position.
    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        Ok(self.wheel.position().await.map_err(neutral)?.is_none())
    }

    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError> {
        self.wheel.names().await.map_err(neutral)
    }

    async fn get_focus_offsets(&self) -> Result<Vec<i32>, ScopinatorError> {
        self.wheel.focus_offsets().await.map_err(neutral)
    }
}
