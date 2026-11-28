//! ASCOM Alpaca: any mount, camera, focuser, filter wheel or switch behind an
//! Alpaca server, which covers most computer-controlled astronomy gear
//! (natively, or through ASCOM Remote in front of an ASCOM driver).

mod camera;
mod filter_wheel;
mod focuser;
mod mount;
mod poller;
mod switch;

pub use camera::AlpacaCameraDevice;
pub use filter_wheel::AlpacaFilterWheelDevice;
pub use focuser::AlpacaFocuserDevice;
pub use mount::AlpacaMount;
pub use switch::AlpacaSwitchDevice;

use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::time::Duration;

use scopinator_alpaca::{
    AlpacaCamera, AlpacaClient, AlpacaFilterWheel, AlpacaFocuser, AlpacaSwitch, AlpacaTelescope,
};
use scopinator_types::DeviceId;

use crate::backend::Backend;
use crate::device::{Camera, Device, FilterWheel, Focuser, Mount, Switch};
use crate::event::UnifiedEventBus;

/// How to use an Alpaca server.
#[derive(Debug, Clone, PartialEq)]
pub struct AlpacaConfig {
    /// How often device state is read to produce events. Alpaca has no
    /// events of its own.
    pub poll_interval: Duration,
    /// Which `telescope` device to use as the mount, by device number, on a
    /// server that has several. `None` is the lowest-numbered one.
    pub telescope: Option<u32>,
    /// Which `camera` devices to use, in this order. `None` is all of them,
    /// by device number.
    pub cameras: Option<Vec<u32>>,
    /// Which `focuser` device to use. `None` is the lowest-numbered one.
    pub focuser: Option<u32>,
    /// Which `filterwheel` device to use. `None` is the lowest-numbered one.
    pub filter_wheel: Option<u32>,
}

impl Default for AlpacaConfig {
    /// Poll once a second and use the first device of each kind.
    fn default() -> Self {
        Self {
            poll_interval: Duration::from_secs(1),
            telescope: None,
            cameras: None,
            focuser: None,
            filter_wheel: None,
        }
    }
}

/// One Alpaca server as an instrument: its mount, cameras, focuser, filter
/// wheel and switch devices. Other kinds of Alpaca device on the server are
/// left alone.
pub struct AlpacaBackend {
    id: DeviceId,
    client: Arc<AlpacaClient>,
    event_bus: Arc<UnifiedEventBus>,
    mount: Option<Arc<AlpacaMount>>,
    cameras: Vec<Arc<AlpacaCameraDevice>>,
    focuser: Option<Arc<AlpacaFocuserDevice>>,
    filter_wheel: Option<Arc<AlpacaFilterWheelDevice>>,
    /// Every `switch` device on the server, by device number.
    switches: Vec<Arc<AlpacaSwitchDevice>>,
    poller: tokio::task::JoinHandle<()>,
}

/// The device numbers of one type on a server, lowest first.
fn numbers_of(devices: &[scopinator_alpaca::client::ConfiguredDevice], kind: &str) -> Vec<u32> {
    let mut numbers: Vec<u32> = devices
        .iter()
        .filter(|device| device.device_type.eq_ignore_ascii_case(kind))
        .map(|device| device.device_number)
        .collect();
    numbers.sort_unstable();
    numbers.dedup();
    numbers
}

/// The one device of a kind to use: the one asked for, which must exist, or
/// the lowest-numbered.
fn choose(
    kind: &str,
    present: &[u32],
    wanted: Option<u32>,
) -> Result<Option<u32>, ScopinatorError> {
    match wanted {
        None => Ok(present.first().copied()),
        Some(number) if present.contains(&number) => Ok(Some(number)),
        Some(number) => Err(ScopinatorError::InvalidArgument(format!(
            "the server has no {kind} number {number} (it has {present:?})"
        ))),
    }
}

impl AlpacaBackend {
    /// The backend id of the server at `host:port`.
    pub fn id_for(host: Ipv4Addr, port: u16) -> DeviceId {
        DeviceId::new(format!("alpaca:{host}:{port}"))
    }

    /// Connect to the Alpaca server at `host:port`: ask it what devices it
    /// has, connect the ones this library drives, and start reporting their
    /// state changes on `event_bus`.
    pub async fn connect(
        host: Ipv4Addr,
        port: u16,
        config: AlpacaConfig,
        event_bus: Arc<UnifiedEventBus>,
    ) -> Result<Self, ScopinatorError> {
        let id = Self::id_for(host, port);
        let client = Arc::new(AlpacaClient::new(host, port));
        let devices = client.get_configured_devices().await.map_err(neutral)?;
        for device in &devices {
            if !["telescope", "camera", "focuser", "filterwheel", "switch"]
                .iter()
                .any(|kind| device.device_type.eq_ignore_ascii_case(kind))
            {
                tracing::debug!(
                    backend = %id,
                    kind = %device.device_type,
                    name = %device.device_name,
                    "Alpaca device type not driven by this library"
                );
            }
        }

        let telescope = choose(
            "telescope",
            &numbers_of(&devices, "telescope"),
            config.telescope,
        )?;
        let focuser = choose("focuser", &numbers_of(&devices, "focuser"), config.focuser)?;
        let filter_wheel = choose(
            "filterwheel",
            &numbers_of(&devices, "filterwheel"),
            config.filter_wheel,
        )?;
        let present_cameras = numbers_of(&devices, "camera");
        let camera_numbers = match &config.cameras {
            None => present_cameras,
            Some(wanted) => {
                for number in wanted {
                    choose("camera", &present_cameras, Some(*number))?;
                }
                wanted.clone()
            }
        };

        let mount = match telescope {
            Some(number) => {
                let mount =
                    AlpacaMount::new(AlpacaTelescope::new(Arc::clone(&client), number), &id);
                mount.connect().await?;
                Some(Arc::new(mount))
            }
            None => None,
        };
        let mut cameras = Vec::with_capacity(camera_numbers.len());
        for number in camera_numbers {
            let camera =
                AlpacaCameraDevice::new(AlpacaCamera::new(Arc::clone(&client), number), &id);
            camera.connect().await?;
            cameras.push(Arc::new(camera));
        }
        let focuser = match focuser {
            Some(number) => {
                let focuser =
                    AlpacaFocuserDevice::new(AlpacaFocuser::new(Arc::clone(&client), number), &id);
                focuser.connect().await?;
                Some(Arc::new(focuser))
            }
            None => None,
        };
        let filter_wheel = match filter_wheel {
            Some(number) => {
                let wheel = AlpacaFilterWheelDevice::new(
                    AlpacaFilterWheel::new(Arc::clone(&client), number),
                    &id,
                );
                wheel.connect().await?;
                Some(Arc::new(wheel))
            }
            None => None,
        };

        let mut switches = Vec::new();
        for number in numbers_of(&devices, "switch") {
            let switch =
                AlpacaSwitchDevice::new(AlpacaSwitch::new(Arc::clone(&client), number), &id);
            switch.connect().await?;
            switches.push(Arc::new(switch));
        }

        let poller = tokio::spawn(poller::run(
            poller::Watched {
                backend_id: id.clone(),
                mount: mount.clone(),
                cameras: cameras.clone(),
                focuser: focuser.clone(),
                filter_wheel: filter_wheel.clone(),
            },
            Arc::clone(&event_bus),
            config.poll_interval,
        ));

        Ok(Self {
            id,
            client,
            event_bus,
            mount,
            cameras,
            focuser,
            filter_wheel,
            switches,
            poller,
        })
    }

    /// The raw Alpaca client, for device types and properties this library
    /// does not wrap.
    pub fn client(&self) -> &Arc<AlpacaClient> {
        &self.client
    }

    /// The bus this backend reports to.
    pub fn event_bus(&self) -> &Arc<UnifiedEventBus> {
        &self.event_bus
    }

    /// The mount with its Alpaca-specific side, if the server has one.
    pub fn alpaca_mount(&self) -> Option<&Arc<AlpacaMount>> {
        self.mount.as_ref()
    }

    /// The cameras with their Alpaca-specific side.
    pub fn alpaca_cameras(&self) -> &[Arc<AlpacaCameraDevice>] {
        &self.cameras
    }

    /// Every `switch` device on the server, lowest device number first.
    /// [`Backend::switch`] hands out the first.
    pub fn alpaca_switches(&self) -> &[Arc<AlpacaSwitchDevice>] {
        &self.switches
    }
}

impl Drop for AlpacaBackend {
    fn drop(&mut self) {
        self.poller.abort();
    }
}

#[async_trait::async_trait]
impl Backend for AlpacaBackend {
    fn id(&self) -> DeviceId {
        self.id.clone()
    }

    fn kind(&self) -> &'static str {
        "alpaca"
    }

    fn mount(&self) -> Option<Arc<dyn Mount>> {
        self.mount.clone().map(|mount| mount as Arc<dyn Mount>)
    }

    async fn cameras(&self) -> Result<Vec<Arc<dyn Camera>>, ScopinatorError> {
        Ok(self
            .cameras
            .iter()
            .map(|camera| Arc::clone(camera) as Arc<dyn Camera>)
            .collect())
    }

    fn focuser(&self) -> Option<Arc<dyn Focuser>> {
        self.focuser
            .clone()
            .map(|focuser| focuser as Arc<dyn Focuser>)
    }

    fn filter_wheel(&self) -> Option<Arc<dyn FilterWheel>> {
        self.filter_wheel
            .clone()
            .map(|wheel| wheel as Arc<dyn FilterWheel>)
    }

    /// The lowest-numbered `switch` device. A server with several has the
    /// rest in [`AlpacaBackend::alpaca_switches`]. Alpaca has no events, and
    /// switches are not polled for any.
    fn switch(&self) -> Option<Arc<dyn Switch>> {
        self.switches
            .first()
            .map(|switch| Arc::clone(switch) as Arc<dyn Switch>)
    }

    /// Stop reporting events and clear `Connected` on every device. All of
    /// them are attempted; the first error, if any, is returned.
    async fn disconnect(&self) -> Result<(), ScopinatorError> {
        self.poller.abort();
        let mut first_error = None;
        let mut note = |result: Result<(), ScopinatorError>| {
            if let Err(e) = result {
                first_error.get_or_insert(e);
            }
        };
        if let Some(mount) = &self.mount {
            note(mount.disconnect().await);
        }
        for camera in &self.cameras {
            note(camera.disconnect().await);
        }
        if let Some(focuser) = &self.focuser {
            note(focuser.disconnect().await);
        }
        if let Some(wheel) = &self.filter_wheel {
            note(wheel.disconnect().await);
        }
        for switch in &self.switches {
            note(switch.disconnect().await);
        }
        first_error.map_or(Ok(()), Err)
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Finds Alpaca servers with the Alpaca discovery broadcast.
pub struct AlpacaDiscoverer;

#[async_trait::async_trait]
impl crate::discovery::Discoverer for AlpacaDiscoverer {
    fn kind(&self) -> &'static str {
        "alpaca"
    }

    async fn discover(
        &self,
        timeout: Duration,
    ) -> Result<Vec<crate::discovery::Discovered>, ScopinatorError> {
        let servers = scopinator_alpaca::discovery::discover(timeout)
            .await
            .map_err(neutral)?;
        Ok(servers.iter().map(discovered).collect())
    }
}

/// An Alpaca server in the neutral form, named after the devices it has.
fn discovered(
    server: &scopinator_alpaca::discovery::DiscoveredServer,
) -> crate::discovery::Discovered {
    let address = SocketAddr::V4(SocketAddrV4::new(server.address, server.port));
    let mut found = crate::discovery::Discovered::new("alpaca", address);
    let names: Vec<&str> = server
        .devices
        .iter()
        .map(|device| device.device_name.as_str())
        .collect();
    if !names.is_empty() {
        found.name = Some(names.join(", "));
    }
    found
}

use scopinator_alpaca::AlpacaError;
use scopinator_alpaca::error::codes;

use crate::error::ScopinatorError;

/// An Alpaca error as the neutral error a caller can match on.
pub(crate) fn neutral(error: AlpacaError) -> ScopinatorError {
    match error.device_code() {
        _ if error.is_not_implemented() => ScopinatorError::NotSupported(error.to_string()),
        Some(codes::INVALID_WHILE_PARKED) => ScopinatorError::Parked,
        Some(codes::NOT_CONNECTED) => ScopinatorError::NotConnected,
        Some(codes::INVALID_VALUE) => ScopinatorError::OutOfRange(error.to_string()),
        Some(codes::INVALID_OPERATION) => ScopinatorError::InvalidState(error.to_string()),
        _ => ScopinatorError::Alpaca(error),
    }
}

/// The value of an optional property: `None` if the device does not
/// implement it, the error for anything else that goes wrong.
pub(crate) fn optional<T>(result: Result<T, AlpacaError>) -> Result<Option<T>, ScopinatorError> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(e) if e.is_not_implemented() => Ok(None),
        Err(e) => Err(neutral(e)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn device_error(code: i32) -> AlpacaError {
        AlpacaError::Device {
            code,
            message: "m".into(),
        }
    }

    fn device(kind: &str, number: u32, name: &str) -> scopinator_alpaca::client::ConfiguredDevice {
        serde_json::from_value(serde_json::json!({
            "DeviceName": name, "DeviceType": kind, "DeviceNumber": number, "UniqueID": name,
        }))
        .unwrap()
    }

    #[test]
    fn device_numbers_are_found_by_type_whatever_its_case() {
        let devices = [
            device("Camera", 1, "guide"),
            device("Telescope", 0, "mount"),
            device("camera", 0, "main"),
            device("Dome", 0, "dome"),
        ];
        assert_eq!(numbers_of(&devices, "camera"), [0, 1]);
        assert_eq!(numbers_of(&devices, "telescope"), [0]);
        assert!(numbers_of(&devices, "focuser").is_empty());
    }

    #[test]
    fn the_lowest_numbered_device_is_used_unless_one_is_asked_for() {
        assert_eq!(choose("camera", &[2, 5], None).unwrap(), Some(2));
        assert_eq!(choose("camera", &[2, 5], Some(5)).unwrap(), Some(5));
        assert_eq!(choose("camera", &[], None).unwrap(), None);
        // Asking for one that is not there is a mistake, not "none".
        assert!(matches!(
            choose("camera", &[2, 5], Some(3)),
            Err(ScopinatorError::InvalidArgument(_))
        ));
    }

    #[test]
    fn a_discovered_server_is_named_after_its_devices() {
        let server = scopinator_alpaca::discovery::DiscoveredServer {
            address: Ipv4Addr::new(192, 168, 1, 50),
            port: 11111,
            devices: vec![
                device("Telescope", 0, "EQ6"),
                device("Camera", 0, "ASI2600"),
            ],
        };
        let found = discovered(&server);
        assert_eq!(found.backend_kind, "alpaca");
        assert_eq!(found.address.to_string(), "192.168.1.50:11111");
        assert_eq!(found.name.as_deref(), Some("EQ6, ASI2600"));
    }

    #[test]
    fn alpaca_error_numbers_become_neutral_kinds() {
        assert!(matches!(
            neutral(device_error(0x408)),
            ScopinatorError::Parked
        ));
        assert!(matches!(
            neutral(device_error(0x407)),
            ScopinatorError::NotConnected
        ));
        assert!(matches!(
            neutral(device_error(0x401)),
            ScopinatorError::OutOfRange(_)
        ));
        assert!(matches!(
            neutral(device_error(0x40B)),
            ScopinatorError::InvalidState(_)
        ));
        assert!(neutral(AlpacaError::NotImplemented("x".into())).is_not_supported());
        // Anything else keeps the Alpaca error.
        assert!(matches!(
            neutral(device_error(0x500)),
            ScopinatorError::Alpaca(_)
        ));
        assert!(matches!(
            neutral(AlpacaError::Timeout),
            ScopinatorError::Alpaca(_)
        ));
    }

    #[test]
    fn an_unimplemented_property_is_none_and_a_failure_is_an_error() {
        assert_eq!(optional(Ok(5)).unwrap(), Some(5));
        assert_eq!(
            optional::<i32>(Err(AlpacaError::NotImplemented("x".into()))).unwrap(),
            None
        );
        assert!(optional::<i32>(Err(AlpacaError::Timeout)).is_err());
    }
}
