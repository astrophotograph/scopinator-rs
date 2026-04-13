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
                let data = &buf[..len];
                if let Ok(resp) = serde_json::from_slice::<DiscoveryResponse>(data) {
                    let ip = match src {
                        SocketAddr::V4(v4) => *v4.ip(),
                        _ => continue,
                    };
                    if seen.insert(ip) {
                        debug!(ip = %ip, port = resp.alpaca_port, "found Alpaca server");

                        // Query management API for devices
                        let client = AlpacaClient::new(ip, resp.alpaca_port);
                        match client.get_configured_devices().await {
                            Ok(devices) => {
                                servers.push(DiscoveredServer {
                                    address: ip,
                                    port: resp.alpaca_port,
                                    devices,
                                });
                            }
                            Err(e) => {
                                warn!(ip = %ip, error = %e, "failed to query Alpaca devices");
                            }
                        }
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
