mod camera;
mod event_translator;
mod mount;
mod wide_camera;

pub use camera::SeestarCamera;
pub use event_translator::SeestarEventTranslator;
pub use mount::SeestarMount;
pub use wide_camera::SeestarWideCamera;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use scopinator_seestar::{DeviceSummary, SeestarClient, SeestarConfig};
use scopinator_types::DeviceId;

use crate::event::UnifiedEventBus;

/// Where a Seestar's services are. [`SeestarEndpoints::for_host`] gives the
/// standard ports; set the fields to go through a proxy or a test double.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeestarEndpoints {
    /// Main control port (4700).
    pub control: SocketAddr,
    /// Main image port (4800).
    pub imaging: SocketAddr,
    /// Wide camera's control port (4702) on two-camera models.
    pub wide_control: SocketAddr,
    /// Wide camera's image port (4804) on two-camera models.
    pub wide_imaging: SocketAddr,
}

impl SeestarEndpoints {
    /// The standard ports on `ip`.
    pub fn for_host(ip: Ipv4Addr) -> Self {
        use scopinator_seestar::protocol::json_rpc::{
            CONTROL_PORT, IMAGING_PORT, WIDE_CONTROL_PORT, WIDE_IMAGING_PORT,
        };
        let at = |port| SocketAddr::V4(SocketAddrV4::new(ip, port));
        Self {
            control: at(CONTROL_PORT),
            imaging: at(IMAGING_PORT),
            wide_control: at(WIDE_CONTROL_PORT),
            wide_imaging: at(WIDE_IMAGING_PORT),
        }
    }
}

/// A Seestar backend that wraps a `SeestarClient` and provides
/// Mount and Camera device adapters.
pub struct SeestarBackend {
    client: Arc<SeestarClient>,
    device_id: DeviceId,
    event_bus: Arc<UnifiedEventBus>,
    endpoints: SeestarEndpoints,
    config: SeestarConfig,
    /// The wide camera's own connection, opened on first use.
    wide_client: tokio::sync::OnceCell<Arc<SeestarClient>>,
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
        Self::connect_with_endpoints(ip, SeestarEndpoints::for_host(ip), config, event_bus).await
    }

    /// Create a new Seestar backend whose services are at explicit addresses
    /// (a proxy, or a test double). `ip` names the device.
    pub async fn connect_with_endpoints(
        ip: Ipv4Addr,
        endpoints: SeestarEndpoints,
        config: SeestarConfig,
        event_bus: Arc<UnifiedEventBus>,
    ) -> Result<Self, scopinator_seestar::SeestarError> {
        let client = SeestarClient::connect_with_ports_and_config(
            ip,
            endpoints.control,
            endpoints.imaging,
            config.clone(),
        )
        .await?;
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
            endpoints,
            config,
            wide_client: tokio::sync::OnceCell::new(),
        })
    }

    /// The wide camera of a two-camera Seestar, or `None` on a one-camera
    /// model.
    ///
    /// The wide camera runs as a second imager process with its own control
    /// and image ports; the first call opens a second connection to them
    /// (verified with the same key) and later calls reuse it. Its events
    /// (`SecondView`) are also forwarded by the device to the main
    /// connection, which is where the event bus gets them, once.
    ///
    /// Whether there is a second camera comes from the device state, which
    /// the main connection requests at connect; this waits a short time for
    /// it if it has not arrived yet.
    pub async fn second_camera(
        &self,
    ) -> Result<Option<SeestarWideCamera>, scopinator_seestar::SeestarError> {
        let mut has_second = self.client.device_info().has_second_camera;
        let deadline = tokio::time::Instant::now() + Duration::from_secs(5);
        while has_second.is_none() && tokio::time::Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(50)).await;
            has_second = self.client.device_info().has_second_camera;
        }
        if has_second != Some(true) {
            return Ok(None);
        }

        let wide = self
            .wide_client
            .get_or_try_init(|| async {
                // The clock belongs to the device, not to a camera: the main
                // connection has already set it if that was asked for.
                let config = SeestarConfig {
                    clock_sync: scopinator_seestar::ClockSync::Off,
                    ..self.config.clone()
                };
                let ip = match self.endpoints.wide_control {
                    SocketAddr::V4(addr) => *addr.ip(),
                    SocketAddr::V6(_) => Ipv4Addr::UNSPECIFIED,
                };
                SeestarClient::connect_with_ports_and_config(
                    ip,
                    self.endpoints.wide_control,
                    self.endpoints.wide_imaging,
                    config,
                )
                .await
                .map(Arc::new)
            })
            .await?;
        Ok(Some(SeestarWideCamera::new(
            Arc::clone(wide),
            Arc::clone(&self.client),
            DeviceId::new(format!("{}:wide", self.device_id)),
        )))
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
        if let Some(wide) = self.wide_client.get() {
            wide.shutdown().await;
        }
        self.client.shutdown().await;
    }
}
