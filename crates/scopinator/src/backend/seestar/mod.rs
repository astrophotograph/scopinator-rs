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

/// What the backend remembers about the goto it last sent, shared by every
/// mount adapter of one telescope.
///
/// A Seestar accepts a goto and starts moving some seconds later (6 s was
/// seen on an S30), and reports nothing in between. The mount has to answer
/// "slewing" in that gap, so the time of acceptance is kept here.
#[derive(Debug, Default)]
pub(crate) struct GotoTracker {
    pending: std::sync::Mutex<Option<PendingGoto>>,
}

#[derive(Debug, Clone, Copy)]
struct PendingGoto {
    accepted_at: tokio::time::Instant,
    /// The telescope has been seen working on it.
    seen_running: bool,
}

impl GotoTracker {
    /// A goto was accepted just now.
    pub(crate) fn accepted(&self) {
        *self.lock() = Some(PendingGoto {
            accepted_at: tokio::time::Instant::now(),
            seen_running: false,
        });
    }

    /// The telescope reports that it is working on the goto.
    pub(crate) fn running(&self) {
        if let Some(goto) = self.lock().as_mut() {
            goto.seen_running = true;
        }
    }

    /// The goto is over (arrived, failed or aborted).
    pub(crate) fn clear(&self) {
        *self.lock() = None;
    }

    /// True if a goto has been accepted and not yet seen to end.
    pub(crate) fn is_pending(&self) -> bool {
        self.lock().is_some()
    }

    /// True if a goto was accepted less than `grace` ago and the telescope
    /// has not been seen working on it yet.
    pub(crate) fn awaiting_start(&self, grace: Duration) -> bool {
        self.lock()
            .is_some_and(|goto| !goto.seen_running && goto.accepted_at.elapsed() < grace)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<PendingGoto>> {
        self.pending
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
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
    goto: Arc<GotoTracker>,
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
            goto: Arc::new(GotoTracker::default()),
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
        SeestarMount::new(
            Arc::clone(&self.client),
            DeviceId::new(format!("{}:mount", self.device_id)),
            Arc::clone(&self.goto),
        )
    }

    /// Get a camera adapter for this Seestar.
    pub fn camera(&self) -> SeestarCamera {
        SeestarCamera::new(
            Arc::clone(&self.client),
            DeviceId::new(format!("{}:camera", self.device_id)),
        )
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

    /// The id of the telescope as a whole (`seestar:<ip>`), which is the
    /// `device_id` on the events it emits. Its mount and cameras have ids
    /// under it: `seestar:<ip>:mount`, `seestar:<ip>:camera` and
    /// `seestar:<ip>:wide`.
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

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn a_goto_awaits_its_start_only_for_the_grace_period() {
        let tracker = GotoTracker::default();
        let grace = Duration::from_secs(15);
        assert!(!tracker.awaiting_start(grace));

        tracker.accepted();
        assert!(tracker.is_pending());
        assert!(tracker.awaiting_start(grace));
        tokio::time::advance(Duration::from_secs(14)).await;
        assert!(tracker.awaiting_start(grace));
        tokio::time::advance(Duration::from_secs(2)).await;
        assert!(!tracker.awaiting_start(grace));
        // Still pending until someone sees it end.
        assert!(tracker.is_pending());
    }

    #[tokio::test(start_paused = true)]
    async fn a_goto_seen_running_no_longer_awaits_its_start() {
        let tracker = GotoTracker::default();
        tracker.accepted();
        tracker.running();
        assert!(!tracker.awaiting_start(Duration::from_secs(15)));
        assert!(tracker.is_pending());
        tracker.clear();
        assert!(!tracker.is_pending());
    }
}
