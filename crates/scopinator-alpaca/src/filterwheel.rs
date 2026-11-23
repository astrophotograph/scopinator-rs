//! The Alpaca `filterwheel` device.

use std::sync::Arc;

use crate::client::AlpacaClient;
use crate::device::{Endpoint, getters, setters};
use crate::error::AlpacaError;

/// One `filterwheel` device on an Alpaca server. Filter positions count
/// from zero.
#[derive(Clone)]
pub struct AlpacaFilterWheel {
    endpoint: Endpoint,
}

impl AlpacaFilterWheel {
    pub fn new(client: Arc<AlpacaClient>, device_number: u32) -> Self {
        Self {
            endpoint: Endpoint::new(client, "filterwheel", device_number),
        }
    }

    pub fn device_number(&self) -> u32 {
        self.endpoint.device_number()
    }

    getters! {
        connected: bool = "connected";
        name: String = "name";
        driver_info: String = "driverinfo";
        /// The filter names, in position order.
        names: Vec<String> = "names";
        /// The focus offset of each filter, in position order.
        focus_offsets: Vec<i32> = "focusoffsets";
    }

    setters! {
        set_connected(bool) = "connected", "Connected";
        /// Start a move to a filter.
        set_position(u16) = "position", "Position";
    }

    /// The filter in the light path, or `None` while the wheel is turning
    /// (which Alpaca reports as -1).
    pub async fn position(&self) -> Result<Option<u16>, AlpacaError> {
        let position: i32 = self.endpoint.get("position").await?;
        match position {
            -1 => Ok(None),
            p => u16::try_from(p)
                .map(Some)
                .map_err(|_| AlpacaError::Value(format!("filter position {p}"))),
        }
    }
}

#[cfg(test)]
mod tests {
    use std::net::Ipv4Addr;

    use super::*;
    use crate::client::tests::{request_line_parts, serve};

    async fn wheel(
        body: &'static str,
    ) -> (AlpacaFilterWheel, tokio::task::JoinHandle<Vec<String>>) {
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = Arc::new(AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port()));
        (AlpacaFilterWheel::new(client, 0), handle)
    }

    #[tokio::test]
    async fn minus_one_means_the_wheel_is_turning() {
        let (w, _h) = wheel(r#"{"Value": -1, "ErrorNumber": 0}"#).await;
        assert_eq!(w.position().await.unwrap(), None);
        let (w, _h) = wheel(r#"{"Value": 0, "ErrorNumber": 0}"#).await;
        assert_eq!(w.position().await.unwrap(), Some(0));
        let (w, _h) = wheel(r#"{"Value": 4, "ErrorNumber": 0}"#).await;
        assert_eq!(w.position().await.unwrap(), Some(4));
        // Any other negative number is not a position.
        let (w, _h) = wheel(r#"{"Value": -2, "ErrorNumber": 0}"#).await;
        assert!(matches!(w.position().await, Err(AlpacaError::Value(_))));
    }

    #[tokio::test]
    async fn names_and_offsets_are_lists_in_position_order() {
        let (w, _h) = wheel(r#"{"Value": ["L", "R", "G", "B"], "ErrorNumber": 0}"#).await;
        assert_eq!(w.names().await.unwrap(), ["L", "R", "G", "B"]);
        let (w, _h) = wheel(r#"{"Value": [0, 12, -5, 30], "ErrorNumber": 0}"#).await;
        assert_eq!(w.focus_offsets().await.unwrap(), [0, 12, -5, 30]);
    }

    #[tokio::test]
    async fn a_move_sends_the_position() {
        let (w, handle) = wheel(r#"{"ErrorNumber": 0}"#).await;
        w.set_position(3).await.unwrap();
        let request = handle.await.unwrap().pop().unwrap();
        let (method, path, _) = request_line_parts(&request);
        assert_eq!((method, path), ("PUT", "/api/v1/filterwheel/0/position"));
        assert!(request.contains("Position=3"), "{request}");
    }
}
