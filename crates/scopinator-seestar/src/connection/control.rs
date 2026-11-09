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
            // Surface unmodeled event types — a signal of firmware drift, since
            // the Unknown variant does not retain the payload.
            if matches!(event, SeestarEvent::Unknown) {
                warn!(
                    event = json_rpc::event_name(msg).unwrap_or("?"),
                    "received unmodeled event type (possible firmware drift)"
                );
            } else {
                trace!(event = event.name(), "received event");
            }
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

#[cfg(test)]
mod heartbeat_tests {
    use super::*;

    /// Run the real heartbeat task under paused time and return the first line it sends.
    async fn first_heartbeat_line() -> String {
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(heartbeat_task(tx, shutdown_rx));

        // Nothing is sent before the first interval elapses.
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());

        tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
        let line = rx.recv().await.expect("heartbeat line");

        shutdown_tx.send(true).unwrap();
        handle.await.unwrap();
        line
    }

    // Characterizes docs/bugs/heartbeat-hardcoded-verify.md (Forge task
    // "Pin heartbeat verify behavior and write bug report"). pyscopinator's
    // `_should_inject_verify` only injects "verify" on firmware > 2582; the
    // Rust heartbeat bypasses `serialize_command` and always injects it.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_always_sends_hardcoded_verify() {
        let line = first_heartbeat_line().await;
        assert_eq!(
            line,
            "{\"id\":1,\"method\":\"pi_get_time\",\"params\":[\"verify\"]}\r\n"
        );
    }

    // The heartbeat task takes no firmware input, so its bytes are identical
    // for every firmware. Compare with `serialize_command`: equal on
    // post-2582 firmware, DIVERGENT on firmware <= 2582 (no params expected).
    #[tokio::test(start_paused = true)]
    async fn heartbeat_ignores_firmware_unlike_serialize_command() {
        let line = first_heartbeat_line().await;
        let sent: serde_json::Value = serde_json::from_str(line.trim()).unwrap();

        let post = serialize_command(&Command::PiGetTime, 1, Some(FirmwareVersion(2700)));
        assert_eq!(sent, post, "post-2582 firmware: heartbeat matches");

        for fw in [2000, FirmwareVersion::VERIFY_THRESHOLD] {
            let pre = serialize_command(&Command::PiGetTime, 1, Some(FirmwareVersion(fw)));
            assert!(
                pre.get("params").is_none(),
                "fw {fw}: serialize omits verify"
            );
            // Pinned current behaviour: heartbeat still carries params.
            assert_ne!(
                sent, pre,
                "fw {fw}: heartbeat diverges from serialize_command"
            );
            assert_eq!(sent["params"], serde_json::json!(["verify"]));
        }
    }
}

/// Timeout interplay under paused tokio time (Forge task "Add timeout
/// interplay tests with paused time"). No Python original: pyscopinator has no
/// spanning test for these constants either.
///
/// These drive the real `run` loop against a real loopback socket. Tokio
/// auto-advances a paused clock whenever the runtime is idle, which includes
/// "idle while waiting for a loopback packet" — that makes virtual timestamps
/// meaningless. So the tests never go idle: they advance virtual time in small
/// explicit steps (`step`) and poll everything else non-blockingly.
#[cfg(test)]
mod timeout_interplay_tests {
    use super::*;
    use tokio::net::TcpListener;
    use tokio::time::Instant;

    /// Mirrors the private `AUTH_TIMEOUT` in `auth.rs` (10 s per handshake read).
    const AUTH_TIMEOUT: Duration = Duration::from_secs(10);
    const STEP: Duration = Duration::from_millis(10);
    const TOL: Duration = Duration::from_millis(60);

    /// Advance virtual time one step and let real loopback IO and tasks run.
    async fn step() {
        tokio::time::advance(STEP).await;
        std::thread::sleep(Duration::from_micros(300));
        tokio::task::yield_now().await;
        tokio::task::yield_now().await;
    }

    async fn run_for(d: Duration) {
        let end = Instant::now() + d;
        while Instant::now() < end {
            step().await;
        }
    }

    /// Server-side view of one accepted control connection.
    struct Conn {
        accepted_at: Instant,
        lines: mpsc::UnboundedReceiver<String>,
        /// Dropping/sending closes the server end of the socket.
        close: Option<oneshot::Sender<()>>,
    }

    impl Conn {
        fn close(&mut self) {
            if let Some(tx) = self.close.take() {
                let _ = tx.send(());
            }
        }
    }

    struct Harness {
        conns: mpsc::UnboundedReceiver<Conn>,
        connected: Arc<AtomicBool>,
        shutdown_tx: tokio::sync::watch::Sender<bool>,
        _request_tx: mpsc::Sender<ClientRequest>,
        handle: tokio::task::JoinHandle<()>,
    }

    async fn start(key: Option<Arc<InteropKey>>) -> Harness {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let (conn_tx, conns) = mpsc::unbounded_channel::<Conn>();
        tokio::spawn(async move {
            while let Ok((stream, _)) = listener.accept().await {
                let accepted_at = Instant::now();
                let (line_tx, lines) = mpsc::unbounded_channel();
                let (close_tx, mut close_rx) = oneshot::channel::<()>();
                tokio::spawn(async move {
                    let mut reader = BufReader::new(stream);
                    loop {
                        let mut s = String::new();
                        tokio::select! {
                            n = reader.read_line(&mut s) => match n {
                                Ok(0) | Err(_) => break,
                                Ok(_) => { let _ = line_tx.send(s); }
                            },
                            _ = &mut close_rx => break,
                        }
                    }
                    // `reader` (and the socket) drops here.
                });
                let _ = conn_tx.send(Conn {
                    accepted_at,
                    lines,
                    close: Some(close_tx),
                });
            }
        });

        let (request_tx, request_rx) = mpsc::channel(8);
        let (event_tx, _) = broadcast::channel(8);
        let connected = Arc::new(AtomicBool::new(false));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run(
            addr,
            request_rx,
            event_tx,
            Arc::clone(&connected),
            shutdown_rx,
            key,
        ));
        Harness {
            conns,
            connected,
            shutdown_tx,
            _request_tx: request_tx,
            handle,
        }
    }

    impl Harness {
        /// Step virtual time until the next connection is accepted (bounded).
        async fn next_conn(&mut self, within: Duration) -> Conn {
            let end = Instant::now() + within;
            loop {
                if let Ok(c) = self.conns.try_recv() {
                    return c;
                }
                assert!(Instant::now() < end, "no connection within {within:?}");
                step().await;
            }
        }

        /// Signal shutdown and step (bounded) until `run` exits.
        async fn stop(self) {
            self.shutdown_tx.send(true).unwrap();
            let end = Instant::now() + Duration::from_secs(60);
            while !self.handle.is_finished() {
                assert!(Instant::now() < end, "run did not exit after shutdown");
                step().await;
            }
        }
    }

    // Forge task: "Heartbeat behavior across a reconnect window".
    // Pinned: after the socket dies the control loop reconnects after the 1 s
    // policy backoff, and the new connection gets a fresh heartbeat task whose
    // first beat comes a full HEARTBEAT_INTERVAL after the reconnect — not
    // sooner, and not carried over from the dead connection's timer.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_restarts_with_fresh_interval_after_reconnect() {
        let mut h = start(None).await;

        let mut c1 = h.next_conn(Duration::from_secs(1)).await;
        run_for(HEARTBEAT_INTERVAL + Duration::from_millis(100)).await;
        let hb = c1.lines.try_recv().expect("first heartbeat");
        assert!(hb.contains("pi_get_time"), "got {hb:?}");
        run_for(HEARTBEAT_INTERVAL).await;
        assert!(c1.lines.try_recv().is_ok(), "second heartbeat");

        // Kill the connection from the telescope side.
        c1.close();
        let dead_at = Instant::now();

        let mut c2 = h.next_conn(Duration::from_secs(30)).await;
        let backoff = c2.accepted_at - dead_at;
        assert!(
            backoff >= Duration::from_secs(1) && backoff <= Duration::from_secs(1) + TOL,
            "reconnected after {backoff:?}"
        );
        assert!(h.connected.load(Ordering::Acquire));

        // No heartbeat until (almost) a full interval on the new connection...
        run_for(HEARTBEAT_INTERVAL - Duration::from_millis(200)).await;
        assert!(c2.lines.try_recv().is_err(), "heartbeat arrived early");
        // ...then exactly one.
        run_for(Duration::from_millis(400)).await;
        let hb2 = c2.lines.try_recv().expect("heartbeat on new connection");
        assert!(hb2.contains("pi_get_time"));
        assert!(c2.lines.try_recv().is_err());

        c2.close();
        h.stop().await;
    }

    // Forge task: "Auth timeout mid-handshake -> connection retries per
    // ReconnectPolicy backoff". The telescope accepts and reads
    // `get_verify_str` but never answers. Pinned: each attempt fails after
    // AUTH_TIMEOUT, then waits the policy backoff; because the policy is only
    // reset on a *successful* auth, backoff escalates 1 s, 2 s, 4 s. The
    // control connection is never advertised as connected.
    #[tokio::test(start_paused = true)]
    async fn auth_timeout_retries_with_escalating_backoff() {
        use rsa::pkcs8::EncodePrivateKey;
        let private = rsa::RsaPrivateKey::new(&mut rand::rngs::OsRng, 512).unwrap();
        let pem = private.to_pkcs8_pem(rsa::pkcs8::LineEnding::LF).unwrap();
        let key = Arc::new(InteropKey::from_pem(pem.as_str()).unwrap());

        let mut h = start(Some(key)).await;

        let mut conns = Vec::new();
        for _ in 0..4 {
            let mut c = h.next_conn(Duration::from_secs(30)).await;
            step().await;
            let line = c.lines.try_recv().expect("handshake request");
            assert!(line.contains("get_verify_str"), "got {line:?}");
            assert!(!h.connected.load(Ordering::Acquire));
            conns.push(c); // keep open: silence, not EOF
        }

        let gaps: Vec<Duration> = conns
            .windows(2)
            .map(|w| w[1].accepted_at - w[0].accepted_at)
            .collect();
        for (gap, backoff) in gaps.iter().zip([1u64, 2, 4]) {
            let want = AUTH_TIMEOUT + Duration::from_secs(backoff);
            assert!(
                *gap >= want && *gap <= want + TOL,
                "gap {gap:?}, expected ~{want:?}"
            );
        }

        for c in &mut conns {
            c.close();
        }
        h.stop().await;
    }
}
