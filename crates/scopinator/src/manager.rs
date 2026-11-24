use std::collections::HashMap;
use std::sync::{Arc, RwLock};

use scopinator_types::DeviceId;

use crate::backend::Backend;
use crate::error::ScopinatorError;
use crate::event::UnifiedEventBus;

/// The instruments a program is connected to, of any kind, and the event bus
/// they all report to.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use scopinator::DeviceManager;
///
/// let manager = DeviceManager::new();
/// let id = manager.add_seestar("192.168.1.100".parse()?).await?;
///
/// // From here on nothing is specific to a Seestar.
/// let telescope = manager.get(&id).expect("just added");
/// if let Some(mount) = telescope.mount() {
///     println!("tracking: {}", mount.is_tracking().await?);
/// }
/// for camera in telescope.cameras().await? {
///     println!("{}", camera.info().name);
/// }
/// # Ok(())
/// # }
/// ```
pub struct DeviceManager {
    event_bus: Arc<UnifiedEventBus>,
    backends: RwLock<HashMap<DeviceId, Arc<dyn Backend>>>,
}

impl DeviceManager {
    /// Create a new device manager with a shared event bus.
    pub fn new() -> Self {
        Self {
            event_bus: Arc::new(UnifiedEventBus::new(256)),
            backends: RwLock::new(HashMap::new()),
        }
    }

    /// Get the shared event bus. A backend built by hand should be given
    /// this bus so that its events arrive with everyone else's.
    pub fn event_bus(&self) -> &Arc<UnifiedEventBus> {
        &self.event_bus
    }

    /// Add a connected backend and return its id. Fails, leaving the manager
    /// unchanged, if a backend with the same id is already there: remove
    /// that one first.
    pub fn add(&self, backend: Arc<dyn Backend>) -> Result<DeviceId, ScopinatorError> {
        let id = backend.id();
        let mut backends = self.write();
        if backends.contains_key(&id) {
            return Err(ScopinatorError::InvalidState(format!(
                "{id} has already been added"
            )));
        }
        backends.insert(id.clone(), backend);
        Ok(id)
    }

    /// The backend with this id.
    pub fn get(&self, id: &DeviceId) -> Option<Arc<dyn Backend>> {
        self.read().get(id).cloned()
    }

    /// Every backend, ordered by id.
    pub fn backends(&self) -> Vec<Arc<dyn Backend>> {
        let backends = self.read();
        let mut all: Vec<_> = backends.iter().collect();
        all.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
        all.into_iter()
            .map(|(_, backend)| Arc::clone(backend))
            .collect()
    }

    /// The ids of every backend, in order.
    pub fn list_backends(&self) -> Vec<DeviceId> {
        self.backends().iter().map(|backend| backend.id()).collect()
    }

    /// Take a backend out of the manager without disconnecting it.
    pub fn remove(&self, id: &DeviceId) -> Option<Arc<dyn Backend>> {
        self.write().remove(id)
    }

    /// Disconnect every backend and remove it. All of them are attempted;
    /// the first error, if any, is returned afterwards.
    pub async fn disconnect_all(&self) -> Result<(), ScopinatorError> {
        let backends: Vec<_> = self.write().drain().map(|(_, backend)| backend).collect();
        let mut first_error = None;
        for backend in backends {
            if let Err(e) = backend.disconnect().await {
                tracing::warn!(backend = %backend.id(), error = %e, "disconnect failed");
                first_error.get_or_insert(e);
            }
        }
        first_error.map_or(Ok(()), Err)
    }

    /// Connect to a Seestar by IP address and add it.
    #[cfg(feature = "seestar")]
    pub async fn add_seestar(&self, ip: std::net::Ipv4Addr) -> Result<DeviceId, ScopinatorError> {
        self.add_seestar_with_config(ip, Default::default()).await
    }

    /// Connect to a Seestar by IP address with a
    /// [`SeestarConfig`](scopinator_seestar::SeestarConfig), e.g. to supply
    /// an interop key, and add it.
    ///
    /// What only a Seestar has is reached by downcasting:
    /// `manager.get(&id)?.as_any().downcast_ref::<SeestarBackend>()`.
    #[cfg(feature = "seestar")]
    pub async fn add_seestar_with_config(
        &self,
        ip: std::net::Ipv4Addr,
        config: scopinator_seestar::SeestarConfig,
    ) -> Result<DeviceId, ScopinatorError> {
        use crate::backend::seestar::SeestarBackend;

        let id = DeviceId::new(format!("seestar:{ip}"));
        if self.get(&id).is_some() {
            return Err(ScopinatorError::InvalidState(format!(
                "{id} has already been added"
            )));
        }
        let backend =
            SeestarBackend::connect_with_config(ip, config, Arc::clone(&self.event_bus)).await?;
        self.add(Arc::new(backend))
    }

    /// Connect to an instrument found by [`crate::discovery`] and add it.
    /// `NotSupported` if this build has no backend of its kind.
    pub async fn connect(
        &self,
        found: &crate::discovery::Discovered,
    ) -> Result<DeviceId, ScopinatorError> {
        match found.backend_kind {
            #[cfg(feature = "seestar")]
            "seestar" => match found.address {
                std::net::SocketAddr::V4(address) => self.add_seestar(*address.ip()).await,
                std::net::SocketAddr::V6(address) => Err(ScopinatorError::InvalidArgument(
                    format!("a Seestar has an IPv4 address, not {address}"),
                )),
            },
            #[cfg(feature = "alpaca")]
            "alpaca" => match found.address {
                std::net::SocketAddr::V4(address) => {
                    self.add_alpaca(*address.ip(), address.port()).await
                }
                std::net::SocketAddr::V6(address) => Err(ScopinatorError::InvalidArgument(
                    format!("this Alpaca client takes an IPv4 address, not {address}"),
                )),
            },
            kind => Err(ScopinatorError::NotSupported(format!(
                "no `{kind}` backend in this build"
            ))),
        }
    }

    /// Connect to the Alpaca server at `host:port` with the default
    /// configuration and add it.
    #[cfg(feature = "alpaca")]
    pub async fn add_alpaca(
        &self,
        host: std::net::Ipv4Addr,
        port: u16,
    ) -> Result<DeviceId, ScopinatorError> {
        self.add_alpaca_with_config(host, port, Default::default())
            .await
    }

    /// Connect to the Alpaca server at `host:port`, choosing devices and the
    /// polling interval with an
    /// [`AlpacaConfig`](crate::backend::alpaca::AlpacaConfig), and add it.
    #[cfg(feature = "alpaca")]
    pub async fn add_alpaca_with_config(
        &self,
        host: std::net::Ipv4Addr,
        port: u16,
        config: crate::backend::alpaca::AlpacaConfig,
    ) -> Result<DeviceId, ScopinatorError> {
        use crate::backend::alpaca::AlpacaBackend;

        let id = AlpacaBackend::id_for(host, port);
        if self.get(&id).is_some() {
            return Err(ScopinatorError::InvalidState(format!(
                "{id} has already been added"
            )));
        }
        let backend =
            AlpacaBackend::connect(host, port, config, Arc::clone(&self.event_bus)).await?;
        self.add(Arc::new(backend))
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, HashMap<DeviceId, Arc<dyn Backend>>> {
        self.backends
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, HashMap<DeviceId, Arc<dyn Backend>>> {
        self.backends
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for DeviceManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use std::any::Any;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use async_trait::async_trait;

    use super::*;
    use crate::backend::main_camera;
    use crate::device::mock::{MockCamera, MockMount};
    use crate::device::{Camera, FilterWheel, Focuser, Mount};

    /// A backend with a mount and one camera that counts its disconnects.
    struct MockBackend {
        id: &'static str,
        disconnects: AtomicUsize,
        fail_disconnect: bool,
    }

    impl MockBackend {
        fn new(id: &'static str) -> Arc<Self> {
            Arc::new(Self {
                id,
                disconnects: AtomicUsize::new(0),
                fail_disconnect: false,
            })
        }
    }

    #[async_trait]
    impl Backend for MockBackend {
        fn id(&self) -> DeviceId {
            DeviceId::new(self.id)
        }

        fn kind(&self) -> &'static str {
            "mock"
        }

        fn mount(&self) -> Option<Arc<dyn Mount>> {
            Some(Arc::new(MockMount::default()))
        }

        async fn cameras(&self) -> Result<Vec<Arc<dyn Camera>>, ScopinatorError> {
            Ok(vec![Arc::new(MockCamera::default())])
        }

        fn focuser(&self) -> Option<Arc<dyn Focuser>> {
            None
        }

        fn filter_wheel(&self) -> Option<Arc<dyn FilterWheel>> {
            None
        }

        async fn disconnect(&self) -> Result<(), ScopinatorError> {
            self.disconnects.fetch_add(1, Ordering::SeqCst);
            if self.fail_disconnect {
                return Err(ScopinatorError::ConnectionLost("already gone".into()));
            }
            Ok(())
        }

        fn as_any(&self) -> &dyn Any {
            self
        }
    }

    fn ids(manager: &DeviceManager) -> Vec<String> {
        manager
            .list_backends()
            .iter()
            .map(|id| id.to_string())
            .collect()
    }

    #[test]
    fn added_backends_are_found_by_id_and_listed_in_order() {
        let manager = DeviceManager::new();
        let id_b = manager.add(MockBackend::new("mock:b")).unwrap();
        let id_a = manager.add(MockBackend::new("mock:a")).unwrap();
        assert_eq!(id_a.as_str(), "mock:a");
        assert_eq!(id_b.as_str(), "mock:b");

        assert_eq!(ids(&manager), ["mock:a", "mock:b"]);
        assert_eq!(manager.backends().len(), 2);
        assert_eq!(manager.get(&id_a).unwrap().kind(), "mock");
        assert!(manager.get(&DeviceId::new("mock:c")).is_none());
    }

    #[test]
    fn adding_the_same_id_twice_is_refused_and_keeps_the_first() {
        let manager = DeviceManager::new();
        let first = MockBackend::new("mock:a");
        manager.add(Arc::clone(&first) as Arc<dyn Backend>).unwrap();
        let err = manager.add(MockBackend::new("mock:a")).unwrap_err();
        assert!(matches!(err, ScopinatorError::InvalidState(_)), "{err:?}");

        let kept = manager.get(&DeviceId::new("mock:a")).unwrap();
        assert!(std::ptr::eq(
            kept.as_any().downcast_ref::<MockBackend>().unwrap(),
            Arc::as_ptr(&first)
        ));
    }

    #[tokio::test]
    async fn remove_takes_a_backend_out_without_disconnecting_it() {
        let manager = DeviceManager::new();
        let backend = MockBackend::new("mock:a");
        let id = manager
            .add(Arc::clone(&backend) as Arc<dyn Backend>)
            .unwrap();

        assert!(manager.remove(&id).is_some());
        assert!(manager.get(&id).is_none());
        assert!(manager.remove(&id).is_none());
        assert_eq!(backend.disconnects.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn disconnect_all_disconnects_each_backend_once_and_empties_the_manager() {
        let manager = DeviceManager::new();
        let a = MockBackend::new("mock:a");
        let b = MockBackend::new("mock:b");
        manager.add(Arc::clone(&a) as Arc<dyn Backend>).unwrap();
        manager.add(Arc::clone(&b) as Arc<dyn Backend>).unwrap();

        manager.disconnect_all().await.unwrap();
        assert_eq!(a.disconnects.load(Ordering::SeqCst), 1);
        assert_eq!(b.disconnects.load(Ordering::SeqCst), 1);
        assert!(manager.list_backends().is_empty());
    }

    #[tokio::test]
    async fn a_failed_disconnect_does_not_stop_the_others() {
        let manager = DeviceManager::new();
        let failing = Arc::new(MockBackend {
            id: "mock:a",
            disconnects: AtomicUsize::new(0),
            fail_disconnect: true,
        });
        let fine = MockBackend::new("mock:b");
        manager
            .add(Arc::clone(&failing) as Arc<dyn Backend>)
            .unwrap();
        manager.add(Arc::clone(&fine) as Arc<dyn Backend>).unwrap();

        let err = manager.disconnect_all().await.unwrap_err();
        assert!(matches!(err, ScopinatorError::ConnectionLost(_)), "{err:?}");
        assert_eq!(fine.disconnects.load(Ordering::SeqCst), 1);
        assert!(manager.list_backends().is_empty());
    }

    #[tokio::test]
    async fn connecting_to_an_unknown_kind_of_instrument_is_not_supported() {
        let manager = DeviceManager::new();
        let found = crate::discovery::Discovered::new("nexstar", "10.0.0.5:9999".parse().unwrap());
        let err = manager.connect(&found).await.unwrap_err();
        assert!(err.is_not_supported(), "{err:?}");
        assert!(manager.list_backends().is_empty());
    }

    #[cfg(feature = "seestar")]
    #[tokio::test]
    async fn a_seestar_at_an_ipv6_address_is_an_invalid_argument() {
        let manager = DeviceManager::new();
        let found = crate::discovery::Discovered::new("seestar", "[::1]:4700".parse().unwrap());
        let err = manager.connect(&found).await.unwrap_err();
        assert!(
            matches!(err, ScopinatorError::InvalidArgument(_)),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn devices_are_reached_through_the_neutral_traits() {
        let manager = DeviceManager::new();
        let id = manager.add(MockBackend::new("mock:a")).unwrap();
        let backend = manager.get(&id).unwrap();

        let mount = backend.mount().expect("mount");
        assert!(!mount.is_slewing().await.unwrap());
        let camera = main_camera(backend.as_ref())
            .await
            .unwrap()
            .expect("camera");
        assert_eq!(camera.info().name, "camera");
        assert!(backend.focuser().is_none());
        assert!(backend.filter_wheel().is_none());
        // A backend that is not a smart telescope has none of the smart
        // traits, without having to say so.
        assert!(backend.observer().is_none());
        assert!(backend.live_stacker().is_none());
        assert!(backend.auto_focus().is_none());
        assert!(backend.plate_solver().is_none());
    }
}
