//! The Alpaca `focuser` device.

use std::sync::Arc;

use crate::client::AlpacaClient;
use crate::device::{Endpoint, actions, getters, setters};
use crate::error::AlpacaError;

/// One `focuser` device on an Alpaca server.
#[derive(Clone)]
pub struct AlpacaFocuser {
    endpoint: Endpoint,
}

impl AlpacaFocuser {
    pub fn new(client: Arc<AlpacaClient>, device_number: u32) -> Self {
        Self {
            endpoint: Endpoint::new(client, "focuser", device_number),
        }
    }

    pub fn device_number(&self) -> u32 {
        self.endpoint.device_number()
    }

    getters! {
        connected: bool = "connected";
        name: String = "name";
        driver_info: String = "driverinfo";
        /// True if the focuser knows its position and moves to one; false
        /// if it only moves by a number of steps.
        absolute: bool = "absolute";
        /// The position in steps. Only an absolute focuser has one.
        position: i32 = "position";
        /// The far end of the travel, in steps.
        max_step: i32 = "maxstep";
        /// The most steps one move can cover.
        max_increment: i32 = "maxincrement";
        /// The size of one step in micrometres.
        step_size: f64 = "stepsize";
        is_moving: bool = "ismoving";
        /// Temperature in degrees Celsius.
        temperature: f64 = "temperature";
        temp_comp: bool = "tempcomp";
        temp_comp_available: bool = "tempcompavailable";
    }

    setters! {
        set_connected(bool) = "connected", "Connected";
        set_temp_comp(bool) = "tempcomp", "TempComp";
    }

    actions! {
        halt = "halt";
    }

    /// Start a move. On an absolute focuser `position` is where to go; on a
    /// relative one it is how many steps to move, either way.
    pub async fn move_to(&self, position: i32) -> Result<(), AlpacaError> {
        self.endpoint
            .put("move", &[("Position", position.to_string())])
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::client::tests::{request_line_parts, serve};

    async fn focuser(body: &'static str) -> (AlpacaFocuser, tokio::task::JoinHandle<Vec<String>>) {
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        (AlpacaFocuser::new(client, 1), handle)
    }

    #[tokio::test]
    async fn a_move_sends_the_position() {
        let (focuser, handle) = focuser(r#"{"ErrorNumber": 0}"#).await;
        focuser.move_to(-250).await.unwrap();
        let request = handle.await.unwrap().pop().unwrap();
        let (method, path, _) = request_line_parts(&request);
        assert_eq!((method, path), ("PUT", "/api/v1/focuser/1/move"));
        assert!(request.contains("Position=-250"), "{request}");
    }

    #[tokio::test]
    async fn position_and_travel_are_read() {
        let (f, _h) = focuser(r#"{"Value": 12500, "ErrorNumber": 0}"#).await;
        assert_eq!(f.position().await.unwrap(), 12500);
        let (f, _h) = focuser(r#"{"Value": false, "ErrorNumber": 0}"#).await;
        assert!(!f.absolute().await.unwrap());
    }

    #[tokio::test]
    async fn a_focuser_without_a_thermometer_says_not_implemented() {
        let (f, _h) =
            focuser(r#"{"ErrorNumber": 1024, "ErrorMessage": "Temperature not implemented"}"#)
                .await;
        assert!(f.temperature().await.unwrap_err().is_not_implemented());
    }
}
