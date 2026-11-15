use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tracing::{debug, info};

use crate::auth::{InteropKey, Verification};
use crate::command::params::{
    Direction, PolarAlignParams, SetTimeParams, SetUserLocationParams, SettingParams,
    SpeedMoveParams, StartViewParams,
};
use crate::command::{Command, ImagingCommand};
use crate::connection::control::{self, ClientRequest, Link, LinkState};
use crate::connection::imaging::{self, ImageFrame};
use crate::error::SeestarError;
use crate::event::SeestarEvent;
use crate::protocol::json_rpc::{CONTROL_PORT, IMAGING_PORT};
use crate::response::CommandResponse;

/// Configuration for connecting to a Seestar telescope.
#[derive(Default)]
pub struct SeestarConfig {
    /// RSA interoperability key for the `verify_client` challenge/response
    /// handshake (present on firmware 6.70 and later).
    ///
    /// With a key the control socket is verified at connect and requests are
    /// sent without the `verify` marker. With `None` the client falls back to
    /// the marker where the device still serves it; a device that does not
    /// fails with [`SeestarError::VerificationRequired`]. See
    /// [`SeestarClient::verification`].
    pub interop_key: Option<InteropKey>,
    /// How long [`SeestarClient::send_command`] waits for a response before
    /// returning [`SeestarError::Timeout`]. Defaults to [`DEFAULT_RESPONSE_TIMEOUT`].
    pub response_timeout: Option<Duration>,
}

/// Device error code for an unexpected (unknown) parameter.
const CODE_UNEXPECTED_PARAM: i32 = 109;

/// What happened to one setting sent with [`SeestarClient::set_setting`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingOutcome {
    /// The device accepted it, or the value read back afterwards matches.
    Applied,
    /// The device answered 109 "unexpected param" and the value could not be
    /// confirmed by reading the settings back: either `get_setting` does not
    /// report this setting, or it reports a different value.
    ///
    /// This only arises on a connection that needs the `verify` marker, where
    /// the marker itself draws a 109 on every `set_setting`. The setting may
    /// have been applied, or its name may be unknown to this firmware.
    Unconfirmed,
    /// The device rejected it with this error.
    Rejected { code: i32, message: String },
}

/// The outcome for one member of a [`SeestarClient::set_setting`] call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SettingResult {
    /// The setting's wire name, e.g. `stack_after_goto`.
    pub key: String,
    pub outcome: SettingOutcome,
}

impl SettingResult {
    pub fn is_applied(&self) -> bool {
        self.outcome == SettingOutcome::Applied
    }
}

/// True if `actual` (a value read back from the device) agrees with
/// `expected` (the value that was set). Objects need only contain the expected
/// members; numbers compare by value; a boolean matches a number by
/// truthiness, since the firmware takes and reports either for a flag.
fn setting_matches(actual: &serde_json::Value, expected: &serde_json::Value) -> bool {
    use serde_json::Value;
    let flag = |v: &Value| match v {
        Value::Bool(b) => Some(*b),
        Value::Number(n) => n.as_f64().map(|f| f != 0.0),
        _ => None,
    };
    match (actual, expected) {
        (Value::Object(a), Value::Object(e)) => e
            .iter()
            .all(|(k, ev)| a.get(k).is_some_and(|av| setting_matches(av, ev))),
        (Value::Number(a), Value::Number(e)) => a.as_f64() == e.as_f64(),
        (Value::Bool(_), Value::Number(_)) | (Value::Number(_), Value::Bool(_)) => {
            flag(actual) == flag(expected)
        }
        _ => actual == expected,
    }
}

/// Default command response timeout.
pub const DEFAULT_RESPONSE_TIMEOUT: Duration = Duration::from_secs(30);

/// `id` used for client-issued imaging-port commands. The device echoes its
/// low byte in every reply header, and stream frames repeat the id of
/// `begin_streaming`.
const IMAGING_CMD_ID: u64 = imaging::request_id::STREAM as u64;

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
    /// Verification state of the control connection.
    link: Arc<Link>,
    /// Whether the imaging connection is alive.
    imaging_connected: Arc<AtomicBool>,
    /// Shutdown signal.
    shutdown_tx: watch::Sender<bool>,
    /// Per-command response timeout.
    response_timeout: Duration,
    /// Serializes one-shot image requests and counts them for id allocation.
    /// Only one may be outstanding: the device builds compressed replies in a
    /// single fixed-name scratch file.
    image_requests: tokio::sync::Mutex<u8>,
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
        let link = Arc::new(Link::default());
        let imaging_connected = Arc::new(AtomicBool::new(false));

        let interop_key = config.interop_key.map(Arc::new);
        let response_timeout = config.response_timeout.unwrap_or(DEFAULT_RESPONSE_TIMEOUT);

        // Spawn control connection task
        {
            let event_tx = event_tx.clone();
            let connected = Arc::clone(&control_connected);
            let shutdown_rx = shutdown_rx.clone();
            let interop_key = interop_key.clone();
            let link = Arc::clone(&link);

            tokio::spawn(async move {
                control::run(
                    control_addr,
                    request_rx,
                    event_tx,
                    connected,
                    shutdown_rx,
                    interop_key,
                    link,
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
            link,
            imaging_connected,
            shutdown_tx,
            response_timeout,
            image_requests: tokio::sync::Mutex::new(0),
        })
    }

    /// Send a command and wait for the response.
    ///
    /// Fails at once with [`SeestarError::VerificationRequired`] if the device
    /// is known to need an interop key that is not configured.
    pub async fn send_command(&self, cmd: Command) -> Result<CommandResponse, SeestarError> {
        if self.link.get() == LinkState::KeyRequired {
            return Err(SeestarError::VerificationRequired);
        }

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

    /// Request one image on the imaging port and wait for its reply.
    ///
    /// For [`ImagingCommand::GetStackedImage`],
    /// [`GetCurrentImage`](ImagingCommand::GetCurrentImage) and
    /// [`GetPhotometryImage`](ImagingCommand::GetPhotometryImage). The reply
    /// is matched by the request id the device echoes in the frame header,
    /// so frames of a running stream (or a heartbeat echo) are never taken
    /// for it. Use [`ImageFrame::image_bytes`] to get the pixel data, which
    /// inflates a zip-compressed reply.
    ///
    /// Errors: [`SeestarError::ImageRequest`] with the device's message when
    /// it refuses (e.g. `there is no image now`), [`SeestarError::Timeout`]
    /// when no reply arrives within the response timeout. Requests are
    /// serialized; a second caller waits for the first to finish.
    pub async fn request_image(
        &self,
        cmd: ImagingCommand,
    ) -> Result<Arc<ImageFrame>, SeestarError> {
        use imaging::request_id;

        let mut counter = self.image_requests.lock().await;
        let first = match cmd {
            ImagingCommand::GetStackedImage => request_id::STACK_FIRST,
            _ => request_id::ONE_SHOT_FIRST,
        };
        let id = first + *counter % request_id::BLOCK_LEN;
        *counter = counter.wrapping_add(1);

        // Subscribe before sending so the reply cannot be missed.
        let mut frames = self.subscribe_frames();
        self.send_imaging_command(cmd.serialize(u64::from(id)))
            .await?;

        let deadline = tokio::time::Instant::now() + self.response_timeout;
        loop {
            let frame = match tokio::time::timeout_at(deadline, frames.recv()).await {
                Err(_) => return Err(SeestarError::Timeout(self.response_timeout)),
                Ok(Err(broadcast::error::RecvError::Closed)) => {
                    return Err(SeestarError::Disconnected);
                }
                // Skipped frames were stream frames we were not waiting for.
                Ok(Err(broadcast::error::RecvError::Lagged(_))) => continue,
                Ok(Ok(frame)) => frame,
            };
            if frame.header.request_id != id {
                continue;
            }
            if let Some(message) = frame.error_text() {
                return Err(SeestarError::ImageRequest(message.into_owned()));
            }
            if !frame.payload_kind().is_image() {
                return Err(SeestarError::Protocol(format!(
                    "{} answered with {:?} instead of an image",
                    cmd.method(),
                    frame.payload_kind()
                )));
            }
            return Ok(frame);
        }
    }

    /// Fetch the stacked image ([`ImagingCommand::GetStackedImage`] through
    /// [`request_image`](Self::request_image)).
    pub async fn get_stacked_image(&self) -> Result<Arc<ImageFrame>, SeestarError> {
        self.request_image(ImagingCommand::GetStackedImage).await
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
    /// [`Command::IscopeStartView`] on
    /// the control port — `begin_streaming` starts the frame pipe, not the view.
    pub async fn begin_streaming(&self) -> Result<(), SeestarError> {
        self.send_imaging(ImagingCommand::BeginStreaming).await
    }

    /// Change device settings and report what happened to each one.
    ///
    /// On a connection verified by the handshake (an interop key is
    /// configured) this is one `set_setting` request: code 0 applies every
    /// member, and any other code is returned as
    /// [`SeestarError::CommandFailed`]. The firmware does not roll back, so
    /// members ahead of the rejected one may already have been applied.
    ///
    /// On a connection that relies on the `verify` marker, the firmware
    /// answers every `set_setting` with 109 because of the marker, whether or
    /// not it applied the setting. There each member is sent in its own
    /// request, and a 109 is resolved by reading the settings back with
    /// `get_setting`: a matching value is [`SettingOutcome::Applied`],
    /// anything else [`SettingOutcome::Unconfirmed`]. Other error codes are
    /// [`SettingOutcome::Rejected`].
    ///
    /// Results are in the order the settings were sent (alphabetical by wire
    /// name). `Err` is returned only for transport failures, or for a
    /// rejected request on a verified connection.
    pub async fn set_setting(
        &self,
        params: SettingParams,
    ) -> Result<Vec<SettingResult>, SeestarError> {
        let serde_json::Value::Object(members) = serde_json::to_value(&params)? else {
            return Err(SeestarError::Protocol(
                "set_setting params did not serialize to an object".to_string(),
            ));
        };
        if members.is_empty() {
            return Ok(Vec::new());
        }

        if self.verification() == Some(Verification::Handshake) {
            self.send_and_validate(Command::SetSetting(params)).await?;
            return Ok(members
                .into_iter()
                .map(|(key, _)| SettingResult {
                    key,
                    outcome: SettingOutcome::Applied,
                })
                .collect());
        }

        // One member per request, so that an error belongs to one setting.
        let mut results = Vec::with_capacity(members.len());
        let mut unconfirmed = false;
        for (key, value) in &members {
            let single: SettingParams =
                serde_json::from_value(serde_json::json!({ key.as_str(): value }))?;
            let resp = self.send_command(Command::SetSetting(single)).await?;
            let outcome = match resp.code {
                0 => SettingOutcome::Applied,
                CODE_UNEXPECTED_PARAM => {
                    unconfirmed = true;
                    SettingOutcome::Unconfirmed
                }
                code => SettingOutcome::Rejected {
                    code,
                    message: resp.error.unwrap_or_default(),
                },
            };
            results.push(SettingResult {
                key: key.clone(),
                outcome,
            });
        }

        if unconfirmed {
            let current = self.send_and_validate(Command::GetSetting).await?;
            for result in &mut results {
                if result.outcome == SettingOutcome::Unconfirmed
                    && current
                        .get(&result.key)
                        .is_some_and(|actual| setting_matches(actual, &members[&result.key]))
                {
                    result.outcome = SettingOutcome::Applied;
                }
            }
        }
        Ok(results)
    }

    /// Slew to a target and start viewing it (a "goto").
    ///
    /// `ra_hours` is right ascension in **hours** (0..24) and `dec_deg`
    /// declination in degrees. This sends `iscope_start_view` in star mode
    /// with the target, which is how the firmware slews to coordinates.
    ///
    /// A successful reply means the request was accepted, not that the scope
    /// has arrived. The slew, plate solve and re-centring are reported by
    /// `AutoGoto` / `ScopeGoto` events (see
    /// [`subscribe_events`](Self::subscribe_events)); cancel with
    /// [`Command::IscopeStopView`] and [`StopStage::AutoGoto`](crate::command::params::StopStage::AutoGoto).
    pub async fn goto(
        &self,
        target_name: &str,
        ra_hours: f64,
        dec_deg: f64,
    ) -> Result<CommandResponse, SeestarError> {
        self.send_command(Command::IscopeStartView(StartViewParams::goto(
            target_name,
            ra_hours,
            dec_deg,
        )))
        .await
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
    /// local system clock) and the observing location, then return the
    /// device's activation status.
    ///
    /// This does **not** perform polar alignment — call
    /// [`start_polar_align`](Self::start_polar_align) separately (EQ mode, clear
    /// sky). For a specific local time-zone string, build the [`SetTimeParams`]
    /// yourself and send [`Command::PiSetTime`] instead of using this helper.
    ///
    /// Returns `true` if the device reports itself **activated**
    /// (`pi_is_verified`). Despite the method's name this is a property of the
    /// device, not of this connection: it says nothing about whether this
    /// socket passed verification. For that see
    /// [`verification`](Self::verification).
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
        // it under `is_verified`); treat anything else as not activated.
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

    /// How the current control connection passes the device's verification
    /// gate, or `None` while it is not connected.
    ///
    /// Negotiated afresh on every (re)connect.
    pub fn verification(&self) -> Option<Verification> {
        match self.link.get() {
            LinkState::Negotiated(mode) => Some(mode),
            LinkState::Unknown | LinkState::KeyRequired => None,
        }
    }

    /// Returns true if the control connection is currently alive.
    pub fn is_control_connected(&self) -> bool {
        self.control_connected.load(Ordering::Acquire)
    }

    /// Returns true if the imaging connection is currently alive.
    pub fn is_imaging_connected(&self) -> bool {
        self.imaging_connected.load(Ordering::Acquire)
    }

    /// Wait for the control connection to be established and verified.
    ///
    /// Returns [`SeestarError::VerificationRequired`] as soon as the device
    /// is found to need an interop key that is not configured.
    pub async fn wait_for_connection(&self, timeout: Duration) -> Result<(), SeestarError> {
        let deadline = tokio::time::Instant::now() + timeout;
        while !self.is_control_connected() {
            if self.link.get() == LinkState::KeyRequired {
                return Err(SeestarError::VerificationRequired);
            }
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
