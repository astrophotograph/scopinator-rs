use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use serde::Deserialize;
use tokio::net::UdpSocket;
use tracing::{debug, warn};

use crate::client::{AlpacaClient, ConfiguredDevice};
use crate::error::AlpacaError;

/// Alpaca discovery broadcast port.
const DISCOVERY_PORT: u16 = 32227;

/// Discovery probe string.
const DISCOVERY_PROBE: &[u8] = b"alpacadiscovery1";

/// A discovered Alpaca server.
#[derive(Debug, Clone)]
pub struct DiscoveredServer {
    pub address: Ipv4Addr,
    pub port: u16,
    pub devices: Vec<ConfiguredDevice>,
}

/// Response from Alpaca discovery broadcast.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct DiscoveryResponse {
    alpaca_port: u16,
}

/// Parse a discovery reply (`{"AlpacaPort": <port>}`) into its port.
///
/// Extracted from `discover()` (identical behaviour) so the reply format can
/// be unit-tested without a UDP socket.
fn parse_reply(data: &[u8]) -> Result<DiscoveryResponse, serde_json::Error> {
    serde_json::from_slice(data)
}

/// Query the management API of a server that answered a discovery probe and
/// build the resulting `DiscoveredServer`.
///
/// Returns `None` if the management API cannot be queried; the server is then
/// skipped. Extracted from `discover()` (identical behaviour) so the reply
/// handling can be tested without a UDP socket: only the management query
/// needs a socket, and that runs over TCP.
async fn query_server(ip: Ipv4Addr, port: u16) -> Option<DiscoveredServer> {
    debug!(ip = %ip, port, "found Alpaca server");

    let client = AlpacaClient::new(ip, port);
    match client.get_configured_devices().await {
        Ok(devices) => Some(DiscoveredServer {
            address: ip,
            port,
            devices,
        }),
        Err(e) => {
            warn!(ip = %ip, error = %e, "failed to query Alpaca devices");
            None
        }
    }
}

/// Discover Alpaca servers on the local network.
///
/// Sends a UDP broadcast and then queries each responding server for its
/// configured devices.
pub async fn discover(timeout: Duration) -> Result<Vec<DiscoveredServer>, AlpacaError> {
    let socket = UdpSocket::bind("0.0.0.0:0")
        .await
        .map_err(AlpacaError::Connection)?;

    socket
        .set_broadcast(true)
        .map_err(AlpacaError::Connection)?;

    let broadcast_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT));

    // Send discovery probes
    for _ in 0..2 {
        socket
            .send_to(DISCOVERY_PROBE, broadcast_addr)
            .await
            .map_err(AlpacaError::Connection)?;
    }

    debug!("sent Alpaca discovery probes");

    let mut servers = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut buf = [0u8; 1024];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, src))) => {
                if let Ok(resp) = parse_reply(&buf[..len]) {
                    let ip = match src {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => continue,
                    };
                    if seen.insert(ip)
                        && let Some(server) = query_server(ip, resp.alpaca_port).await
                    {
                        servers.push(server);
                    }
                }
            }
            Ok(Err(e)) => {
                warn!(error = %e, "discovery recv error");
                break;
            }
            Err(_) => break,
        }
    }

    Ok(servers)
}

#[cfg(test)]
mod tests {
    // Mirrors of the pyscopinator Alpaca oracle's discovery cases
    // (pyscopinator/tests/v2/backends/test_alpaca.py). The Python oracle
    // only tests discovery through the mocked HTTP session; the UDP wire is
    // never exercised there. UDP cannot be bound in this sandbox (a
    // UdpSocket bind fails with "Operation not permitted"), so discover()
    // itself is not called here: the probe bytes and the reply handling are
    // pinned without a socket, and the management query (which runs over
    // loopback TCP) uses the client tests' HTTP test double.
    //
    // TestAlpacaDiscovery::test_discover_devices is ported here as
    // query_server_builds_discovered_server (and at the client layer in
    // client::tests::get_configured_devices_parses_devices);
    // test_discover_devices_error as query_server_skips_unreachable_servers
    // (and client::tests::get_configured_devices_failure_is_propagated).
    // The 12 other oracle cases are accounted for in client::tests.

    use super::*;
    use crate::client::tests::{refused_port, request_line_parts, serve};

    // No direct Python case: Python's discover_alpaca_servers sends
    // b"alpacadiscovery1" to UDP port 32227 (see discovery.py); the Rust
    // probe is pinned on its own because the socket cannot be exercised.
    #[test]
    fn discovery_probe_is_alpacadiscovery1() {
        assert_eq!(DISCOVERY_PROBE, b"alpacadiscovery1");
        assert_eq!(DISCOVERY_PORT, 32227);
    }

    // No direct Python case: pins the reply format
    // {"AlpacaPort": <port>} that discover() parses.
    #[test]
    fn parse_reply_extracts_alpaca_port() {
        let resp = parse_reply(br#"{"AlpacaPort": 11111}"#).expect("valid reply");
        assert_eq!(resp.alpaca_port, 11111);
    }

    // No direct Python case: pins what discover() does with replies it
    // cannot use — they are dropped silently. Python's
    // discover_alpaca_servers differs: it keeps non-JSON replies as raw text
    // keyed by host; the Rust port drops them.
    #[test]
    fn parse_reply_rejects_unusable_replies() {
        assert!(parse_reply(b"not json").is_err());
        assert!(parse_reply(br#"{"SomethingElse": 11111}"#).is_err());
    }

    // Mirrors TestAlpacaDiscovery::test_discover_devices (discovery layer):
    // the AlpacaPort from a reply plus the management API yield a
    // DiscoveredServer carrying the configured devices. Differences pinned
    // as-is: DeviceType stays "Telescope" (no Python "telescope" -> "mount"
    // normalization); the device key follows the Alpaca API spelling,
    // "UniqueID".
    #[tokio::test]
    async fn query_server_builds_discovered_server() {
        let body = r#"{"Value": [
            {"DeviceName": "Simulator", "DeviceType": "Telescope", "DeviceNumber": 0, "UniqueID": "u1"},
            {"DeviceName": "Camera Sim", "DeviceType": "Camera", "DeviceNumber": 0, "UniqueID": "u2"}
        ]}"#;
        let (addr, handle) = serve("200 OK", "application/json", body, 1).await;

        let server = query_server(Ipv4Addr::LOCALHOST, addr.port())
            .await
            .expect("discovered server");

        assert_eq!(server.address, Ipv4Addr::LOCALHOST);
        assert_eq!(server.port, addr.port());
        assert_eq!(server.devices.len(), 2);
        assert_eq!(server.devices[0].device_name, "Simulator");
        assert_eq!(server.devices[0].device_type, "Telescope");
        assert_eq!(server.devices[0].device_number, 0);
        assert_eq!(server.devices[1].device_type, "Camera");

        let request = handle.await.unwrap().pop().expect("one request");
        let (method, path, _) = request_line_parts(&request);
        assert_eq!((method, path), ("GET", "/management/v1/configureddevices"));
    }

    // Mirrors TestAlpacaDiscovery::test_discover_devices_error (discovery
    // layer): a server whose management API cannot be queried is handled
    // gracefully — where Python's discover_alpaca_devices swallows the
    // failure and returns [] for the whole call, the Rust discover() skips
    // just that server and keeps listening.
    #[tokio::test]
    async fn query_server_skips_unreachable_servers() {
        assert!(
            query_server(Ipv4Addr::LOCALHOST, refused_port())
                .await
                .is_none()
        );
    }
}
