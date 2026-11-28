//! An INDI client: one connection to an `indiserver`, a live copy of every
//! property it has defined, and the means to change one and wait for the
//! device to act on it.
//!
//! INDI pushes: the server defines its properties when asked and then
//! reports every change. The client mirrors that, so reading a property is
//! a lookup, not a request.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, info, warn};

use crate::error::IndiError;
use crate::protocol::message::{IndiMessage, VectorKind};
use crate::protocol::parse::StreamParser;
use crate::protocol::property::SwitchState;
use crate::protocol::xml::{IndiCommand, serialize_command};
use crate::store::{Property, Store};

/// The port `indiserver` listens on unless told otherwise.
pub const DEFAULT_PORT: u16 = 7624;

/// How an [`IndiClient`] connects.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub struct IndiClientConfig {
    /// How long one attempt to open the connection may take.
    pub connect_timeout: Duration,
    /// How long to wait before the first attempt to reconnect. Each failed
    /// attempt doubles it, up to `max_reconnect_delay`.
    pub reconnect_delay: Duration,
    pub max_reconnect_delay: Duration,
}

impl Default for IndiClientConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            reconnect_delay: Duration::from_secs(1),
            max_reconnect_delay: Duration::from_secs(30),
        }
    }
}

impl IndiClientConfig {
    pub fn with_connect_timeout(mut self, timeout: Duration) -> Self {
        self.connect_timeout = timeout;
        self
    }

    pub fn with_reconnect_delay(mut self, first: Duration, longest: Duration) -> Self {
        self.reconnect_delay = first;
        self.max_reconnect_delay = longest.max(first);
        self
    }
}

/// A connection to an INDI server.
pub struct IndiClient {
    addr: SocketAddr,
    store: Arc<Store>,
    outgoing: mpsc::Sender<String>,
    connected: Arc<AtomicBool>,
    messages: broadcast::Sender<IndiMessage>,
    shutdown: watch::Sender<bool>,
}

impl std::fmt::Debug for IndiClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndiClient")
            .field("addr", &self.addr)
            .field("connected", &self.is_connected())
            .finish_non_exhaustive()
    }
}

fn kind_name(kind: VectorKind) -> &'static str {
    match kind {
        VectorKind::Number => "number",
        VectorKind::Switch => "switch",
        VectorKind::Text => "text",
        VectorKind::Light => "light",
        VectorKind::Blob => "BLOB",
    }
}

async fn open(addr: SocketAddr, timeout: Duration) -> Result<TcpStream, IndiError> {
    match tokio::time::timeout(timeout, TcpStream::connect(addr)).await {
        Ok(Ok(stream)) => Ok(stream),
        Ok(Err(e)) => Err(IndiError::Connection(e)),
        Err(_) => Err(IndiError::Timeout),
    }
}

impl IndiClient {
    /// Open the connection and ask the server for its properties. This
    /// returns once the socket is up; the definitions arrive after it, and
    /// [`wait_for`](Self::wait_for) waits for one.
    pub async fn connect(addr: SocketAddr, config: IndiClientConfig) -> Result<Self, IndiError> {
        let stream = open(addr, config.connect_timeout).await?;
        let store = Arc::new(Store::default());
        let connected = Arc::new(AtomicBool::new(false));
        let (outgoing, outgoing_rx) = mpsc::channel(64);
        let (messages, _) = broadcast::channel(256);
        let (shutdown, shutdown_rx) = watch::channel(false);
        // Up before the first read or write, so that what the caller does
        // next finds the connection.
        connected.store(true, Ordering::Release);
        tokio::spawn(run(
            Link {
                addr,
                config,
                store: Arc::clone(&store),
                connected: Arc::clone(&connected),
                messages: messages.clone(),
            },
            stream,
            outgoing_rx,
            shutdown_rx,
        ));
        Ok(Self {
            addr,
            store,
            outgoing,
            connected,
            messages,
            shutdown,
        })
    }

    /// The server's address.
    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    pub fn is_connected(&self) -> bool {
        self.connected.load(Ordering::Acquire)
    }

    /// Close the connection and stop reconnecting.
    pub async fn shutdown(&self) {
        let _ = self.shutdown.send(true);
        for _ in 0..100 {
            if !self.is_connected() {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    }

    /// Every device the server has defined a property for, by name.
    pub fn devices(&self) -> Vec<String> {
        self.store.devices()
    }

    /// One property as it is now, or `None` if it is not (or is no longer)
    /// defined.
    pub fn property(&self, device: &str, name: &str) -> Option<Property> {
        self.store.get(device, name)
    }

    /// Every property of one device, by name.
    pub fn properties(&self, device: &str) -> Vec<Property> {
        self.store.of_device(device)
    }

    /// Every message as it arrives: definitions, updates, deletions and
    /// text messages.
    pub fn subscribe(&self) -> broadcast::Receiver<IndiMessage> {
        self.messages.subscribe()
    }

    /// The property, if it can be written as `kind`.
    fn writable(&self, device: &str, name: &str, kind: VectorKind) -> Result<Property, IndiError> {
        if !self.is_connected() {
            return Err(IndiError::NotConnected);
        }
        let property = self
            .store
            .get(device, name)
            .ok_or_else(|| IndiError::PropertyNotFound {
                device: device.to_string(),
                name: name.to_string(),
            })?;
        if property.kind != kind {
            return Err(IndiError::WrongKind {
                device: device.to_string(),
                name: name.to_string(),
                actual: kind_name(property.kind),
                wanted: kind_name(kind),
            });
        }
        if !property.permission.can_write() {
            return Err(IndiError::ReadOnly {
                device: device.to_string(),
                name: name.to_string(),
            });
        }
        Ok(property)
    }

    fn no_member(property: &Property, member: &str) -> IndiError {
        IndiError::MemberNotFound {
            device: property.device.clone(),
            name: property.name.clone(),
            member: member.to_string(),
        }
    }

    async fn send(&self, command: IndiCommand) -> Result<(), IndiError> {
        self.outgoing
            .send(serialize_command(&command))
            .await
            .map_err(|_| IndiError::NotConnected)
    }

    /// Ask for new values of number members. Members left out keep their
    /// values. Nothing is sent if the property is not defined, cannot be
    /// written, has no such member, or a value is outside the member's
    /// limits.
    pub async fn set_numbers(
        &self,
        device: &str,
        name: &str,
        values: &[(&str, f64)],
    ) -> Result<(), IndiError> {
        let property = self.writable(device, name, VectorKind::Number)?;
        for (member, value) in values {
            let definition = property
                .members
                .iter()
                .find(|defined| defined.name == *member)
                .ok_or_else(|| Self::no_member(&property, member))?;
            let (min, max) = definition
                .limits
                .as_ref()
                .map_or((0.0, 0.0), |limits| (limits.min, limits.max));
            // A member defined with max no greater than min has no limits.
            let outside = max > min && !(min..=max).contains(value);
            if !value.is_finite() || outside {
                return Err(IndiError::OutOfRange {
                    device: device.to_string(),
                    name: name.to_string(),
                    member: member.to_string(),
                    value: *value,
                    min,
                    max,
                });
            }
        }
        self.send(IndiCommand::NewNumber {
            device: device.to_string(),
            name: name.to_string(),
            values: values
                .iter()
                .map(|(member, value)| (member.to_string(), *value))
                .collect(),
        })
        .await
    }

    /// Ask for switch members to be turned on or off.
    pub async fn set_switches(
        &self,
        device: &str,
        name: &str,
        values: &[(&str, SwitchState)],
    ) -> Result<(), IndiError> {
        let property = self.writable(device, name, VectorKind::Switch)?;
        for (member, _) in values {
            if property.member(member).is_none() {
                return Err(Self::no_member(&property, member));
            }
        }
        self.send(IndiCommand::NewSwitch {
            device: device.to_string(),
            name: name.to_string(),
            values: values
                .iter()
                .map(|(member, state)| (member.to_string(), *state))
                .collect(),
        })
        .await
    }

    /// Ask for new values of text members.
    pub async fn set_text(
        &self,
        device: &str,
        name: &str,
        values: &[(&str, &str)],
    ) -> Result<(), IndiError> {
        let property = self.writable(device, name, VectorKind::Text)?;
        for (member, _) in values {
            if property.member(member).is_none() {
                return Err(Self::no_member(&property, member));
            }
        }
        self.send(IndiCommand::NewText {
            device: device.to_string(),
            name: name.to_string(),
            values: values
                .iter()
                .map(|(member, value)| (member.to_string(), value.to_string()))
                .collect(),
        })
        .await
    }

    /// Wait until the property is defined and `done` is true of it, and
    /// return it as it then is. [`IndiError::Timeout`] if `timeout` passes
    /// first. A property in `Alert` does not end the wait by itself: `done`
    /// decides what counts.
    pub async fn wait_for(
        &self,
        device: &str,
        name: &str,
        timeout: Duration,
        done: impl Fn(&Property) -> bool,
    ) -> Result<Property, IndiError> {
        let mut changes = self.store.watch();
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            if let Some(property) = self.store.get(device, name)
                && done(&property)
            {
                return Ok(property);
            }
            match tokio::time::timeout_at(deadline, changes.changed()).await {
                Ok(Ok(())) => {}
                Ok(Err(_)) => return Err(IndiError::NotConnected),
                Err(_) => return Err(IndiError::Timeout),
            }
        }
    }
}

impl Drop for IndiClient {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
    }
}

/// What the connection task shares with the client.
struct Link {
    addr: SocketAddr,
    config: IndiClientConfig,
    store: Arc<Store>,
    connected: Arc<AtomicBool>,
    messages: broadcast::Sender<IndiMessage>,
}

enum Ended {
    /// The client asked for it, or is gone.
    Shutdown,
    /// The server went away.
    Lost,
}

/// Keep a connection to the server for as long as the client lives.
async fn run(
    link: Link,
    first: TcpStream,
    mut outgoing: mpsc::Receiver<String>,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut stream = Some(first);
    let mut delay = link.config.reconnect_delay;
    loop {
        let socket = match stream.take() {
            Some(socket) => socket,
            None => {
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    _ = shutdown.changed() => return,
                }
                if *shutdown.borrow() {
                    return;
                }
                match open(link.addr, link.config.connect_timeout).await {
                    Ok(socket) => socket,
                    Err(e) => {
                        debug!(addr = %link.addr, "could not reconnect to the INDI server: {e}");
                        delay = (delay * 2).min(link.config.max_reconnect_delay);
                        continue;
                    }
                }
            }
        };
        delay = link.config.reconnect_delay;
        // Whatever was asked for while the server was away is not sent to
        // it now: a slew meant for a minute ago is not meant for now.
        while outgoing.try_recv().is_ok() {}

        let ended = serve(&link, socket, &mut outgoing, &mut shutdown).await;
        link.connected.store(false, Ordering::Release);
        link.store.clear();
        match ended {
            Ended::Shutdown => return,
            Ended::Lost => info!(addr = %link.addr, "INDI connection lost, will reconnect"),
        }
    }
}

async fn serve(
    link: &Link,
    mut socket: TcpStream,
    outgoing: &mut mpsc::Receiver<String>,
    shutdown: &mut watch::Receiver<bool>,
) -> Ended {
    if let Err(e) = socket.set_nodelay(true) {
        debug!("could not set TCP_NODELAY: {e}");
    }
    let hello = serialize_command(&IndiCommand::GetProperties {
        device: None,
        name: None,
    });
    if socket.write_all(hello.as_bytes()).await.is_err() {
        return Ended::Lost;
    }
    link.connected.store(true, Ordering::Release);

    let mut parser = StreamParser::new();
    let mut chunk = vec![0u8; 64 * 1024];
    loop {
        tokio::select! {
            read = socket.read(&mut chunk) => {
                let n = match read {
                    Ok(0) | Err(_) => return Ended::Lost,
                    Ok(n) => n,
                };
                for message in parser.feed(&chunk[..n]) {
                    match message {
                        Ok(message) => {
                            if !link.store.apply(&message) {
                                debug!("an update for a property that was never defined");
                            }
                            // Nobody listening is not an error.
                            let _ = link.messages.send(message);
                        }
                        Err(e) => warn!(addr = %link.addr, "unreadable INDI message: {e}"),
                    }
                }
            }
            command = outgoing.recv() => match command {
                Some(xml) => {
                    if socket.write_all(xml.as_bytes()).await.is_err() {
                        return Ended::Lost;
                    }
                }
                None => return Ended::Shutdown,
            },
            changed = shutdown.changed() => {
                if changed.is_err() || *shutdown.borrow() {
                    let _ = socket.shutdown().await;
                    return Ended::Shutdown;
                }
            }
        }
    }
}
