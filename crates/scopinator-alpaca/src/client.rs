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

#[cfg(test)]
pub(crate) mod tests {
    // Mirrors of the pyscopinator Alpaca oracle tests
    // (pyscopinator/tests/v2/backends/test_alpaca.py, 14 cases).
    //
    // Ported (Rust test <- Python case):
    // - client_new_targets_host_and_port
    //   <- TestAlpacaBackend::test_backend_initialization
    // - client_is_usable_immediately_after_new
    //   <- TestAlpacaBackend::test_backend_connect
    // - get_configured_devices_parses_devices
    //   <- TestAlpacaDiscovery::test_discover_devices
    // - get_configured_devices_failure_is_propagated
    //   <- TestAlpacaDiscovery::test_discover_devices_error
    //
    // Skipped:
    // - TestAlpacaMount::{test_mount_initialization, test_get_coordinates,
    //   test_slew_to_coordinates, test_sync_to_coordinates, test_park} — the
    //   Rust port has no mount device class (pyscopinator's
    //   backends/alpaca/mount.py has no Rust counterpart).
    // - TestAlpacaCamera::{test_camera_initialization, test_start_exposure,
    //   test_abort_exposure, test_is_exposing} — same, no camera class.
    // - TestAlpacaBackend::test_backend_disconnect — the Rust port has no
    //   session lifecycle; the reqwest::Client is closed when the
    //   AlpacaClient is dropped, so there is no disconnect() to test.
    //
    // The Python mocks only ever answer with well-formed 200 envelopes, so
    // the request-formation, envelope, malformed-JSON and non-200 cases
    // below have no direct Python case; they pin the same Alpaca contract
    // the Python client implements in mount.py
    // (AlpacaMount::_get/_put/_check_error).

    use super::*;
    use std::net::SocketAddr;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use tokio::net::{TcpListener, TcpStream};
    use tokio::task::JoinHandle;

    /// Serve `count` sequential HTTP/1.1 connections on a fresh loopback
    /// listener, answering each with the same canned response (with
    /// `Content-Length` and `Connection: close`), and return the listener's
    /// address plus a handle for the raw request texts.
    ///
    /// Shared with `discovery::tests` — a raw-TCP stand-in for the aiohttp
    /// session the Python oracle mocks.
    pub(crate) async fn serve(
        status: &'static str,
        content_type: &'static str,
        body: &'static str,
        count: usize,
    ) -> (SocketAddr, JoinHandle<Vec<String>>) {
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind loopback listener");
        let addr = listener.local_addr().expect("listener address");

        let handle = tokio::spawn(async move {
            let mut requests = Vec::new();
            for _ in 0..count {
                let (mut socket, _) = listener.accept().await.expect("accept");
                let request = read_request(&mut socket).await;
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len(),
                );
                socket
                    .write_all(response.as_bytes())
                    .await
                    .expect("write canned response");
                requests.push(request);
            }
            requests
        });

        (addr, handle)
    }

    /// Read one full HTTP request: the headers, then `Content-Length` bytes
    /// of body. Anything past the declared body belongs to the next request
    /// on the connection and is dropped (the test double serves exactly one
    /// request per connection).
    async fn read_request(socket: &mut TcpStream) -> String {
        let mut buf: Vec<u8> = Vec::new();
        let mut chunk = [0u8; 1024];

        let header_end = loop {
            let n = socket.read(&mut chunk).await.expect("read request");
            assert!(n > 0, "client closed connection before sending a request");
            buf.extend_from_slice(&chunk[..n]);
            if let Some(pos) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                break pos + 4;
            }
        };

        let headers = String::from_utf8_lossy(&buf[..header_end]).into_owned();
        let mut content_length = 0usize;
        for line in headers.lines() {
            if let Some((name, value)) = line.split_once(':')
                && name.trim().eq_ignore_ascii_case("content-length")
            {
                content_length = value.trim().parse().unwrap_or(0);
            }
        }

        let request_end = header_end + content_length;
        while buf.len() < request_end {
            let n = socket.read(&mut chunk).await.expect("read request body");
            assert!(n > 0, "client closed connection mid-request");
            buf.extend_from_slice(&chunk[..n]);
        }

        String::from_utf8_lossy(&buf[..request_end]).into_owned()
    }

    /// Split a raw HTTP request into its method, path, and query parameters.
    pub(crate) fn request_line_parts(request: &str) -> (&str, &str, Vec<String>) {
        let request_line = request.lines().next().expect("request line");
        let mut parts = request_line.split(' ');
        let method = parts.next().expect("method");
        let target = parts.next().expect("request target");
        let (path, query) = target.split_once('?').unwrap_or((target, ""));
        let params = query
            .split('&')
            .filter(|p| !p.is_empty())
            .map(str::to_string)
            .collect();
        (method, path, params)
    }

    /// A loopback port with (almost certainly) nothing listening on it.
    pub(crate) fn refused_port() -> u16 {
        let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).expect("bind");
        listener.local_addr().expect("local addr").port()
        // the listener is dropped here, releasing the port
    }

    const JSON_OK: &str = r#"{"Value": 1, "ErrorNumber": 0, "ErrorMessage": ""}"#;

    // Mirrors TestAlpacaBackend::test_backend_initialization.
    //
    // Python asserts AlpacaBackend(host, port) stores host and port; the Rust
    // AlpacaClient::new(host, port) exposes no accessors for them, so the
    // same contract is pinned through the request it sends to that host:port.
    #[tokio::test]
    async fn client_new_targets_host_and_port() {
        let (addr, handle) = serve("200 OK", "application/json", JSON_OK, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        client
            .get_property("telescope", 0, "connected")
            .await
            .unwrap();

        let request = handle.await.unwrap().pop().expect("one request");
        let (method, _, _) = request_line_parts(&request);
        assert_eq!(method, "GET");
        let lower = request.to_ascii_lowercase();
        assert!(
            lower.contains(&format!("host: 127.0.0.1:{}", addr.port())),
            "request must target the configured host:port: {request}"
        );
    }

    // Mirrors TestAlpacaBackend::test_backend_connect.
    //
    // Python's connect() creates the aiohttp session and the test asserts the
    // session exists. The Rust port has no connect(): AlpacaClient::new
    // already builds the reqwest::Client, so a request must succeed
    // immediately after construction.
    #[tokio::test]
    async fn client_is_usable_immediately_after_new() {
        let (addr, handle) = serve(
            "200 OK",
            "application/json",
            r#"{"Value": true, "ErrorNumber": 0}"#,
            1,
        )
        .await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let value = client
            .get_property("telescope", 0, "connected")
            .await
            .expect("request without connect()");

        assert_eq!(value, serde_json::json!(true));
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // No direct Python case: the Python mocks never inspect the URLs their
    // session receives. This pins the request formation performed by
    // AlpacaMount::_get — path /api/v1/<device>/<number>/<property> with
    // ClientID and a per-client incrementing ClientTransactionID as query
    // parameters — which the oracle could only assert indirectly.
    #[tokio::test]
    async fn get_property_forms_request_with_transaction_ids() {
        let (addr, handle) = serve("200 OK", "application/json", JSON_OK, 2).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        client
            .get_property("telescope", 0, "rightascension")
            .await
            .unwrap();
        client
            .get_property("telescope", 0, "declination")
            .await
            .unwrap();

        let requests = handle.await.unwrap();
        assert_eq!(requests.len(), 2);

        let (method, path, params) = request_line_parts(&requests[0]);
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/v1/telescope/0/rightascension");
        assert!(params.contains(&"ClientID=1".to_string()));
        assert!(params.contains(&"ClientTransactionID=1".to_string()));

        let (method, path, params) = request_line_parts(&requests[1]);
        assert_eq!(method, "GET");
        assert_eq!(path, "/api/v1/telescope/0/declination");
        assert!(params.contains(&"ClientID=1".to_string()));
        assert!(params.contains(&"ClientTransactionID=2".to_string()));
    }

    // No direct Python case (the mocks never inspect PUT bodies): pins
    // AlpacaMount::_put — on PUT the transaction parameters travel in the
    // urlencoded form body (not the query) together with the caller's
    // parameters.
    #[tokio::test]
    async fn set_property_forms_request_with_form_body() {
        let (addr, handle) = serve("200 OK", "application/json", r#"{"ErrorNumber": 0}"#, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        client
            .set_property(
                "telescope",
                0,
                "slewtocoordinates",
                &[
                    ("RightAscension", "83.63".to_string()),
                    ("Declination", "22.01".to_string()),
                ],
            )
            .await
            .unwrap();

        let request = handle.await.unwrap().pop().expect("one request");
        let (method, path, params) = request_line_parts(&request);
        assert_eq!(method, "PUT");
        assert_eq!(path, "/api/v1/telescope/0/slewtocoordinates");
        assert!(
            params.is_empty(),
            "PUT must carry its parameters in the body: {request}"
        );

        let (_, body) = request.split_once("\r\n\r\n").expect("request body");
        let form: Vec<&str> = body.split('&').collect();
        assert!(form.contains(&"ClientID=1"));
        assert!(form.contains(&"ClientTransactionID=1"));
        assert!(form.contains(&"RightAscension=83.63"));
        assert!(form.contains(&"Declination=22.01"));

        assert!(
            request
                .to_ascii_lowercase()
                .contains("content-type: application/x-www-form-urlencoded")
        );
    }

    // No direct Python case for the envelope itself: the Python mocks always
    // answer {"Value": ..., "ErrorNumber": 0} and the device classes assert
    // on the parsed value. This pins that Value is returned verbatim, and
    // that a missing ErrorNumber defaults to 0 (the Rust envelope is
    // all-default; Python's dict.get defaults behave the same).
    #[tokio::test]
    async fn get_property_returns_value_from_envelope() {
        let (addr, handle) = serve("200 OK", "application/json", r#"{"Value": 5.575}"#, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let ra = client
            .get_property("telescope", 0, "rightascension")
            .await
            .unwrap();

        assert_eq!(ra, serde_json::json!(5.575));
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // Spelled out by AlpacaMount::_check_error and asserted by every mocked
    // oracle call, but never exercised as a case of its own: a non-zero
    // ErrorNumber becomes an error carrying the code and message. Python
    // raises CommandError; the Rust port maps it to AlpacaError::Device.
    #[tokio::test]
    async fn get_property_maps_nonzero_error_number_to_device_error() {
        let body =
            r#"{"Value": null, "ErrorNumber": 1025, "ErrorMessage": "Property not implemented"}"#;
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        match client.get_property("telescope", 0, "rightascension").await {
            Err(AlpacaError::Device { code, message }) => {
                assert_eq!(code, 1025);
                assert_eq!(message, "Property not implemented");
            }
            other => panic!("expected a Device error, got {other:?}"),
        }
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // The PUT path duplicates the same ErrorNumber mapping; asserted for
    // set_property the same way as for get_property above.
    #[tokio::test]
    async fn set_property_maps_nonzero_error_number_to_device_error() {
        let body = r#"{"ErrorNumber": 1031, "ErrorMessage": "Value out of range"}"#;
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        match client
            .set_property(
                "telescope",
                0,
                "trackingrate",
                &[("TrackingRate", "3".to_string())],
            )
            .await
        {
            Err(AlpacaError::Device { code, message }) => {
                assert_eq!(code, 1031);
                assert_eq!(message, "Value out of range");
            }
            other => panic!("expected a Device error, got {other:?}"),
        }
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // No direct Python case: the mocks are always valid JSON. Python's
    // resp.json() would raise a decode error; the Rust port surfaces the
    // failure as AlpacaError::Http (the reqwest decode error).
    #[tokio::test]
    async fn malformed_json_body_is_an_error() {
        let (addr, handle) = serve(
            "200 OK",
            "application/json",
            r#"{"Value": 5.575, "Error""#,
            1,
        )
        .await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let err = client
            .get_property("telescope", 0, "rightascension")
            .await
            .unwrap_err();

        assert!(matches!(err, AlpacaError::Http(_)), "got {err:?}");
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // No direct Python case: neither client inspects the HTTP status, so a
    // non-200 whose body is not an envelope at all cannot be parsed and the
    // request fails with the transport-level (reqwest) error.
    #[tokio::test]
    async fn non_200_status_with_non_json_body_is_an_error() {
        let (addr, handle) = serve("404 Not Found", "text/plain", "Not Found", 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let err = client
            .get_property("telescope", 0, "rightascension")
            .await
            .unwrap_err();

        assert!(matches!(err, AlpacaError::Http(_)), "got {err:?}");
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // No direct Python case: neither client inspects the HTTP status (the
    // Python oracle's resp.json() is likewise called without checking
    // resp.status). Pinned as-is: only the envelope decides, so a 500 that
    // carries a valid envelope is still a success.
    #[tokio::test]
    async fn non_200_status_is_not_itself_an_error() {
        let (addr, handle) =
            serve("500 Internal Server Error", "application/json", JSON_OK, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let value = client
            .get_property("telescope", 0, "rightascension")
            .await
            .unwrap();

        assert_eq!(value, serde_json::json!(1));
        assert_eq!(handle.await.unwrap().len(), 1);
    }

    // Mirrors TestAlpacaDiscovery::test_discover_devices.
    //
    // Differences from the Python case, pinned as-is: the Rust port
    // returns the devices as a flat Vec<ConfiguredDevice> without Python's
    // "telescope" -> "mount" normalization and grouping (so the type stays
    // "Telescope"), and it requires a UniqueID field the Python mock omits
    // (Python never reads UniqueID). Note serde's PascalCase renaming maps
    // unique_id to the JSON key "UniqueId" with a lowercase "d", not the
    // ASCOM API's "UniqueID" spelling — a reply using the spec spelling
    // fails to parse.
    #[tokio::test]
    async fn get_configured_devices_parses_devices() {
        let body = r#"{"Value": [
            {"DeviceName": "Simulator", "DeviceType": "Telescope", "DeviceNumber": 0, "UniqueId": "9e2c0f5d-1"},
            {"DeviceName": "Camera Sim", "DeviceType": "Camera", "DeviceNumber": 0, "UniqueId": "9e2c0f5d-2"}
        ]}"#;
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, addr.port());

        let devices = client.get_configured_devices().await.unwrap();

        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0].device_name, "Simulator");
        assert_eq!(devices[0].device_type, "Telescope");
        assert_eq!(devices[0].device_number, 0);
        assert_eq!(devices[0].unique_id, "9e2c0f5d-1");
        assert_eq!(devices[1].device_name, "Camera Sim");
        assert_eq!(devices[1].device_type, "Camera");

        let request = handle.await.unwrap().pop().expect("one request");
        let (method, path, params) = request_line_parts(&request);
        assert_eq!((method, path), ("GET", "/management/v1/configureddevices"));
        assert!(
            params.is_empty(),
            "the management API takes no transaction parameters"
        );
    }

    // Mirrors TestAlpacaDiscovery::test_discover_devices_error.
    //
    // Difference pinned as-is: Python's discover_alpaca_devices catches the
    // failure and returns []; the Rust port has no such wrapper, so
    // get_configured_devices propagates the error to its caller (discover()
    // handles it by skipping the server — see the query_server tests in
    // discovery.rs).
    #[tokio::test]
    async fn get_configured_devices_failure_is_propagated() {
        let client = AlpacaClient::new(Ipv4Addr::LOCALHOST, refused_port());

        let err = client.get_configured_devices().await.unwrap_err();

        assert!(matches!(err, AlpacaError::Http(_)), "got {err:?}");
    }
}
