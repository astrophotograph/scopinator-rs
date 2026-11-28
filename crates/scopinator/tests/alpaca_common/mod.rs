//! A fake Alpaca server for tests: a table of property values, a record of
//! every request, and hooks that let a PUT change the table (so a slew can
//! set `slewing`).
#![allow(dead_code)]

use std::collections::HashMap;
use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// A form body or query string, decoded.
pub type Params = HashMap<String, String>;

type Hook = Box<dyn Fn(&mut HashMap<String, Value>, &Params) + Send>;

#[derive(Default)]
struct State {
    /// Property values by `devicetype/number/property`, lower case.
    values: HashMap<String, Value>,
    /// Properties and methods that answer with an Alpaca error.
    errors: HashMap<String, (i32, String)>,
    hooks: HashMap<String, Hook>,
    /// Every PUT, in order: key and form parameters.
    puts: Vec<(String, Params)>,
    /// Every GET, in order: key and query parameters.
    gets: Vec<(String, Params)>,
    /// The reply to an ImageBytes request for a camera's `imagearray`.
    image: Option<Vec<u8>>,
    devices: Value,
    /// Answer every request with HTTP 500.
    broken: bool,
}

/// Handle to a running fake Alpaca server.
pub struct FakeAlpaca {
    pub addr: SocketAddr,
    state: Arc<Mutex<State>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for FakeAlpaca {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl FakeAlpaca {
    pub async fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("address");
        let state = Arc::new(Mutex::new(State {
            devices: json!([]),
            ..State::default()
        }));
        let shared = Arc::clone(&state);
        let task = tokio::spawn(async move {
            loop {
                let Ok((socket, _)) = listener.accept().await else {
                    return;
                };
                tokio::spawn(serve(socket, Arc::clone(&shared)));
            }
        });
        Self { addr, state, task }
    }

    /// Set a property, e.g. `set("telescope/0/slewing", json!(false))`.
    pub fn set(&self, key: &str, value: Value) -> &Self {
        self.state
            .lock()
            .unwrap()
            .values
            .insert(key.to_string(), value);
        self
    }

    /// Set several properties of one device.
    pub fn set_all(&self, device: &str, values: Value) -> &Self {
        let mut state = self.state.lock().unwrap();
        for (property, value) in values.as_object().expect("an object of properties") {
            state
                .values
                .insert(format!("{device}/{property}"), value.clone());
        }
        self
    }

    /// Make a property or method answer with an Alpaca error.
    pub fn fail(&self, key: &str, code: i32, message: &str) -> &Self {
        self.state
            .lock()
            .unwrap()
            .errors
            .insert(key.to_string(), (code, message.to_string()));
        self
    }

    /// Make a property or method answer "not implemented" (0x400).
    pub fn not_implemented(&self, key: &str) -> &Self {
        self.fail(key, 0x400, "not implemented")
    }

    /// Run `hook` on the property table when `key` is PUT.
    pub fn on_put(
        &self,
        key: &str,
        hook: impl Fn(&mut HashMap<String, Value>, &Params) + Send + 'static,
    ) -> &Self {
        self.state
            .lock()
            .unwrap()
            .hooks
            .insert(key.to_string(), Box::new(hook));
        self
    }

    pub fn set_devices(&self, devices: Value) -> &Self {
        self.state.lock().unwrap().devices = devices;
        self
    }

    pub fn set_image(&self, bytes: Vec<u8>) -> &Self {
        self.state.lock().unwrap().image = Some(bytes);
        self
    }

    pub fn set_broken(&self, broken: bool) -> &Self {
        self.state.lock().unwrap().broken = broken;
        self
    }

    /// The form parameters of every PUT to `key`, in order.
    pub fn puts(&self, key: &str) -> Vec<Params> {
        self.state
            .lock()
            .unwrap()
            .puts
            .iter()
            .filter(|(k, _)| k == key)
            .map(|(_, params)| params.clone())
            .collect()
    }

    /// The keys of every PUT, in order.
    pub fn put_keys(&self) -> Vec<String> {
        self.state
            .lock()
            .unwrap()
            .puts
            .iter()
            .map(|(key, _)| key.clone())
            .collect()
    }

    /// How many times `key` has been read.
    pub fn get_count(&self, key: &str) -> usize {
        self.state
            .lock()
            .unwrap()
            .gets
            .iter()
            .filter(|(k, _)| k == key)
            .count()
    }

    /// How many requests of any kind have arrived.
    pub fn request_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.gets.len() + state.puts.len()
    }
}

/// A telescope with everything a full-featured mount reports, unparked and
/// idle at RA 6 h, Dec +20, working in J2000.
pub fn full_telescope() -> Value {
    json!({
        "connected": true, "name": "Fake Mount", "driverinfo": "fake driver 1.0",
        "rightascension": 6.0, "declination": 20.0, "altitude": 45.0, "azimuth": 180.0,
        "slewing": false, "atpark": false, "athome": false, "tracking": true,
        "sideofpier": 0, "trackingrate": 0, "trackingrates": [0, 1, 2],
        "equatorialsystem": 2, "alignmentmode": 2,
        "canslew": true, "canslewasync": true, "canslewaltazasync": true, "cansync": true,
        "canpark": true, "canunpark": true, "cansettracking": true, "canfindhome": true,
        "canpulseguide": true, "canmoveaxis": true,
        "axisrates": [{"Minimum": 0.0, "Maximum": 4.0}],
        "sitelatitude": 41.88, "sitelongitude": -87.63, "siteelevation": 180.0,
        "utcdate": "2026-11-15T17:18:54.0000000Z",
    })
}

async fn serve(mut socket: TcpStream, state: Arc<Mutex<State>>) {
    let Some(request) = read_request(&mut socket).await else {
        return;
    };
    let (status, content_type, body) = respond(&request, &state);
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    let _ = socket.write_all(head.as_bytes()).await;
    let _ = socket.write_all(&body).await;
    let _ = socket.shutdown().await;
}

struct Request {
    method: String,
    path: String,
    query: Params,
    form: Params,
    accept: String,
}

fn respond(request: &Request, state: &Arc<Mutex<State>>) -> (&'static str, &'static str, Vec<u8>) {
    let mut state = state.lock().unwrap();
    let json_ok = |value: Value| ("200 OK", "application/json", value.to_string().into_bytes());
    if state.broken {
        return (
            "500 Internal Server Error",
            "text/plain",
            b"broken".to_vec(),
        );
    }
    if request.path == "/management/v1/configureddevices" {
        return json_ok(json!({"Value": state.devices}));
    }
    let Some(key) = request.path.strip_prefix("/api/v1/") else {
        return ("404 Not Found", "text/plain", b"no such path".to_vec());
    };
    let mut key = key.to_ascii_lowercase();
    let is_put = request.method == "PUT";
    // A property of one switch: `switch/0/getswitchvalue?id=1`, when the
    // test has set that.
    if !is_put
        && let Some(id) = request
            .query
            .iter()
            .find(|(name, _)| name.eq_ignore_ascii_case("id"))
            .map(|(_, id)| id)
    {
        let of_one = format!("{key}?id={id}");
        if state.values.contains_key(&of_one) || state.errors.contains_key(&of_one) {
            key = of_one;
        }
    }
    if is_put {
        state.puts.push((key.clone(), request.form.clone()));
    } else {
        state.gets.push((key.clone(), request.query.clone()));
    }
    if let Some((code, message)) = state.errors.get(&key).cloned() {
        return json_ok(json!({"ErrorNumber": code, "ErrorMessage": message}));
    }
    if is_put {
        if let Some(hook) = state.hooks.remove(&key) {
            hook(&mut state.values, &request.form);
            state.hooks.insert(key, hook);
        }
        return json_ok(json!({"ErrorNumber": 0, "ErrorMessage": ""}));
    }
    if key.ends_with("/imagearray")
        && request.accept.contains("application/imagebytes")
        && let Some(image) = state.image.clone()
    {
        return ("200 OK", "application/imagebytes", image);
    }
    match state.values.get(&key) {
        Some(value) => json_ok(json!({"Value": value, "ErrorNumber": 0, "ErrorMessage": ""})),
        // A property the test did not set: say so loudly rather than
        // inventing a value.
        None => json_ok(
            json!({"ErrorNumber": 0x4FF, "ErrorMessage": format!("fake: `{key}` is not set")}),
        ),
    }
}

async fn read_request(socket: &mut TcpStream) -> Option<Request> {
    let mut buf: Vec<u8> = Vec::new();
    let mut chunk = [0u8; 2048];
    let header_end = loop {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break pos + 4;
        }
    };
    let head = String::from_utf8_lossy(&buf[..header_end]).into_owned();
    let mut lines = head.lines();
    let mut request_line = lines.next()?.split(' ');
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let mut content_length = 0usize;
    let mut accept = String::new();
    for line in lines {
        if let Some((name, value)) = line.split_once(':') {
            match name.trim().to_ascii_lowercase().as_str() {
                "content-length" => content_length = value.trim().parse().unwrap_or(0),
                "accept" => accept = value.trim().to_ascii_lowercase(),
                _ => {}
            }
        }
    }
    while buf.len() < header_end + content_length {
        let n = socket.read(&mut chunk).await.ok()?;
        if n == 0 {
            return None;
        }
        buf.extend_from_slice(&chunk[..n]);
    }
    let body = String::from_utf8_lossy(&buf[header_end..header_end + content_length]).into_owned();
    let (path, query) = target.split_once('?').unwrap_or((&target, ""));
    Some(Request {
        method,
        path: path.to_string(),
        query: decode(query),
        form: decode(&body),
        accept,
    })
}

/// Decode `a=1&b=x%3Ay` into a map.
fn decode(encoded: &str) -> Params {
    encoded
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| {
            let (name, value) = pair.split_once('=').unwrap_or((pair, ""));
            (percent_decode(name), percent_decode(value))
        })
        .collect()
}

fn percent_decode(text: &str) -> String {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let escaped = (bytes[i] == b'%')
            .then(|| bytes.get(i + 1..i + 3))
            .flatten()
            .and_then(|hex| std::str::from_utf8(hex).ok())
            .and_then(|hex| u8::from_str_radix(hex, 16).ok());
        match (bytes[i], escaped) {
            (_, Some(byte)) => {
                out.push(byte);
                i += 2;
            }
            (b'+', None) => out.push(b' '),
            (other, None) => out.push(other),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}
