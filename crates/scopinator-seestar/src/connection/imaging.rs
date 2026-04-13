use std::net::SocketAddr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::broadcast;
use tracing::{debug, error, info, trace, warn};

use crate::connection::reconnect::ReconnectPolicy;
use crate::error::SeestarError;
use crate::protocol::frame::{self, FrameHeader, HEADER_SIZE, MAX_PAYLOAD_SIZE};
use scopinator_types::FrameKind;

/// Connection timeout for TCP connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Heartbeat interval on the imaging port.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// An image frame received from the imaging port.
#[derive(Debug, Clone)]
pub struct ImageFrame {
    pub header: FrameHeader,
    pub data: Bytes,
    pub kind: FrameKind,
}

/// Run the imaging connection loop (reconnecting on failure).
pub(crate) async fn run(
    addr: SocketAddr,
    frame_tx: broadcast::Sender<Arc<ImageFrame>>,
    connected: Arc<AtomicBool>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut policy = ReconnectPolicy::new();

    loop {
        if *shutdown.borrow() {
            info!("imaging connection shutting down");
            break;
        }

        info!("connecting to telescope imaging at {addr}");

        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(stream)) => {
                if let Err(e) = stream.set_nodelay(true) {
                    warn!("failed to set TCP_NODELAY on imaging: {e}");
                }
                info!("connected to telescope imaging at {addr}");
                connected.store(true, Ordering::Release);
                policy.reset();

                run_imaging_connected(stream, &frame_tx, shutdown.clone()).await;

                connected.store(false, Ordering::Release);
                info!("imaging connection lost, will reconnect");
            }
            Ok(Err(e)) => {
                warn!("failed to connect imaging to {addr}: {e}");
            }
            Err(_) => {
                warn!("imaging connection to {addr} timed out");
            }
        }

        let wait = policy.next_backoff();
        tokio::time::sleep(wait).await;
    }
}

/// Run while connected: reads frames and sends heartbeats.
async fn run_imaging_connected(
    stream: TcpStream,
    frame_tx: &broadcast::Sender<Arc<ImageFrame>>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (mut read_half, mut write_half) = tokio::io::split(stream);

    // Heartbeat task
    let hb_handle = tokio::spawn({
        let mut shutdown = shutdown.clone();
        async move {
            let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
            interval.tick().await; // skip first

            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        let msg = b"{\"id\":99,\"method\":\"test_connection\",\"params\":\"verify\"}\r\n";
                        if let Err(e) = write_half.write_all(msg).await {
                            debug!("imaging heartbeat write error: {e}");
                            break;
                        }
                    }
                    _ = shutdown.changed() => break,
                }
            }
        }
    });

    // Reader loop
    let mut header_buf = [0u8; HEADER_SIZE];

    loop {
        tokio::select! {
            result = read_frame(&mut read_half, &mut header_buf) => {
                match result {
                    Ok(Some(frame)) => {
                        trace!(
                            id = frame.header.id,
                            width = frame.header.width,
                            height = frame.header.height,
                            size = frame.header.size,
                            "received imaging frame"
                        );
                        let _ = frame_tx.send(Arc::new(frame));
                    }
                    Ok(None) => {
                        // EOF
                        info!("imaging connection EOF");
                        break;
                    }
                    Err(e) => {
                        error!("imaging read error: {e}");
                        break;
                    }
                }
            }
            _ = shutdown.changed() => break,
        }
    }

    hb_handle.abort();
}

/// Read a single frame (header + payload) from the imaging stream.
async fn read_frame(
    reader: &mut tokio::io::ReadHalf<TcpStream>,
    header_buf: &mut [u8; HEADER_SIZE],
) -> Result<Option<ImageFrame>, SeestarError> {
    // Read header
    match reader.read_exact(header_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SeestarError::Connection(e)),
    }

    let header = FrameHeader::parse(header_buf);

    if header.size > MAX_PAYLOAD_SIZE {
        return Err(SeestarError::FrameTooLarge {
            size: header.size,
            limit: MAX_PAYLOAD_SIZE,
        });
    }

    // Read payload
    let mut payload = vec![0u8; header.size as usize];
    match reader.read_exact(&mut payload).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(SeestarError::Connection(e)),
    }

    let kind = match header.id {
        frame::frame_id::STACK => FrameKind::Stack,
        _ => FrameKind::Preview,
    };

    Ok(Some(ImageFrame {
        header,
        data: Bytes::from(payload),
        kind,
    }))
}
