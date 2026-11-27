use std::sync::Arc;

use async_trait::async_trait;
use scopinator_seestar::SeestarClient;
use scopinator_types::{DeviceId, DeviceInfo, DeviceKind, SwitchInfo};
use serde_json::{Value, json};

use crate::device::status::DeviceStatus;
use crate::device::traits::{Device, Switch, check_switch_value};
use crate::error::ScopinatorError;

/// The dew heater's switch number.
pub const DEW_HEATER: u32 = 0;

/// The power a heater is switched on at, in percent. The request has to
/// name one; an S30 reports its outputs as not variable and does not report
/// the value back, so the heater is offered as on or off.
const HEATER_POWER: u32 = 100;

/// A Seestar's switchable outputs: its dew heater, as switch 0, on or off.
///
/// It is switched with `pi_output_set2 {"heater": {"state", "value"}}` and
/// read from `setting.heater_enable` in the device state. (`set_setting`
/// refuses `heater_enable` as an unexpected parameter.)
pub struct SeestarSwitch {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
}

impl SeestarSwitch {
    pub(crate) fn new(client: Arc<SeestarClient>, device_id: DeviceId) -> Self {
        Self { client, device_id }
    }
}

fn heater_info() -> SwitchInfo {
    SwitchInfo::on_off(DEW_HEATER, "Dew heater")
        .with_description("Warms the lens to keep dew off it")
}

fn no_such_switch(index: u32) -> ScopinatorError {
    ScopinatorError::OutOfRange(format!(
        "there is no switch {index}: a Seestar has switch {DEW_HEATER}, the dew heater"
    ))
}

/// Whether the heater is on, from a `get_device_state` reply.
fn heater_on(state: &Value) -> Result<bool, ScopinatorError> {
    state
        .pointer("/setting/heater_enable")
        .and_then(Value::as_bool)
        .ok_or_else(|| {
            ScopinatorError::Backend(
                "get_device_state reply has no `setting.heater_enable`".to_string(),
            )
        })
}

#[async_trait]
impl Device for SeestarSwitch {
    fn id(&self) -> DeviceId {
        self.device_id.clone()
    }

    fn info(&self) -> DeviceInfo {
        let summary = self.client.device_info();
        let mut info = DeviceInfo::new(
            self.device_id.clone(),
            DeviceKind::Switch,
            "Seestar outputs",
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
            description: Some("Seestar outputs".into()),
        })
    }
}

#[async_trait]
impl Switch for SeestarSwitch {
    async fn switches(&self) -> Result<Vec<SwitchInfo>, ScopinatorError> {
        Ok(vec![heater_info()])
    }

    async fn get(&self, index: u32) -> Result<f64, ScopinatorError> {
        if index != DEW_HEATER {
            return Err(no_such_switch(index));
        }
        let state = self
            .client
            .send_raw_and_validate("get_device_state", Some(json!({"keys": ["setting"]})))
            .await?;
        Ok(if heater_on(&state)? { 1.0 } else { 0.0 })
    }

    async fn set(&self, index: u32, value: f64) -> Result<(), ScopinatorError> {
        if index != DEW_HEATER {
            return Err(no_such_switch(index));
        }
        let info = heater_info();
        check_switch_value(&info, value)?;
        self.client
            .send_raw_and_validate(
                "pi_output_set2",
                Some(json!({"heater": {"state": info.is_on(value), "value": HEATER_POWER}})),
            )
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_heater_state_is_read_from_the_settings() {
        let on = json!({"setting": {"heater_enable": true, "expt_heater_enable": false}});
        assert!(heater_on(&on).unwrap());
        let off = json!({"setting": {"heater_enable": false}});
        assert!(!heater_on(&off).unwrap());
        // Not known is not "off".
        assert!(heater_on(&json!({"setting": {}})).is_err());
    }
}
