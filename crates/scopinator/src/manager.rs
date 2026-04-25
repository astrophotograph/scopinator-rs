use std::collections::HashMap;
use std::sync::Arc;

use crate::event::UnifiedEventBus;

#[cfg(feature = "seestar")]
use crate::backend::seestar::SeestarBackend;

/// Manages multiple telescope backends and provides a unified interface.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use scopinator::DeviceManager;
///
/// let manager = DeviceManager::new();
///
/// // Add a Seestar backend
/// manager.add_seestar("192.168.1.100".parse()?).await?;
///
/// // Get devices
/// let backends = manager.list_backends();
/// # Ok(())
/// # }
/// ```
pub struct DeviceManager {
    event_bus: Arc<UnifiedEventBus>,
    #[cfg(feature = "seestar")]
    seestar_backends: tokio::sync::Mutex<HashMap<String, SeestarBackend>>,
}

impl DeviceManager {
    /// Create a new device manager with a shared event bus.
    pub fn new() -> Self {
        Self {
            event_bus: Arc::new(UnifiedEventBus::new(256)),
            #[cfg(feature = "seestar")]
            seestar_backends: tokio::sync::Mutex::new(HashMap::new()),
        }
    }

    /// Get the shared event bus.
    pub fn event_bus(&self) -> &Arc<UnifiedEventBus> {
        &self.event_bus
    }

    /// Add a Seestar backend by IP address.
    #[cfg(feature = "seestar")]
    pub async fn add_seestar(
        &self,
        ip: std::net::Ipv4Addr,
    ) -> Result<(), crate::error::ScopinatorError> {
        let backend = SeestarBackend::connect(ip, Arc::clone(&self.event_bus)).await?;
        let key = ip.to_string();
        let mut backends = self.seestar_backends.lock().await;
        backends.insert(key, backend);
        Ok(())
    }

    /// Get a Seestar mount by IP address string.
    #[cfg(feature = "seestar")]
    pub async fn seestar_mount(&self, key: &str) -> Option<crate::backend::seestar::SeestarMount> {
        let backends = self.seestar_backends.lock().await;
        backends.get(key).map(|b| b.mount())
    }

    /// Get a Seestar camera by IP address string.
    #[cfg(feature = "seestar")]
    pub async fn seestar_camera(
        &self,
        key: &str,
    ) -> Option<crate::backend::seestar::SeestarCamera> {
        let backends = self.seestar_backends.lock().await;
        backends.get(key).map(|b| b.camera())
    }

    /// List all backend keys.
    pub async fn list_backends(&self) -> Vec<String> {
        let mut keys = Vec::new();
        #[cfg(feature = "seestar")]
        {
            let backends = self.seestar_backends.lock().await;
            keys.extend(backends.keys().map(|k| format!("seestar:{k}")));
        }
        keys
    }

    /// Disconnect all backends.
    pub async fn disconnect_all(&self) {
        #[cfg(feature = "seestar")]
        {
            let backends = self.seestar_backends.lock().await;
            for backend in backends.values() {
                backend.disconnect().await;
            }
        }
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}
