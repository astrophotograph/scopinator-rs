use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::TcpStream;
use tokio::sync::{Mutex, broadcast, mpsc, oneshot};
use tracing::{debug, error, info, trace, warn};

use crate::auth::{InteropKey, Verification};
use crate::command::Command;
use crate::command::serialize::WireProfile;
use crate::connection::reconnect::ReconnectPolicy;
use crate::connection::registry::Registry;
use crate::error::SeestarError;
use crate::event::SeestarEvent;
use crate::protocol::json_rpc::{self, INITIAL_COMMAND_ID, MAX_LINE_BYTES};
use crate::response::{CommandResponse, DeviceSummary};
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

/// What the control loop currently knows about the link, shared with the client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(crate) enum LinkState {
    /// Not connected, or negotiation has not finished.
    #[default]
    Unknown,
    /// Connected; requests are framed for this verification mode.
    Negotiated(Verification),
    /// The last connection attempt found that the device needs an interop
    /// key and none is configured. Commands cannot succeed.
    KeyRequired,
}

/// What is known about the device behind the link.
#[derive(Debug, Clone, Default)]
struct LinkInfo {
    state: LinkState,
    /// Identity, optics and sensor, from `get_device_state` replies.
    device: DeviceSummary,
}

/// Link state and device identity, shared between the control loop and the
/// client (sync mutex — never held across await).
#[derive(Debug, Default)]
pub(crate) struct Link(std::sync::Mutex<LinkInfo>);

impl Link {
    fn lock(&self) -> std::sync::MutexGuard<'_, LinkInfo> {
        self.0.lock().expect("link mutex poisoned")
    }

    pub(crate) fn get(&self) -> LinkState {
        self.lock().state
    }

    fn set(&self, state: LinkState) {
        self.lock().state = state;
    }

    /// The device's firmware version, once a `get_device_state` reply has
    /// been seen. Kept across reconnects.
    pub(crate) fn firmware(&self) -> Option<FirmwareVersion> {
        self.lock().device.firmware
    }

    /// Everything learned about the device so far.
    pub(crate) fn device(&self) -> DeviceSummary {
        self.lock().device.clone()
    }

    /// The device's product model (e.g. `Seestar S50`), once known.
    pub(crate) fn model(&self) -> Option<String> {
        self.lock().device.model.clone()
    }
}

/// JSON-RPC id of the `get_device_state` the control loop sends by itself
/// right after negotiation, to learn the firmware and model. Its reply is not
/// awaited: the reader picks the fields out of any `get_device_state` reply.
const ID_CONNECT_DEVICE_STATE: u64 = 1004;

/// Shared state for the control connection.
pub(crate) struct ControlState {
    /// Pending requests keyed by allocated ID. Owns its own internal lock
    /// and ID allocator so callers don't need an outer Mutex.
    pub(crate) pending: Registry<ResponseSender>,
    /// Link state and device identity, shared with the client.
    link: Arc<Link>,
}

impl ControlState {
    #[cfg(test)]
    fn new() -> Self {
        Self::with_link(Arc::new(Link::default()))
    }

    fn with_link(link: Arc<Link>) -> Self {
        Self {
            pending: Registry::with_start(INITIAL_COMMAND_ID),
            link,
        }
    }

    fn firmware_version(&self) -> Option<FirmwareVersion> {
        self.link.firmware()
    }

    /// How commands are framed on a connection that negotiated `mode`, given
    /// what is known about the device right now.
    fn profile(&self, mode: Verification) -> WireProfile {
        WireProfile::new(mode).with_firmware(self.firmware_version())
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
    link: Arc<Link>,
) {
    let state = Arc::new(ControlState::with_link(Arc::clone(&link)));
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
            Ok(Ok(stream)) => {
                if let Err(e) = stream.set_nodelay(true) {
                    warn!("failed to set TCP_NODELAY: {e}");
                }
                info!("connected to telescope control at {addr}");

                // One buffered reader for the socket's lifetime: bytes it
                // buffers during negotiation belong to the reader task.
                let (read_half, mut write_half) = tokio::io::split(stream);
                let mut reader = BufReader::new(read_half);

                // Negotiate before advertising the connection as ready. An
                // unverified socket silently consumes requests.
                match crate::auth::negotiate(&mut reader, &mut write_half, interop_key.as_deref())
                    .await
                {
                    Ok(negotiated) => {
                        link.set(LinkState::Negotiated(negotiated.mode));
                        for msg in &negotiated.deferred {
                            if json_rpc::is_event(msg) {
                                handle_event(msg, &event_tx);
                            }
                        }
                        // Learn the firmware and model without waiting: the
                        // reader records them when the reply arrives.
                        let probe = state
                            .profile(negotiated.mode)
                            .line(&Command::GetDeviceState, ID_CONNECT_DEVICE_STATE);
                        if let Err(e) = write_half.write_all(probe.as_bytes()).await {
                            warn!("failed to request device state at connect: {e}");
                        }

                        connected.store(true, Ordering::Release);
                        policy.reset();

                        run_connected(
                            reader,
                            write_half,
                            negotiated.mode,
                            Arc::clone(&state),
                            Arc::clone(&request_rx),
                            event_tx.clone(),
                            shutdown.clone(),
                        )
                        .await;

                        connected.store(false, Ordering::Release);
                        link.set(LinkState::Unknown);
                        info!("control connection lost, will reconnect");
                    }
                    Err(SeestarError::VerificationRequired) => {
                        error!("{}", SeestarError::VerificationRequired);
                        link.set(LinkState::KeyRequired);
                        // Queued commands can never be sent; fail them now
                        // instead of letting each one run out its timeout.
                        let mut rx = request_rx.lock().await;
                        while let Ok(req) = rx.try_recv() {
                            let _ = req
                                .response_tx
                                .send(Err(SeestarError::VerificationRequired));
                        }
                    }
                    Err(e) => {
                        // Falls through to the backoff delay below rather than
                        // retrying immediately — avoids hammering the scope.
                        error!("verification negotiation failed: {e}");
                    }
                }
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
    reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    write_half: tokio::io::WriteHalf<TcpStream>,
    mode: Verification,
    state: Arc<ControlState>,
    request_rx: Arc<Mutex<mpsc::Receiver<ClientRequest>>>,
    event_tx: broadcast::Sender<SeestarEvent>,
    shutdown: tokio::sync::watch::Receiver<bool>,
) {
    // Channel for the writer task to send serialized messages
    let (write_tx, mut write_rx) = mpsc::channel::<String>(256);

    // Reader died signal
    let (reader_dead_tx, reader_dead_rx) = oneshot::channel::<()>();

    // Writer task
    let writer_handle = {
        let state = Arc::clone(&state);
        let shutdown = shutdown.clone();

        tokio::spawn(async move {
            writer_task(
                write_half,
                &mut write_rx,
                request_rx,
                state,
                mode,
                reader_dead_rx,
                shutdown,
            )
            .await;
        })
    };

    // Reader task
    let reader_handle = {
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            reader_task(reader, state, event_tx).await;
            let _ = reader_dead_tx.send(());
        })
    };

    // Heartbeat: framed by the connection's wire profile like any command.
    let hb_handle = tokio::spawn(heartbeat_task(write_tx, shutdown, Arc::clone(&state), mode));

    // Wait for either task to finish
    tokio::select! {
        _ = writer_handle => {},
        _ = reader_handle => {},
    }

    hb_handle.abort();
}

/// Reader task: reads lines from TCP, routes responses and events.
async fn reader_task(
    mut reader: BufReader<tokio::io::ReadHalf<TcpStream>>,
    state: Arc<ControlState>,
    event_tx: broadcast::Sender<SeestarEvent>,
) {
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
            // Unmodeled events are ordinary on newer firmware; they reach
            // subscribers with their name and payload.
            if event.is_unknown() {
                debug!(event = event.name(), "received unmodeled event");
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

    // Record what the device says about itself in device state responses.
    if json_rpc::method_name(msg) == Some("get_device_state")
        && let Some(result) = msg.get("result")
    {
        let mut info = state.link.lock();
        let before = info.device.firmware;
        if info.device.update_from_device_state(result) && info.device.firmware != before {
            debug!(firmware = ?info.device.firmware, "detected firmware version");
        }
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
    mode: Verification,
    mut reader_dead_rx: oneshot::Receiver<()>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
) {
    loop {
        let mut req_rx = request_rx.lock().await;
        tokio::select! {
            // Client request
            Some(req) = req_rx.recv() => {
                drop(req_rx); // release lock before doing work
                let method = req.command.method();
                let id = state.pending.register(req.response_tx);
                let line = state.profile(mode).line(&req.command, id);

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
}

/// Heartbeat task: sends `pi_get_time` every 5 seconds, framed by the
/// connection's current wire profile.
async fn heartbeat_task(
    tx: mpsc::Sender<String>,
    mut shutdown: tokio::sync::watch::Receiver<bool>,
    state: Arc<ControlState>,
    mode: Verification,
) {
    let mut interval = tokio::time::interval(HEARTBEAT_INTERVAL);
    // Skip the first immediate tick
    interval.tick().await;

    loop {
        tokio::select! {
            _ = interval.tick() => {
                // A fixed low id: heartbeat replies are not tracked.
                let line = state.profile(mode).line(&Command::PiGetTime, 1);
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

    /// Run the real heartbeat task under paused time and return the first
    /// line it sends on a connection with this mode and known firmware.
    async fn heartbeat_line(mode: Verification, firmware: Option<u32>) -> String {
        let state = Arc::new(ControlState::new());
        state.link.lock().device.firmware = firmware.map(FirmwareVersion);
        let (tx, mut rx) = mpsc::channel::<String>(4);
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(heartbeat_task(tx, shutdown_rx, state, mode));

        // Nothing is sent before the first interval elapses.
        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());

        tokio::time::advance(HEARTBEAT_INTERVAL + Duration::from_millis(1)).await;
        let line = rx.recv().await.expect("heartbeat line");

        shutdown_tx.send(true).unwrap();
        handle.await.unwrap();
        line
    }

    const MARKED: &str = "{\"id\":1,\"method\":\"pi_get_time\",\"params\":[\"verify\"]}\r\n";
    const PLAIN: &str = "{\"id\":1,\"method\":\"pi_get_time\"}\r\n";

    // The heartbeat is framed like any other command (this closes
    // docs/bugs/heartbeat-hardcoded-verify.md): it used to bypass the
    // serializer and always carry the marker, diverging from
    // pyscopinator's `_should_inject_verify` on firmware <= 2582.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_follows_the_wire_profile() {
        let old = Some(FirmwareVersion::VERIFY_THRESHOLD);
        let new = Some(2670);
        // Before the firmware is known a legacy connection marks it, exactly
        // the bytes the heartbeat always had.
        assert_eq!(heartbeat_line(Verification::Legacy, None).await, MARKED);
        assert_eq!(heartbeat_line(Verification::Legacy, new).await, MARKED);
        // Old firmware predates the marker.
        assert_eq!(heartbeat_line(Verification::Legacy, old).await, PLAIN);
        assert_eq!(
            heartbeat_line(Verification::Legacy, Some(2000)).await,
            PLAIN
        );
        // Marker mode always marks; a verified socket never does (hardware,
        // S50 fw 6.70: unmarked pi_get_time is served once verified).
        assert_eq!(heartbeat_line(Verification::Marker, old).await, MARKED);
        assert_eq!(heartbeat_line(Verification::Handshake, None).await, PLAIN);
        assert_eq!(heartbeat_line(Verification::Handshake, new).await, PLAIN);
    }

    // The heartbeat bytes are exactly what the serializer produces.
    #[tokio::test(start_paused = true)]
    async fn heartbeat_matches_the_serializer() {
        for (mode, fw) in [
            (Verification::Legacy, None),
            (Verification::Legacy, Some(2000)),
            (Verification::Marker, Some(2706)),
            (Verification::Handshake, Some(2670)),
        ] {
            let profile = WireProfile::new(mode).with_firmware(fw.map(FirmwareVersion));
            assert_eq!(
                heartbeat_line(mode, fw).await,
                profile.line(&Command::PiGetTime, 1)
            );
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
        link: Arc<Link>,
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
        let link = Arc::new(Link::default());
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);
        let handle = tokio::spawn(run(
            addr,
            request_rx,
            event_tx,
            Arc::clone(&connected),
            shutdown_rx,
            key,
            Arc::clone(&link),
        ));
        Harness {
            conns,
            connected,
            link,
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

        // This telescope never answers, so each connection first spends
        // PROBE_TIMEOUT on the unanswered `get_verify_str` before it is
        // treated as legacy and advertised as connected.
        async fn settle(h: &Harness, c: &mut Conn) {
            assert!(!h.connected.load(Ordering::Acquire));
            assert_eq!(h.link.get(), LinkState::Unknown);
            run_for(crate::auth::PROBE_TIMEOUT + Duration::from_millis(50)).await;
            let probe = c.lines.try_recv().expect("negotiation request");
            assert!(probe.contains("get_verify_str"), "got {probe:?}");
            // Then the device-state request the loop sends by itself.
            let probe = c.lines.try_recv().expect("device state request");
            assert!(probe.contains("get_device_state"), "got {probe:?}");
            assert!(h.connected.load(Ordering::Acquire));
            assert_eq!(h.link.get(), LinkState::Negotiated(Verification::Legacy));
        }

        let mut c1 = h.next_conn(Duration::from_secs(1)).await;
        settle(&h, &mut c1).await;
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
        settle(&h, &mut c2).await;

        // No heartbeat until (almost) a full interval on the new connection...
        run_for(HEARTBEAT_INTERVAL - Duration::from_millis(300)).await;
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

/// Ports of the pyscopinator connection edge-case tests
/// (`tests/unit/test_connection.py`) that live at the unit seam: fixed
/// connection constants and response routing. The client-level ports of the
/// same oracle live in `tests/control_integration.rs`, whose header comment
/// accounts for all 43 Python tests.
#[cfg(test)]
mod connection_edge_case_tests {
    use super::*;

    // Python TestConnectionInit::test_fields pins the constructor defaults
    // (host, port, 5.0 s connection/read/write timeouts, zeroed counters).
    // Rust has no connection object to configure: the timeouts are compile-time
    // constants, not per-instance settings, and there are no message counters.
    // Pin the constants. DIFFERENCE vs Python: the fixture's 5.0 s connect
    // timeout is a constructor argument there; here CONNECT_TIMEOUT is a
    // fixed 10 s (and HEARTBEAT_INTERVAL a fixed 5 s) with no override.
    #[test]
    fn connect_and_heartbeat_timeouts_are_fixed_constants() {
        assert_eq!(CONNECT_TIMEOUT, Duration::from_secs(10));
        assert_eq!(HEARTBEAT_INTERVAL, Duration::from_secs(5));
    }

    // Python TestRead::test_read_success: read() returns the raw JSON text of
    // one line and bumps read_messages. The Rust counterpart of "the line was
    // read and handed on" is handle_response routing the parsed response to
    // the pending caller. DIFFERENCE vs Python: there is no raw-text read API
    // and no counter — the control path delivers typed CommandResponse values.
    #[test]
    fn handle_response_delivers_to_registered_caller() {
        let state = Arc::new(ControlState::new());
        let (tx, rx) = oneshot::channel();
        let id = state.pending.register(tx);

        let msg = serde_json::json!({
            "jsonrpc": "2.0", "id": id, "code": 0,
            "result": {"answer": 42},
        });
        handle_response(&msg, &state);

        let resp = rx
            .blocking_recv()
            .expect("pending caller must be resolved")
            .expect("response must be Ok");
        assert_eq!(resp.id, id);
        assert_eq!(resp.code, 0);
        assert_eq!(resp.result.unwrap()["answer"], 42);
    }

    // No Python original: pyscopinator's read() hands raw text to the caller,
    // so id correlation is the caller's problem there. Rust's reader routes
    // by id, and a well-formed response whose id matches no pending request
    // must be dropped (warn/trace) without panicking and without resolving
    // any other pending caller.
    #[test]
    fn handle_response_for_unknown_id_is_dropped_without_panic() {
        let state = Arc::new(ControlState::new());
        let (tx, mut rx) = oneshot::channel();
        let id = state.pending.register(tx);

        let msg = serde_json::json!({"id": id + 1, "code": 0});
        handle_response(&msg, &state);

        // Our caller is still pending: the unknown-id response resolved no one.
        assert!(rx.try_recv().is_err());
        // And the registry still holds it, so a later correct response works.
        let msg = serde_json::json!({"id": id, "code": 0});
        handle_response(&msg, &state);
        let resp = rx.blocking_recv().expect("resolved by the matching id");
        assert!(resp.is_ok());
    }
}
