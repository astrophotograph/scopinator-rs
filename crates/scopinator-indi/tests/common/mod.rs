//! A scriptable INDI server on a loopback port, for tests: it sends the
//! definitions it was given when a client asks for properties, records
//! everything clients send, and can answer a `new*Vector` with a scripted
//! reply.
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

enum ToClient {
    Send {
        bytes: Vec<u8>,
        chunk: Option<usize>,
    },
    Close,
}

#[derive(Default)]
struct State {
    /// `def*Vector` elements, sent to a client that asks for properties.
    definitions: Vec<String>,
    /// Every top-level element any client has sent, in order.
    received: Vec<String>,
    /// The reply to a `new*Vector` for a device and property.
    replies: HashMap<(String, String), String>,
    clients: Vec<mpsc::UnboundedSender<ToClient>>,
    connections: usize,
}

/// Handle to a running fake INDI server.
pub struct FakeIndi {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeIndi {
    fn drop(&mut self) {
        self.task.abort();
        self.drop_connections();
    }
}

impl FakeIndi {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("address");
        let state = Arc::new(Mutex::new(State::default()));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                let (tx, rx) = mpsc::unbounded_channel();
                {
                    let mut state = shared.lock().unwrap();
                    state.clients.push(tx);
                    state.connections += 1;
                }
                tokio::spawn(serve(socket, rx, Arc::clone(&shared)));
            }
        });
        Self { addr, state, task }
    }

    /// Remember a `def*Vector` element, to send whenever a client asks for
    /// properties.
    pub fn define(&self, xml: &str) -> &Self {
        self.state.lock().unwrap().definitions.push(xml.to_string());
        self
    }

    /// Forget every definition (for a server that comes back different).
    pub fn clear_definitions(&self) -> &Self {
        self.state.lock().unwrap().definitions.clear();
        self
    }

    /// Write raw XML to every connected client now.
    pub fn send(&self, xml: &str) -> &Self {
        self.to_all(xml, None)
    }

    /// The same, in writes of `chunk` bytes with a flush after each.
    pub fn send_in_chunks(&self, xml: &str, chunk: usize) -> &Self {
        self.to_all(xml, Some(chunk))
    }

    fn to_all(&self, xml: &str, chunk: Option<usize>) -> &Self {
        let mut state = self.state.lock().unwrap();
        state.clients.retain(|client| {
            client
                .send(ToClient::Send {
                    bytes: xml.as_bytes().to_vec(),
                    chunk,
                })
                .is_ok()
        });
        self
    }

    /// Answer a `new*Vector` for `device` and `property` with `reply_xml`.
    pub fn on_new(&self, device: &str, property: &str, reply_xml: &str) -> &Self {
        self.state.lock().unwrap().replies.insert(
            (device.to_string(), property.to_string()),
            reply_xml.to_string(),
        );
        self
    }

    /// Every top-level element received so far, as text.
    pub fn received(&self) -> Vec<String> {
        self.state.lock().unwrap().received.clone()
    }

    /// The `new*Vector` elements received for one device and property.
    pub fn received_new(&self, device: &str, property: &str) -> Vec<String> {
        self.received()
            .into_iter()
            .filter(|element| {
                let (name, attributes) = describe(element);
                name.starts_with("new")
                    && attributes.get("device").map(String::as_str) == Some(device)
                    && attributes.get("name").map(String::as_str) == Some(property)
            })
            .collect()
    }

    /// How many elements named `element` have been received.
    pub fn count(&self, element: &str) -> usize {
        self.received()
            .iter()
            .filter(|text| describe(text).0 == element)
            .count()
    }

    /// Close every client socket. The listener stays up.
    pub fn drop_connections(&self) {
        let mut state = self.state.lock().unwrap();
        for client in state.clients.drain(..) {
            let _ = client.send(ToClient::Close);
        }
    }

    /// How many connections have been accepted so far.
    pub fn connection_count(&self) -> usize {
        self.state.lock().unwrap().connections
    }
}

/// The name of an element and its attributes.
pub fn describe(element: &str) -> (String, HashMap<String, String>) {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(element);
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag) | Event::Empty(tag)) => {
                let name = String::from_utf8_lossy(tag.name().as_ref()).into_owned();
                let attributes = tag
                    .attributes()
                    .flatten()
                    .map(|attribute| {
                        (
                            String::from_utf8_lossy(attribute.key.as_ref()).into_owned(),
                            attribute
                                .unescape_value()
                                .map(|value| value.into_owned())
                                .unwrap_or_default(),
                        )
                    })
                    .collect();
                return (name, attributes);
            }
            Ok(Event::Eof) | Err(_) => return (String::new(), HashMap::new()),
            Ok(_) => {}
        }
    }
}

/// The text of one member of an element, e.g. `member(xml, "oneSwitch",
/// "CONNECT")`.
pub fn member(element: &str, tag: &str, name: &str) -> Option<String> {
    use quick_xml::events::Event;

    let mut reader = quick_xml::Reader::from_str(element);
    let mut inside = false;
    let mut text = String::new();
    loop {
        match reader.read_event().ok()? {
            Event::Start(start) if start.name().as_ref() == tag.as_bytes() => {
                let attribute = start.try_get_attribute("name").ok()??;
                inside = attribute.unescape_value().ok()?.as_ref() == name;
            }
            Event::Text(part) if inside => text.push_str(&part.unescape().ok()?),
            Event::End(end) if inside && end.name().as_ref() == tag.as_bytes() => {
                return Some(text.trim().to_string());
            }
            Event::Eof => return None,
            _ => {}
        }
    }
}

/// Take the complete top-level elements off the front of `buf`.
fn split_elements(buf: &mut Vec<u8>) -> Vec<String> {
    let mut elements = Vec::new();
    let (mut at, mut depth, mut start) = (0, 0usize, 0);
    while let Some(offset) = buf[at..].iter().position(|&b| b == b'<') {
        let open = at + offset;
        if depth == 0 {
            start = open;
        }
        let mut quote = None;
        let mut end = None;
        for (index, &byte) in buf.iter().enumerate().skip(open + 1) {
            match quote {
                Some(q) if byte == q => quote = None,
                Some(_) => {}
                None if byte == b'"' || byte == b'\'' => quote = Some(byte),
                None if byte == b'>' => {
                    end = Some(index);
                    break;
                }
                None => {}
            }
        }
        let Some(end) = end else { break };
        if buf[open + 1] == b'/' {
            depth = depth.saturating_sub(1);
        } else if buf[end - 1] != b'/' && buf[open + 1] != b'?' && buf[open + 1] != b'!' {
            depth += 1;
        }
        at = end + 1;
        if depth == 0 {
            elements.push(String::from_utf8_lossy(&buf[start..at]).into_owned());
            start = at;
        }
    }
    if depth == 0 {
        start = at.max(start);
    }
    buf.drain(..start);
    elements
}

async fn serve(
    mut socket: TcpStream,
    mut commands: mpsc::UnboundedReceiver<ToClient>,
    state: Arc<Mutex<State>>,
) {
    let mut pending = Vec::new();
    let mut chunk = [0u8; 4096];
    loop {
        tokio::select! {
            read = socket.read(&mut chunk) => {
                let Ok(n) = read else { return };
                if n == 0 {
                    return;
                }
                pending.extend_from_slice(&chunk[..n]);
                for element in split_elements(&mut pending) {
                    let (name, attributes) = describe(&element);
                    let mut answers = Vec::new();
                    {
                        let mut state = state.lock().unwrap();
                        state.received.push(element);
                        if name == "getProperties" {
                            answers.extend(state.definitions.iter().cloned());
                        } else if name.starts_with("new") {
                            let key = (
                                attributes.get("device").cloned().unwrap_or_default(),
                                attributes.get("name").cloned().unwrap_or_default(),
                            );
                            answers.extend(state.replies.get(&key).cloned());
                        }
                    }
                    for answer in answers {
                        if socket.write_all(answer.as_bytes()).await.is_err() {
                            return;
                        }
                    }
                }
            }
            command = commands.recv() => match command {
                Some(ToClient::Send { bytes, chunk }) => {
                    let size = chunk.unwrap_or(bytes.len()).max(1);
                    for piece in bytes.chunks(size) {
                        if socket.write_all(piece).await.is_err() || socket.flush().await.is_err() {
                            return;
                        }
                        if chunk.is_some() {
                            tokio::task::yield_now().await;
                        }
                    }
                }
                Some(ToClient::Close) | None => {
                    let _ = socket.shutdown().await;
                    return;
                }
            }
        }
    }
}
