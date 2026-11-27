use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, FilterPosition};
use serde_json::{Value, json};

use crate::device::capabilities::FilterWheelCapabilities;
use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, FilterWheel};
use crate::error::ScopinatorError;

/// How many positions a Seestar's wheel has, until the telescope has been
/// asked: the three every model so far has.
const USUAL_SLOTS: u32 = 3;

/// A Seestar's internal filter wheel.
///
/// The telescope counts its positions from zero, as the neutral trait does,
/// so no translation is needed. An S30 on firmware 9.75 names them `dark`
/// (the shutter, used for dark frames), `IRCUT` and `LP` (the light
/// pollution filter), in that order; the names are passed on as the
/// telescope gives them.
///
/// The telescope moves the wheel itself too: a view puts it on `IRCUT` or
/// `LP` according to its `lp_filter`, and dark frames put it on `dark`.
pub struct SeestarFilterWheel {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
    /// The names, kept from the first time they are asked for.
    names: OnceLock<Vec<String>>,
}

impl SeestarFilterWheel {
    pub(crate) fn new(client: Arc<SeestarClient>, device_id: DeviceId) -> Self {
        Self {
            client,
            device_id,
            names: OnceLock::new(),
        }
    }

    async fn names(&self) -> Result<Vec<String>, ScopinatorError> {
        if let Some(names) = self.names.get() {
            return Ok(names.clone());
        }
        let setting = self
            .client
            .send_raw_and_validate("get_wheel_setting", None)
            .await?;
        let names = wheel_names(&setting)?;
        Ok(self.names.get_or_init(|| names).clone())
    }
}

/// The filter names in a `get_wheel_setting` reply, in position order.
fn wheel_names(setting: &Value) -> Result<Vec<String>, ScopinatorError> {
    let names = setting
        .get("names")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            ScopinatorError::Backend(format!("get_wheel_setting reply has no `names`: {setting}"))
        })?;
    names
        .iter()
        .map(|name| {
            name.as_str().map(str::to_string).ok_or_else(|| {
                ScopinatorError::Backend(format!("a filter name is not a string: {setting}"))
            })
        })
        .collect()
}

/// True unless a `get_wheel_state` reply says the wheel is at rest.
fn wheel_moving(state: &Value) -> bool {
    state.get("state").and_then(Value::as_str) != Some("idle")
}

#[async_trait]
impl Device for SeestarFilterWheel {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info = DeviceInfo::new(
            self.device_id.clone(),
            DeviceKind::FilterWheel,
            "Seestar filter wheel",
        );
        info.model = summary.model;
        info.firmware = summary.firmware_string;
        info
    }

    async fn connect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        Ok(())
    }

    fn is_connected(&self) -> bool {
        self.client.is_control_connected()
    }

    async fn get_status(&self) -> Result<DeviceStatus, ScopinatorError> {
        Ok(DeviceStatus {
            connected: self.is_connected(),
            name: Some(self.device_id.to_string()),
            description: Some("Seestar filter wheel".into()),
        })
    }
}

#[async_trait]
impl FilterWheel for SeestarFilterWheel {
    /// The slot count is the telescope's once the names have been asked
    /// for, and three before that.
    fn capabilities(&self) -> FilterWheelCapabilities {
        FilterWheelCapabilities {
            slot_count: self
                .names
                .get()
                .map_or(USUAL_SLOTS, |names| names.len() as u32),
            ..FilterWheelCapabilities::default()
        }
    }

    async fn filter_position(&self) -> Result<Option<FilterPosition>, ScopinatorError> {
        if self.is_moving().await? {
            return Ok(None);
        }
        let reply = self
            .client
            .send_raw_and_validate("get_wheel_position", None)
            .await?;
        let position = reply
            .as_u64()
            .and_then(|position| u32::try_from(position).ok())
            .ok_or_else(|| {
                ScopinatorError::Backend(format!(
                    "get_wheel_position reply is not a position: {reply}"
                ))
            })?;
        let name = self.names().await?.get(position as usize).cloned();
        Ok(Some(FilterPosition { position, name }))
    }

    /// `set_wheel_position [n]`. A position the wheel does not have is
    /// `OutOfRange`.
    async fn set_filter_position(&self, position: u32) -> Result<(), ScopinatorError> {
        let slots = self.names().await?.len();
        if position as usize >= slots {
            return Err(ScopinatorError::OutOfRange(format!(
                "filter position {position}: the wheel has {slots}, counted from zero"
            )));
        }
        self.client
            .send_raw_and_validate("set_wheel_position", Some(json!([position])))
            .await?;
        Ok(())
    }

    async fn is_moving(&self) -> Result<bool, ScopinatorError> {
        let state = self
            .client
            .send_raw_and_validate("get_wheel_state", None)
            .await?;
        Ok(wheel_moving(&state))
    }

    async fn get_filter_names(&self) -> Result<Vec<String>, ScopinatorError> {
        self.names().await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Shapes from an S30 on firmware 9.75.
    #[test]
    fn names_come_from_the_wheel_setting_in_position_order() {
        let setting = json!({"exp_sec": [2.0, 2.0, 2.0], "names": ["dark", "IRCUT", "LP"]});
        assert_eq!(wheel_names(&setting).unwrap(), ["dark", "IRCUT", "LP"]);
        assert!(wheel_names(&json!({})).is_err());
        assert!(wheel_names(&json!({"names": [1, 2]})).is_err());
    }

    #[test]
    fn only_an_idle_wheel_is_at_rest() {
        assert!(!wheel_moving(
            &json!({"id": 0, "state": "idle", "unidirection": false})
        ));
        assert!(wheel_moving(&json!({"id": 0, "state": "moving"})));
        assert!(wheel_moving(&json!({})));
    }
}
