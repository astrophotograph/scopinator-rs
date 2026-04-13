use std::net::Ipv4Addr;
use std::sync::atomic::{AtomicU32, Ordering};

use serde::Deserialize;
use tracing::trace;

use crate::error::AlpacaError;

/// Default Alpaca server port.
pub const DEFAULT_PORT: u16 = 11111;

/// Client ID for Alpaca transactions.
const CLIENT_ID: u32 = 1;

/// Low-level ASCOM Alpaca HTTP client.
///
/// Handles GET/PUT requests with automatic transaction ID management.
pub struct AlpacaClient {
    base_url: String,
    http: reqwest::Client,
    transaction_id: AtomicU32,
}

/// Standard Alpaca response envelope.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct AlpacaResponse {
    #[serde(default)]
    value: serde_json::Value,
    #[serde(default)]
    error_number: i32,
    #[serde(default)]
    error_message: String,
}

impl AlpacaClient {
    /// Create a new client for a given host and port.
    pub fn new(host: Ipv4Addr, port: u16) -> Self {
        Self {
            base_url: format!("http://{host}:{port}"),
            http: reqwest::Client::new(),
            transaction_id: AtomicU32::new(1),
        }
    }

    fn next_transaction_id(&self) -> u32 {
        self.transaction_id.fetch_add(1, Ordering::Relaxed)
    }

    /// GET a device property.
    pub async fn get_property(
        &self,
        device_type: &str,
        device_number: u32,
        property: &str,
    ) -> Result<serde_json::Value, AlpacaError> {
        let url = format!(
            "{}/api/v1/{device_type}/{device_number}/{property}",
            self.base_url
        );
        let tid = self.next_transaction_id();

        trace!(url = %url, "alpaca GET");

        let resp = self
            .http
            .get(&url)
            .query(&[
                ("ClientID", CLIENT_ID.to_string()),
                ("ClientTransactionID", tid.to_string()),
            ])
            .send()
            .await?
            .json::<AlpacaResponse>()
            .await?;

        if resp.error_number != 0 {
            return Err(AlpacaError::Device {
                code: resp.error_number,
                message: resp.error_message,
            });
        }

        Ok(resp.value)
    }

    /// PUT (set) a device property.
    pub async fn set_property(
        &self,
        device_type: &str,
        device_number: u32,
        property: &str,
        params: &[(&str, String)],
    ) -> Result<serde_json::Value, AlpacaError> {
        let url = format!(
            "{}/api/v1/{device_type}/{device_number}/{property}",
            self.base_url
        );
        let tid = self.next_transaction_id();

        let mut form = vec![
            ("ClientID", CLIENT_ID.to_string()),
            ("ClientTransactionID", tid.to_string()),
        ];
        for (k, v) in params {
            form.push((k, v.clone()));
        }

        trace!(url = %url, "alpaca PUT");

        let resp = self
            .http
            .put(&url)
            .form(&form)
            .send()
            .await?
            .json::<AlpacaResponse>()
            .await?;

        if resp.error_number != 0 {
            return Err(AlpacaError::Device {
                code: resp.error_number,
                message: resp.error_message,
            });
        }

        Ok(resp.value)
    }

    /// Query the management API for configured devices.
    pub async fn get_configured_devices(&self) -> Result<Vec<ConfiguredDevice>, AlpacaError> {
        let url = format!("{}/management/v1/configureddevices", self.base_url);

        let resp: ManagementResponse = self.http.get(&url).send().await?.json().await?;

        Ok(resp.value)
    }
}

/// A device reported by the Alpaca management API.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "PascalCase")]
pub struct ConfiguredDevice {
    pub device_name: String,
    pub device_type: String,
    pub device_number: u32,
    pub unique_id: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ManagementResponse {
    value: Vec<ConfiguredDevice>,
}
