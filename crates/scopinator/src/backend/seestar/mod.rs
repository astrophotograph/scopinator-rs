mod camera;
mod event_translator;
mod mount;

pub use camera::SeestarCamera;
pub use event_translator::SeestarEventTranslator;
pub use mount::SeestarMount;

use std::net::Ipv4Addr;
use std::sync::Arc;

use scopinator_seestar::{DeviceSummary, SeestarClient, SeestarConfig};
use scopinator_types::DeviceId;

use crate::event::UnifiedEventBus;

/// A Seestar backend that wraps a `SeestarClient` and provides
/// Mount and Camera device adapters.
pub struct SeestarBackend {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
    event_bus: Arc<UnifiedEventBus>,
}

impl SeestarBackend {
    /// Create a new Seestar backend connected to the given IP, with the
    /// default configuration (no interop key).
    pub async fn connect(
        ip: Ipv4Addr,
        event_bus: Arc<UnifiedEventBus>,
    ) -> Result<Self, scopinator_seestar::SeestarError> {
        Self::connect_with_config(ip, SeestarConfig::default(), event_bus).await
    }

    /// Create a new Seestar backend with a [`SeestarConfig`], e.g. to supply
    /// the interop key the device's verification handshake needs, or a
    /// response timeout.
    pub async fn connect_with_config(
        ip: Ipv4Addr,
        config: SeestarConfig,
        event_bus: Arc<UnifiedEventBus>,
    ) -> Result<Self, scopinator_seestar::SeestarError> {
        let client = SeestarClient::connect_with_config(ip, config).await?;
        let client = Arc::new(client);
        let device_id = DeviceId::new(format!("seestar:{ip}"));

        // Start event translation in the background
        SeestarEventTranslator::start(
            Arc::clone(&client),
            device_id.clone(),
            Arc::clone(&event_bus),
        );

        Ok(Self {
            client,
            device_id,
            event_bus,
        })
    }

    /// Get a mount adapter for this Seestar.
    pub fn mount(&self) -> SeestarMount {
        SeestarMount::new(Arc::clone(&self.client), self.device_id.clone())
    }

    /// Get a camera adapter for this Seestar.
    pub fn camera(&self) -> SeestarCamera {
        SeestarCamera::new(Arc::clone(&self.client), self.device_id.clone())
    }

    /// Get the underlying client (for advanced use).
    pub fn client(&self) -> &Arc<SeestarClient> {
        &self.client
    }

    /// What the device has reported about itself (model, firmware, optics,
    /// sensor, capability flags). Fields are `None` until known.
    pub fn device_info(&self) -> DeviceSummary {
        self.client.device_info()
    }

    /// Get the device ID.
    pub fn device_id(&self) -> &DeviceId {
        &self.device_id
    }

    /// Get the shared event bus.
    pub fn event_bus(&self) -> &Arc<UnifiedEventBus> {
        &self.event_bus
    }

    /// Disconnect the backend.
    pub async fn disconnect(&self) {
        self.client.shutdown().await;
    }
}
