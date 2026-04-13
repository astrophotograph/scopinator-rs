use serde::Deserialize;

/// A response to a command sent on the control port.
#[derive(Debug, Clone, Deserialize)]
pub struct CommandResponse {
    pub id: u64,
    #[serde(default)]
    pub jsonrpc: String,
    #[serde(rename = "Timestamp")]
    pub timestamp: Option<String>,
    pub method: Option<String>,
    #[serde(default)]
    pub code: i32,
    pub error: Option<String>,
    pub result: Option<serde_json::Value>,
}

impl CommandResponse {
    /// Returns true if the command succeeded (code == 0).
    pub fn is_success(&self) -> bool {
        self.code == 0
    }

    /// Returns an error if the command failed.
    pub fn into_result(self) -> Result<serde_json::Value, CommandError> {
        if self.is_success() {
            Ok(self.result.unwrap_or(serde_json::Value::Null))
        } else {
            Err(CommandError {
                code: self.code,
                message: self
                    .error
                    .unwrap_or_else(|| format!("command failed with code {}", self.code)),
                method: self.method,
            })
        }
    }
}

/// An error returned by a failed command.
#[derive(Debug, Clone, thiserror::Error)]
#[error("command {method:?} failed (code {code}): {message}")]
pub struct CommandError {
    pub code: i32,
    pub message: String,
    pub method: Option<String>,
}

/// Device state returned by `get_device_state`.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceStateResult {
    pub device: Option<DeviceInfo>,
    pub setting: Option<serde_json::Value>,
    pub camera: Option<CameraInfo>,
    pub focuser: Option<FocuserInfo>,
    pub ap: Option<serde_json::Value>,
    pub station: Option<serde_json::Value>,
    pub storage: Option<serde_json::Value>,
    pub balance_sensor: Option<serde_json::Value>,
    pub compass_sensor: Option<serde_json::Value>,
    pub mount: Option<MountInfo>,
    pub pi_status: Option<PiStatusInfo>,
}

/// Device identification info.
#[derive(Debug, Clone, Deserialize)]
pub struct DeviceInfo {
    pub name: Option<String>,
    pub firmware_ver_int: Option<u32>,
    pub firmware_ver_string: Option<String>,
    pub sn: Option<String>,
    #[serde(rename = "cpuId")]
    pub cpu_id: Option<String>,
    pub product_model: Option<String>,
    pub focal_len: Option<f64>,
    pub fnumber: Option<f64>,
}

/// Camera sensor info.
#[derive(Debug, Clone, Deserialize)]
pub struct CameraInfo {
    pub chip_size: Option<(f64, f64)>,
    pub pixel_size_um: Option<f64>,
    pub debayer_pattern: Option<String>,
}

/// Focuser info.
#[derive(Debug, Clone, Deserialize)]
pub struct FocuserInfo {
    pub state: Option<String>,
    pub max_step: Option<i32>,
    pub step: Option<i32>,
}

/// Mount info.
#[derive(Debug, Clone, Deserialize)]
pub struct MountInfo {
    pub move_type: Option<String>,
    pub tracking: Option<bool>,
    pub equ_mode: Option<String>,
}

/// Pi status info (temperatures, battery, charge).
#[derive(Debug, Clone, Deserialize)]
pub struct PiStatusInfo {
    pub temp: Option<f64>,
    pub battery_capacity: Option<i32>,
    pub charger_status: Option<String>,
    pub charge_online: Option<bool>,
}

/// View state returned by `get_view_state`.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewStateResult {
    #[serde(rename = "View")]
    pub view: Option<ViewInfo>,
}

/// View info within the view state.
#[derive(Debug, Clone, Deserialize)]
pub struct ViewInfo {
    pub state: Option<String>,
    pub mode: Option<String>,
    pub gain: Option<i32>,
    #[serde(rename = "ContinuousExposure")]
    pub continuous_exposure: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserialize_success_response() {
        let json = r#"{
            "id": 100,
            "jsonrpc": "2.0",
            "Timestamp": "1234567890.0",
            "method": "test_connection",
            "code": 0,
            "result": "server connected!"
        }"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert!(resp.is_success());
        assert_eq!(resp.id, 100);
        assert_eq!(resp.method.as_deref(), Some("test_connection"));
    }

    #[test]
    fn deserialize_error_response() {
        let json = r#"{
            "id": 200,
            "jsonrpc": "2.0",
            "method": "unknown_method",
            "code": 210,
            "error": "method not supported"
        }"#;
        let resp: CommandResponse = serde_json::from_str(json).unwrap();
        assert!(!resp.is_success());
        let err = resp.into_result().unwrap_err();
        assert_eq!(err.code, 210);
    }

    #[test]
    fn deserialize_device_state() {
        let json = r#"{
            "device": {
                "name": "Seestar",
                "firmware_ver_int": 3000,
                "firmware_ver_string": "3.0.0",
                "sn": "ABC123",
                "product_model": "Seestar S50",
                "focal_len": 250.0,
                "fnumber": 5.0
            },
            "focuser": {
                "state": "idle",
                "max_step": 80000,
                "step": 40000
            },
            "mount": {
                "tracking": true
            }
        }"#;
        let state: DeviceStateResult = serde_json::from_str(json).unwrap();
        assert_eq!(
            state.device.as_ref().unwrap().firmware_ver_int,
            Some(3000)
        );
        assert_eq!(state.focuser.as_ref().unwrap().step, Some(40000));
        assert_eq!(state.mount.as_ref().unwrap().tracking, Some(true));
    }
}
