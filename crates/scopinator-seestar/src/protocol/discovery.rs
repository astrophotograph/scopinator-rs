use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;
use tracing::{debug, warn};

use crate::error::SeestarError;
use crate::protocol::json_rpc::DISCOVERY_PORT;

/// Information about a discovered Seestar telescope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiscoveredDevice {
    /// IP address of the telescope.
    pub address: Ipv4Addr,
    /// Product model (e.g., "Seestar S50").
    pub product_model: Option<String>,
    /// Serial number.
    pub serial_number: Option<String>,
    /// Full response data from discovery.
    pub raw_response: serde_json::Value,
}

/// Discover Seestar telescopes on the local network via UDP broadcast.
///
/// Sends a `scan_iscope` probe on the broadcast address and collects responses
/// within the given timeout.
pub async fn discover(timeout: Duration) -> Result<Vec<DiscoveredDevice>, SeestarError> {
    let socket = UdpSocket::bind(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, DISCOVERY_PORT))
        .await
        .map_err(SeestarError::Connection)?;

    socket
        .set_broadcast(true)
        .map_err(SeestarError::Connection)?;

    // Build the discovery probe.
    // Note: "name" must not contain dashes (telescope firmware bug).
    let local_ip = local_ipv4_address().unwrap_or(Ipv4Addr::LOCALHOST);
    let probe = serde_json::json!({
        "id": 201,
        "method": "scan_iscope",
        "name": "scopinator",
        "ip": local_ip.to_string(),
    });
    let probe_bytes = format!("{}\r\n", probe);

    let broadcast_addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::BROADCAST, DISCOVERY_PORT));

    socket
        .send_to(probe_bytes.as_bytes(), broadcast_addr)
        .await
        .map_err(SeestarError::Connection)?;

    debug!("sent discovery probe from {local_ip}");

    let mut devices = Vec::new();
    let mut buf = [0u8; 4096];
    let deadline = tokio::time::Instant::now() + timeout;

    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }

        match tokio::time::timeout(remaining, socket.recv_from(&mut buf)).await {
            Ok(Ok((len, src))) => {
                let data = &buf[..len];
                // Skip our own probe
                if data.starts_with(probe_bytes.as_bytes()) {
                    continue;
                }

                match serde_json::from_slice::<serde_json::Value>(data.trim_ascii()) {
                    Ok(response) => {
                        if let Some(device) = parse_discovery_response(&response, src) {
                            debug!("discovered {device:?}");
                            devices.push(device);
                        }
                    }
                    Err(e) => {
                        warn!("failed to parse discovery response from {src}: {e}");
                    }
                }
            }
            Ok(Err(e)) => {
                warn!("discovery recv error: {e}");
                break;
            }
            Err(_) => break, // timeout
        }
    }

    Ok(devices)
}

fn parse_discovery_response(
    response: &serde_json::Value,
    src: SocketAddr,
) -> Option<DiscoveredDevice> {
    let result = response.get("result")?;

    let ip_str = result
        .get("ip")
        .and_then(|v| v.as_str())
        .unwrap_or_default();

    let address = ip_str.parse::<Ipv4Addr>().unwrap_or_else(|_| match src {
        SocketAddr::V4(v4) => *v4.ip(),
        SocketAddr::V6(_) => Ipv4Addr::LOCALHOST,
    });

    Some(DiscoveredDevice {
        address,
        product_model: result
            .get("product_model")
            .and_then(|v| v.as_str())
            .map(String::from),
        serial_number: result.get("sn").and_then(|v| v.as_str()).map(String::from),
        raw_response: response.clone(),
    })
}

/// Best-effort attempt to find a local IPv4 address.
fn local_ipv4_address() -> Option<Ipv4Addr> {
    // Bind a UDP socket to a public address (doesn't actually send anything)
    // and check which local address the OS picks.
    let socket = std::net::UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    match socket.local_addr().ok()? {
        SocketAddr::V4(v4) => Some(*v4.ip()),
        SocketAddr::V6(_) => None,
    }
}
