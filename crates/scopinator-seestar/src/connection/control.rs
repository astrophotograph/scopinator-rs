use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tracing::{debug, error, info, trace, warn};

use crate::auth::InteropKey;
use crate::command::Command;
use crate::command::serialize::serialize_command;
use crate::connection::reconnect::ReconnectPolicy;
use crate::connection::registry::Registry;
use crate::error::SeestarError;
use crate::event::SeestarEvent;
use crate::protocol::json_rpc::{self, INITIAL_COMMAND_ID, MAX_LINE_BYTES};
use crate::response::CommandResponse;
use scopinator_types::FirmwareVersion;

/// Connection timeout for TCP connect.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Heartbeat interval.
const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

/// A request from user code to the writer task.
pub(crate) struct ClientRequest {
    pub command: Command,
    pub response_tx: oneshot::Sender<Result<CommandResponse, SeestarError>>,
}

type ResponseSender = oneshot::Sender<Result<CommandResponse, SeestarError>>;

/// Shared state for the control connection.
pub(crate) struct ControlState {
    /// Pending requests keyed by allocated ID. Owns its own internal lock
    /// and ID allocator so callers don't need an outer Mutex.
    pub(crate) pending: Registry<ResponseSender>,
    /// Detected firmware version (sync mutex — never held across await).
    firmware_version: std::sync::Mutex<Option<FirmwareVersion>>,
}

impl ControlState {
    fn new() -> Self {
        Self {
            pending: Registry::with_start(INITIAL_COMMAND_ID),
            firmware_version: std::sync::Mutex::new(None),
        }
    }

    fn firmware_version(&self) -> Option<FirmwareVersion> {
        *self
            .firmware_version
            .lock()
            .expect("firmware_version mutex poisoned")
    }

    fn set_firmware_version(&self, fw: FirmwareVersion) {
        *self
            .firmware_version
            .lock()
            .expect("firmware_version mutex poisoned") = Some(fw);
    }
}

/// Run the control connection loop (reconnecting on failure).
///
/// This is the top-level task for the control port. It connects to the
/// telescope, spawns reader/writer/heartbeat tasks, and reconnects on failure.
pub(crate) async fn run(
    addr: SocketAddr,
    request_rx: mpsc::Receiver<ClientRequest>,
    event_tx: broadcast::Sender<SeestarEvent>,
    connected: Arc<AtomicBool>,
    shutdown: tokio::sync::watch::Receiver<bool>,
    interop_key: Option<Arc<InteropKey>>,
) {
    let state = Arc::new(ControlState::new());
    let mut policy = ReconnectPolicy::new();

    // Wrap request_rx in Arc<Mutex> so we can reuse it across reconnections
    let request_rx = Arc::new(Mutex::new(request_rx));

    loop {
        if *shutdown.borrow() {
            info!("control connection shutting down");
            break;
        }

        info!("connecting to telescope control at {addr}");

        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect(addr)).await {
            Ok(Ok(mut stream)) => {
                if let Err(e) = stream.set_nodelay(true) {
                    warn!("failed to set TCP_NODELAY: {e}");
                }
                info!("connected to telescope control at {addr}");

                // Authenticate before advertising the connection as ready.
                if let Some(key) = &interop_key {
                    if let Err(e) = crate::auth::authenticate(&mut stream, key).await {
                        error!("authentication failed: {e}");
                        // Fall through to the backoff delay below rather than
                        // continuing immediately — avoids hammering the scope.
                    } else {
                        connected.store(true, Ordering::Release);
                        policy.reset();
                        run_connected(
                            stream,
                            Arc::clone(&state),
                            Arc::clone(&request_rx),
                            event_tx.clone(),
                            shutdown.clone(),
                        )
                        .await;
                        connected.store(false, Ordering::Release);
                        info!("control connection lost, will reconnect");
                    }
                    // Both paths fall through to flush_pending + backoff.
                    flush_pending(&state);
                    let wait = policy.next_backoff();
                    tokio::time::sleep(wait).await;
                    continue;
                }

                connected.store(true, Ordering::Release);
                policy.reset();

                run_connected(
                    stream,
                    Arc::clone(&state),
                    Arc::clone(&request_rx),
                    event_tx.clone(),
                    shutdown.clone(),
                )
                .await;

                connected.store(false, Ordering::Release);
                info!("control connection lost, will reconnect");
            }
            Ok(Err(e)) => {
                warn!("failed to connect to {addr}: {e}");
            }
            Err(_) => {
                warn!("connection to {addr} timed out");
            }
        }

        // Flush all pending requests with disconnect error
        flush_pending(&state);

        let wait = policy.next_backoff();
        tokio::time::sleep(wait).await;
    }
}

/// Run while connected: spawns reader, writer, and heartbeat tasks.
async fn run_connected(
    stream: TcpStream,
    state: Arc<ControlState>,
    request_rx: Arc<Mutex<mpsc::Receiver<ClientRequest>>>,
    event_tx: broadcast::Sender<SeestarEvent>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let (read_half, write_half) = tokio::io::split(stream);

    // Channel for the writer task to send serialized messages
    let (write_tx, mut write_rx) = mpsc::channel::<String>(256);

    // Reader died signal
    let (reader_dead_tx, reader_dead_rx) = oneshot::channel::<()>();

    // Writer task
    let writer_handle = {
        let state = Arc::clone(&state);
        let write_tx_for_hb = write_tx.clone();

        tokio::spawn(async move {
            writer_task(
                write_half,
                &mut write_rx,
                request_rx,
                state,
                reader_dead_rx,
                write_tx_for_hb,
                shutdown,
            )
            .await;
        })
    };

    // Reader task
    let reader_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            reader_task(read_half, state, event_tx).await;
            let _ = reader_dead_tx.send(());
        })
    };

    // Wait for either task to finish
    tokio::select! {
        _ = writer_handle => {},
        _ = reader_handle => {},
    }
}

/// Reader task: reads lines from TCP, routes responses and events.
async fn reader_task(
    read_half: tokio::io::ReadHalf<TcpStream>,
    state: Arc<ControlState>,
    event_tx: broadcast::Sender<SeestarEvent>,
) {
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    loop {
        line.clear();
        match reader.read_line(&mut line).await {
            Ok(0) => {
                info!("telescope control connection EOF");
                return;
            }
            Ok(n) if n > MAX_LINE_BYTES => {
                error!("line too long ({n} bytes), disconnecting");
                return;
            }
            Ok(_) => {
                let trimmed = line.trim();
                if trimmed.is_empty() {
                    continue;
                }

                let msg: serde_json::Value = match serde_json::from_str(trimmed) {
                    Ok(v) => v,
                    Err(e) => {
                        warn!("failed to parse JSON from telescope: {e}");
                        continue;
                    }
                };

                if json_rpc::is_event(&msg) {
                    handle_event(&msg, &event_tx);
                } else if json_rpc::is_response(&msg) {
                    handle_response(&msg, &state);
                } else {
                    trace!("unclassified message: {trimmed}");
                }
            }
            Err(e) => {
                error!("read error on control connection: {e}");
                return;
            }
        }
    }
}

/// Handle an incoming event message.
fn handle_event(msg: &serde_json::Value, event_tx: &broadcast::Sender<SeestarEvent>) {
    match serde_json::from_value::<SeestarEvent>(msg.clone()) {
        Ok(event) => {
            trace!(event = event.name(), "received event");
            let _ = event_tx.send(event);
        }
        Err(e) => {
            warn!("failed to deserialize event: {e}");
        }
    }
}

/// Handle an incoming response message.
fn handle_response(msg: &serde_json::Value, state: &Arc<ControlState>) {
    let id = match json_rpc::json_rpc_id(msg) {
        Some(id) => id,
        None => {
            trace!("response without valid id");
            return;
        }
    };

    // Check for firmware version in device state responses
    if let Some(method) = json_rpc::method_name(msg)
        && method == "get_device_state"
        && let Some(result) = msg.get("result")
        && let Some(fw_int) = result
            .get("device")
            .and_then(|d| d.get("firmware_ver_int"))
            .and_then(|v| v.as_u64())
    {
        state.set_firmware_version(FirmwareVersion(fw_int as u32));
        debug!(firmware = fw_int, "detected firmware version");
    }

    let response: CommandResponse = match serde_json::from_value(msg.clone()) {
        Ok(r) => r,
        Err(e) => {
            warn!("failed to deserialize response: {e}");
            return;
        }
    };

    if let Some(sender) = state.pending.take(id) {
        let _ = sender.send(Ok(response));
    } else {
        trace!(id, "response for unknown/expired request");
    }
}

/// Writer task: processes client requests and writes to TCP.
async fn writer_task(
    mut write_half: tokio::io::WriteHalf<TcpStream>,
    write_rx: &mut mpsc::Receiver<String>,
    request_rx: Arc<Mutex<mpsc::Receiver<ClientRequest>>>,
    state: Arc<ControlState>,
    mut reader_dead_rx: oneshot::Receiver<()>,
    heartbeat_tx: mpsc::Sender<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Spawn heartbeat
    let hb_handle = tokio::spawn(heartbeat_task(heartbeat_tx, shutdown.clone()));

    loop {
        let mut req_rx = request_rx.lock().await;
        tokio::select! {
            // Client request
            Some(req) = req_rx.recv() => {
                drop(req_rx); // release lock before doing work
                let fw = state.firmware_version();
                let method = req.command.method();
                let id = state.pending.register(req.response_tx);
                let msg = serialize_command(&req.command, id, fw);
                let line = format!("{}\r\n", msg);

                trace!(id, method, "sending command");

                if let Err(e) = write_half.write_all(line.as_bytes()).await {
                    error!("write error on control connection: {e}");
                    if let Some(sender) = state.pending.take(id) {
                        let _ = sender.send(Err(SeestarError::Disconnected));
                    }
                    break;
                }
            }
            // Heartbeat or other internal write
            Some(line) = write_rx.recv() => {
                drop(req_rx);
                if let Err(e) = write_half.write_all(line.as_bytes()).await {
                    error!("write error on control connection: {e}");
                    break;
                }
            }
            // Reader died
            _ = &mut reader_dead_rx => {
                drop(req_rx);
                debug!("reader task died, exiting writer");
                break;
            }
            // Shutdown
            _ = shutdown.changed() => {
                drop(req_rx);
                break;
            }
        }
    }

    hb_handle.abort();
}

/// Heartbeat task: sends `pi_get_time` every 5 seconds.
async fn heartbeat_task(
    tx: mpsc::Sender<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    // Skip the first immediate tick
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // Use a fixed low ID for heartbeats that we don't track
                let msg = serde_json::json!({
                    "id": 1,
                    "method": "pi_get_time",
                    "params": ["verify"],
                });
                let line = format!("{}\r\n", msg);
                if tx.send(line).await.is_err() {
                    break;
                }
            }
            _ = shutdown.changed() => break,
        }
    }
}

/// Flush all pending requests with a disconnect error.
fn flush_pending(state: &Arc<ControlState>) {
    let pending = state.pending.drain();
    let count = pending.len();
    for sender in pending {
        let _ = sender.send(Err(SeestarError::Disconnected));
    }
    if count > 0 {
        debug!(count, "flushed pending requests on disconnect");
    }
}
