use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{debug, info};

use crate::auth::InteropKey;
use crate::command::params::{
    Direction, PolarAlignParams, SetTimeParams, SetUserLocationParams, SpeedMoveParams,
};
use crate::command::{Command, ImagingCommand};
use crate::connection::control::{self, ClientRequest};
use crate::connection::imaging::{self, ImageFrame};
use crate::error::SeestarError;
use crate::event::SeestarEvent;
use crate::protocol::json_rpc::{CONTROL_PORT, IMAGING_PORT};
use crate::response::CommandResponse;

/// Configuration for connecting to a Seestar telescope.
#[derive(Default)]
pub struct SeestarConfig {
    /// RSA interoperability PEM key for firmware 7.18+ challenge/response authentication.
    /// If `None`, authentication is skipped (compatible with older firmware).
    pub interop_key: Option<InteropKey>,
    /// How long [`SeestarClient::send_command`] waits for a response before
    /// returning [`SeestarError::Timeout`]. Defaults to [`DEFAULT_RESPONSE_TIMEOUT`].
    pub response_timeout: Option<Duration>,
}

/// Default command response timeout.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// Fixed `id` used for client-issued imaging-port commands. The imaging port
/// does not correlate responses to ids, so the exact value is immaterial.
const IMAGING_CMD_ID: u64 = 21;

/// Client for communicating with a Seestar smart telescope.
///
/// The client manages two TCP connections (control port 4700 and imaging
/// port 4800), automatic heartbeats, command-response correlation, and
/// reconnection.
///
/// # Example
///
/// ```no_run
/// # async fn example() -> Result<(), Box<dyn std::error::Error>> {
/// use scopinator_seestar::client::SeestarClient;
/// use scopinator_seestar::command::Command;
///
/// let client = SeestarClient::connect("192.168.1.100".parse()?).await?;
///
/// let response = client.send_command(Command::GetDeviceState).await?;
/// println!("device state: {:?}", response);
///
/// client.shutdown().await;
/// # Ok(())
/// # }
/// ```
pub struct SeestarClient {
    /// Channel to send requests to the control writer task.
    request_tx: mpsc::Sender<ClientRequest>,
    /// Sender side of the event broadcast (for subscribing).
    event_tx: broadcast::Sender<SeestarEvent>,
    /// Sender side of the frame broadcast (for subscribing).
    frame_tx: broadcast::Sender<Arc<ImageFrame>>,
    /// Channel to send raw JSON commands to the imaging port (port 4800).
    imaging_cmd_tx: mpsc::Sender<Vec<u8>>,
    /// Whether the control connection is alive.
    control_connected: Arc<AtomicBool>,
    /// Whether the imaging connection is alive.
    imaging_connected: Arc<AtomicBool>,
    /// Shutdown signal.
    shutdown_tx: watch::Sender<bool>,
    /// Per-command response timeout.
    response_timeout: Duration,
}

impl SeestarClient {
    /// Connect to a Seestar telescope at the given IP address.
    ///
    /// This starts background tasks for the control and imaging connections.
    /// The connections will automatically reconnect on failure.
    pub async fn connect(ip: Ipv4Addr) -> Result<Self, SeestarError> {
        let control_addr = SocketAddr::V4(SocketAddrV4::new(ip, CONTROL_PORT));
        let imaging_addr = SocketAddr::V4(SocketAddrV4::new(ip, IMAGING_PORT));

        Self::connect_internal(ip, control_addr, imaging_addr, SeestarConfig::default()).await
    }

    /// Connect with a [`SeestarConfig`] (e.g. to supply an interop key for firmware 7.18+).
    pub async fn connect_with_config(
        ip: Ipv4Addr,
        config: SeestarConfig,
    ) -> Result<Self, SeestarError> {
        let control_addr = SocketAddr::V4(SocketAddrV4::new(ip, CONTROL_PORT));
        let imaging_addr = SocketAddr::V4(SocketAddrV4::new(ip, IMAGING_PORT));

        Self::connect_internal(ip, control_addr, imaging_addr, config).await
    }

    /// Connect with explicit addresses (useful for proxies or custom ports).
    pub async fn connect_with_ports(
        ip: Ipv4Addr,
        control_addr: SocketAddr,
        imaging_addr: SocketAddr,
    ) -> Result<Self, SeestarError> {
        Self::connect_internal(ip, control_addr, imaging_addr, SeestarConfig::default()).await
    }

    /// Connect with explicit addresses and a [`SeestarConfig`].
    pub async fn connect_with_ports_and_config(
        ip: Ipv4Addr,
        control_addr: SocketAddr,
        imaging_addr: SocketAddr,
        config: SeestarConfig,
    ) -> Result<Self, SeestarError> {
        Self::connect_internal(ip, control_addr, imaging_addr, config).await
    }

    async fn connect_internal(
        _ip: Ipv4Addr, // reserved — may be used for source binding in future
        control_addr: SocketAddr,
        imaging_addr: SocketAddr,
        config: SeestarConfig,
    ) -> Result<Self, SeestarError> {
        let (request_tx, request_rx) = mpsc::channel::<ClientRequest>(256);
        let (event_tx, _) = broadcast::channel::<SeestarEvent>(256);
        let (frame_tx, _) = broadcast::channel::<Arc<ImageFrame>>(32);
        let (imaging_cmd_tx, imaging_cmd_rx) = mpsc::channel::<Vec<u8>>(64);
        let (shutdown_tx, shutdown_rx) = watch::channel(false);

        let control_connected = Arc::new(AtomicBool::new(false));
        let imaging_connected = Arc::new(AtomicBool::new(false));

        let interop_key = config.interop_key.map(Arc::new);
        let response_timeout = config.response_timeout.unwrap_or(DEFAULT_RESPONSE_TIMEOUT);

        // Spawn control connection task
        {
            let event_tx = event_tx.clone();
            let connected = Arc::clone(&control_connected);
            let shutdown_rx = shutdown_rx.clone();
            let interop_key = interop_key.clone();

            tokio::spawn(async move {
                control::run(
                    control_addr,
                    request_rx,
                    event_tx,
                    connected,
                    shutdown_rx,
                    interop_key,
                )
                .await;
            });
        }

        // Spawn imaging connection task
        {
            let frame_tx = frame_tx.clone();
            let connected = Arc::clone(&imaging_connected);
            let shutdown_rx = shutdown_rx.clone();

            tokio::spawn(async move {
                imaging::run(
                    imaging_addr,
                    frame_tx,
                    connected,
                    shutdown_rx,
                    imaging_cmd_rx,
                )
                .await;
            });
        }

        info!(
            "client started, connecting to {control_addr} (control) and {imaging_addr} (imaging)"
        );

        Ok(Self {
            request_tx,
            event_tx,
            frame_tx,
            imaging_cmd_tx,
            control_connected,
            imaging_connected,
            shutdown_tx,
            response_timeout,
        })
    }

    /// Send a command and wait for the response.
    pub async fn send_command(&self, cmd: Command) -> Result<CommandResponse, SeestarError> {
        let (response_tx, response_rx) = oneshot::channel();

        let request = ClientRequest {
            command: cmd,
            response_tx,
        };

        self.request_tx
            .send(request)
            .await
            .map_err(|_| SeestarError::Disconnected)?;

        match tokio::time::timeout(self.response_timeout, response_rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(SeestarError::Disconnected),
            Err(_) => Err(SeestarError::Timeout(self.response_timeout)),
        }
    }

    /// Send a command and return the result, failing on non-zero response codes.
    pub async fn send_and_validate(&self, cmd: Command) -> Result<serde_json::Value, SeestarError> {
        let response = self.send_command(cmd).await?;
        response
            .into_result()
            .map_err(|e| SeestarError::CommandFailed {
                code: e.code,
                message: e.message,
            })
    }

    /// Subscribe to telescope events.
    pub fn subscribe_events(&self) -> broadcast::Receiver<SeestarEvent> {
        self.event_tx.subscribe()
    }

    /// Subscribe to imaging frames.
    pub fn subscribe_frames(&self) -> broadcast::Receiver<Arc<ImageFrame>> {
        self.frame_tx.subscribe()
    }

    /// Send a raw JSON command to the imaging port (port 4800).
    ///
    /// `msg` should be the JSON payload without a trailing newline — the
    /// `\r\n` terminator is appended automatically.
    pub async fn send_imaging_command(&self, mut msg: Vec<u8>) -> Result<(), SeestarError> {
        msg.extend_from_slice(b"\r\n");
        self.imaging_cmd_tx
            .send(msg)
            .await
            .map_err(|_| SeestarError::Disconnected)
    }

    /// Send a typed [`ImagingCommand`] to the imaging port (4800).
    ///
    /// Fire-and-forget: the scope replies with binary frames (via
    /// [`subscribe_frames`](Self::subscribe_frames)), not a correlated response,
    /// so this returns as soon as the command is queued for the socket. For
    /// arbitrary/raw payloads use [`send_imaging_command`](Self::send_imaging_command).
    pub async fn send_imaging(&self, cmd: ImagingCommand) -> Result<(), SeestarError> {
        self.send_imaging_command(cmd.serialize(IMAGING_CMD_ID))
            .await
    }

    /// Start the live imaging frame stream.
    ///
    /// Convenience for [`send_imaging`](Self::send_imaging) with
    /// [`ImagingCommand::BeginStreaming`]. This goes to the **imaging port
    /// (4800)** — the only place the telescope accepts it; on the control port
    /// (4700) it is rejected with code 103. In star mode the scope then pushes
    /// full-resolution raw frames (`FrameKind::Preview`); solar/moon/planet/
    /// scenery modes use a separate RTSP stream instead.
    ///
    /// Call this after entering a view with
    /// [`Command::IscopeStartView`](crate::command::Command::IscopeStartView) on
    /// the control port — `begin_streaming` starts the frame pipe, not the view.
    pub async fn begin_streaming(&self) -> Result<(), SeestarError> {
        self.send_imaging(ImagingCommand::BeginStreaming).await
    }

    /// Manually jog the mount toward a cardinal [`Direction`].
    ///
    /// This is an **open-loop** motor move (`scope_speed_move`) — unlike a goto
    /// it needs no polar alignment. `level` is the speed gear, `percent` the
    /// speed (`0` stops), `dur_sec` the run time; the scope auto-stops after
    /// `dur_sec`. The direction→angle mapping is verified for **EQ mode** on
    /// firmware 6.70 (see [`Direction`]); call [`stop_jog`](Self::stop_jog) to
    /// halt early.
    pub async fn jog(
        &self,
        direction: Direction,
        level: i32,
        percent: i32,
        dur_sec: i32,
    ) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::ScopeSpeedMove(SpeedMoveParams::toward(
            direction, level, percent, dur_sec,
        )))
        .await
    }

    /// Stop manual jogging immediately (`scope_speed_move` with `percent = 0`).
    pub async fn stop_jog(&self) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::ScopeSpeedMove(SpeedMoveParams::stop()))
            .await
    }

    /// Start the telescope's auto-focus routine (`start_auto_focuse` — the
    /// firmware's spelling is intentional).
    ///
    /// Auto-focus requires the scope to be polar-aligned and able to see stars;
    /// it fails from a fresh, unaligned park. Use
    /// [`stop_auto_focus`](Self::stop_auto_focus) to cancel an in-progress run.
    pub async fn start_auto_focus(&self) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::StartAutoFocus).await
    }

    /// Cancel an in-progress auto-focus routine (`stop_auto_focuse`).
    pub async fn stop_auto_focus(&self) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::StopAutoFocus).await
    }

    /// Send the basic startup commands: set the telescope clock (UTC, from the
    /// local system clock) and the observing location, then return its
    /// verification status.
    ///
    /// This does **not** perform polar alignment — call
    /// [`start_polar_align`](Self::start_polar_align) separately (EQ mode, clear
    /// sky). For a specific local time-zone string, build the [`SetTimeParams`]
    /// yourself and send [`Command::PiSetTime`] instead of using this helper.
    ///
    /// Returns `true` if the scope reports itself verified (`pi_is_verified`).
    pub async fn startup(&self, lat: f64, lon: f64) -> Result<bool, SeestarError> {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        self.send_and_validate(Command::PiSetTime(SetTimeParams::from_unix_utc(now)))
            .await?;
        self.send_and_validate(Command::SetUserLocation(SetUserLocationParams {
            lat,
            lon,
            force: true,
        }))
        .await?;
        // `pi_is_verified` returns a bare boolean `result` (some firmware nests
        // it under `is_verified`); treat anything else as not verified.
        let resp = self.send_command(Command::PiIsVerified).await?;
        Ok(resp
            .result
            .as_ref()
            .and_then(|r| {
                r.as_bool()
                    .or_else(|| r.get("is_verified").and_then(|v| v.as_bool()))
            })
            .unwrap_or(false))
    }

    /// Trigger EQ-mode 3-point polar alignment ("3PPA", `start_polar_align`).
    ///
    /// Requires the mount to be in EQ mode and able to see stars (it images and
    /// plate-solves); it fails otherwise. `restart` begins a fresh alignment;
    /// `dec_pos_index` selects the declination position for the points.
    pub async fn start_polar_align(
        &self,
        restart: bool,
        dec_pos_index: i32,
    ) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::StartPolarAlign(PolarAlignParams {
            restart,
            dec_pos_index,
        }))
        .await
    }

    /// Returns true if the control connection is currently alive.
    pub fn is_control_connected(&self) -> bool {
        self.control_connected.load(Ordering::Acquire)
    }

    /// Returns true if the imaging connection is currently alive.
    pub fn is_imaging_connected(&self) -> bool {
        self.imaging_connected.load(Ordering::Acquire)
    }

    /// Wait for the control connection to be established.
    pub async fn wait_for_connection(&self, timeout: Duration) -> Result<(), SeestarError> {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_control_connected() {
            if tokio::time::Instant::now() >= deadline {
                return Err(SeestarError::Timeout(timeout));
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        Ok(())
    }

    /// Shut down all connections gracefully.
    pub async fn shutdown(&self) {
        debug!("shutting down client");
        let _ = self.shutdown_tx.send(true);
    }
}

impl Drop for SeestarClient {
    fn drop(&mut self) {
        let _ = self.shutdown_tx.send(true);
    }
}
