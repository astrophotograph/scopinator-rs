//! The Alpaca `switch` device: a numbered set of outputs and sensors, each
//! with a value in a range.

use std::sync::Arc;

use serde::de::DeserializeOwned;

use crate::client::AlpacaClient;
use crate::device::{Endpoint, bool_param, getters, setters};
use crate::error::AlpacaError;

/// One `switch` device on an Alpaca server.
#[derive(Clone)]
pub struct AlpacaSwitch {
    endpoint: Endpoint,
}

impl AlpacaSwitch {
    pub fn new(client: Arc<AlpacaClient>, device_number: u32) -> Self {
        Self {
            endpoint: Endpoint::new(client, "switch", device_number),
        }
    }

    pub fn device_number(&self) -> u32 {
        self.endpoint.device_number()
    }

    getters! {
        connected: bool = "connected";
        name: String = "name";
        driver_info: String = "driverinfo";
        /// How many switches the device has. Their ids run from zero.
        max_switch: i32 = "maxswitch";
    }

    setters! {
        set_connected(bool) = "connected", "Connected";
    }

    async fn of<T: DeserializeOwned>(&self, property: &str, id: u32) -> Result<T, AlpacaError> {
        self.endpoint
            .get_with(property, &[("Id", id.to_string())])
            .await
    }

    /// The name of switch `id`.
    pub async fn switch_name(&self, id: u32) -> Result<String, AlpacaError> {
        self.of("getswitchname", id).await
    }

    pub async fn switch_description(&self, id: u32) -> Result<String, AlpacaError> {
        self.of("getswitchdescription", id).await
    }

    /// Whether switch `id` is on.
    pub async fn switch(&self, id: u32) -> Result<bool, AlpacaError> {
        self.of("getswitch", id).await
    }

    /// The value of switch `id`, between its least and greatest.
    pub async fn switch_value(&self, id: u32) -> Result<f64, AlpacaError> {
        self.of("getswitchvalue", id).await
    }

    pub async fn min_switch_value(&self, id: u32) -> Result<f64, AlpacaError> {
        self.of("minswitchvalue", id).await
    }

    pub async fn max_switch_value(&self, id: u32) -> Result<f64, AlpacaError> {
        self.of("maxswitchvalue", id).await
    }

    /// The difference between two values switch `id` can tell apart.
    pub async fn switch_step(&self, id: u32) -> Result<f64, AlpacaError> {
        self.of("switchstep", id).await
    }

    /// False for a switch that can only be read.
    pub async fn can_write(&self, id: u32) -> Result<bool, AlpacaError> {
        self.of("canwrite", id).await
    }

    /// Turn switch `id` on or off.
    pub async fn set_switch(&self, id: u32, on: bool) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "setswitch",
                &[("Id", id.to_string()), ("State", bool_param(on))],
            )
            .await
    }

    /// Set switch `id` to `value`.
    pub async fn set_switch_value(&self, id: u32, value: f64) -> Result<(), AlpacaError> {
        self.endpoint
            .put(
                "setswitchvalue",
                &[("Id", id.to_string()), ("Value", value.to_string())],
            )
            .await
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::client::tests::{request_line_parts, serve};

    async fn switch(body: &'static str) -> (AlpacaSwitch, tokio::task::JoinHandle<Vec<String>>) {
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        (AlpacaSwitch::new(client, 2), handle)
    }

    #[tokio::test]
    async fn a_value_is_asked_for_by_id() {
        let (switch, handle) = switch(r#"{"Value": 0.75, "ErrorNumber": 0}"#).await;
        assert_eq!(switch.switch_value(3).await.unwrap(), 0.75);
        let request = handle.await.unwrap().pop().unwrap();
        let (method, path, query) = request_line_parts(&request);
        assert_eq!((method, path), ("GET", "/api/v1/switch/2/getswitchvalue"));
        assert!(query.iter().any(|pair| pair == "Id=3"), "{query:?}");
    }

    #[tokio::test]
    async fn setting_a_value_sends_the_id_and_the_value() {
        let (switch, handle) = switch(r#"{"ErrorNumber": 0}"#).await;
        switch.set_switch_value(1, 42.5).await.unwrap();
        let request = handle.await.unwrap().pop().unwrap();
        let (method, path, _) = request_line_parts(&request);
        assert_eq!((method, path), ("PUT", "/api/v1/switch/2/setswitchvalue"));
        assert!(
            request.contains("Id=1") && request.contains("Value=42.5"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn switching_on_sends_the_state() {
        let (switch, handle) = switch(r#"{"ErrorNumber": 0}"#).await;
        switch.set_switch(0, true).await.unwrap();
        let request = handle.await.unwrap().pop().unwrap();
        assert!(
            request.contains("Id=0") && request.contains("State=True"),
            "{request}"
        );
    }

    #[tokio::test]
    async fn an_id_the_device_does_not_have_is_an_invalid_value() {
        let (switch, _h) =
            switch(r#"{"ErrorNumber": 1025, "ErrorMessage": "Id 9 is out of range"}"#).await;
        let error = switch.switch_name(9).await.unwrap_err();
        assert_eq!(
            error.device_code(),
            Some(crate::error::codes::INVALID_VALUE)
        );
    }
}
