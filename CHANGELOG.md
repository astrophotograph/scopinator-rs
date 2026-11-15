# Changelog

Notable changes to the scopinator workspace. All crates share one version and are released
together. Versions before 1.0 may break API in a minor release; each entry says when it does.

## [Unreleased]

### Changed

- `scopinator goto` sets the telescope's clock when it is clearly wrong, using this computer's
  local time and time zone. Some models lose their clock when powered off, and a goto does
  nothing without it. `--sync-clock` sets it regardless and no longer switches the telescope to
  UTC; `--no-sync-clock` only warns.
- `scopinator goto` waits for the outcome instead of stopping at "accepted": it reports that the
  mount is slewing, or exits with an error naming the failed stage and code (for example
  `ScopeGoto: mount goto failed (code 501)`), or says the mount never started moving.

### Added

- `ScopeGotoEventData` carries `error` and `code` for a failed slew.

## [0.6.0] - 2026-11-15

An audit of the Seestar client against the [seestar-spec](https://github.com/zmerlynn/seestar-spec) protocol specification and two real telescopes (S50 on firmware 6.70, S30 on firmware 9.75). 0.5.0 could not talk to firmware 9.75 at all and sent several requests no firmware accepts; this release fixes that. **It has breaking changes** — see the upgrade section.

### Fixed

- **Firmware 9.75 works.** The client now negotiates verification at connect: the RSA handshake when an interop key is configured, the `verify` marker on firmware that still accepts it. On 9.75 the marker no longer passes, so a key is required; without one the client reports `SeestarError::VerificationRequired` instead of timing out on every command.
- **Goto works.** `goto_target` does not exist on the device (it returns "method not found"). The new `SeestarClient::goto` slews with `iscope_start_view`, and the mount backend uses it.
- **`set_setting` tells the truth.** Under the marker the device applies a setting and still reports failure; the new `SeestarClient::set_setting` returns a `SettingOutcome` that says what actually happened.
- **One-shot images** are requested on the image socket, where the device serves them (`request_image`, `get_stacked_image`), and zipped replies (9.75) are unpacked. The image frame header is parsed in full.
- **A queued command is no longer sent after its caller gave up**, which could replay a stale command after a reconnect.
- **An unreadable response fails at once** (`SeestarError::MalformedResponse`) instead of leaving the caller to wait out the timeout.
- **Unknown events are kept**, with their name and payload, instead of being dropped; unknown `state` and charger values parse as `Other`.
- The heartbeat follows the connection's verification mode; reconnects have jitter; "too many clients" (Alert 429) is reported as `SeestarError::ConnectionLimit` and backed off.
- `set_sequence_setting` and `get_annotate_result` use forms the device accepts.
- Alpaca: `ConfiguredDevice` reads `UniqueID` as the ASCOM management API sends it.
- The workspace builds without OpenSSL (reqwest uses rustls).

### Added

- **Wide camera** (two-camera models): `SeestarClient::connect_wide`, and `SeestarBackend::second_camera()` for a second streaming camera. On the S30 the wide camera streams only; imaging from it (Pro models) is not implemented.
- **View plans:** typed `ViewPlan`, with `set_view_plan`, `restart_view_plan`, `stop_view_plan`, `clear_view_plan` and `view_plan_state`.
- **Raw commands:** `send_raw` / `send_raw_with` / `send_raw_and_validate` send any method through the negotiated framing, and `scopinator-cli raw` does the same from the shell.
- **Clock sync at connect**, opt-in via `SeestarConfig::clock_sync` (`ClockSync::Off` by default). A Seestar whose clock is unset accepts a goto and does not move; the CLI's `goto --sync-clock` sets it and warns when the clock looks wrong.
- **Device information:** `device_info()`, `verification()`, `firmware_version()` and `product_model()` report what the device said at connect.
- **Typed error codes** (`ErrorCode`), and eleven more typed events (`Version`, `Target`, `Sequence`, `MeridianFlip`, `Settle`, `MountSettle`, `Dither`, `FindStar`, `BalanceSensor`, `AutoExp`, `Station`), plus typed `ViewPlan` and `SecondView`.
- **`EventType::ViewChanged`** on the unified event bus, for a view starting or stopping on either camera.

### Upgrading from 0.5.0

`scopinator-seestar`:

- **Removed commands**, none of which any firmware has: `Command::GotoTarget`, `Command::GetStackedImage` (control port), `GetStackSetting`, `SetStackSetting`. Use `SeestarClient::goto`, `SeestarClient::get_stacked_image` / `request_image`, and `SeestarClient::set_setting`.
- `serialize_command(cmd, id, firmware)` is now `serialize_command(cmd, id, &WireProfile)`. `auth::authenticate` is replaced by `auth::negotiate`.
- `SeestarConfig` has new fields (`clock_sync`); construct it with `..Default::default()`.
- `Command::method()` returns `&str` rather than `&'static str`.
- `SeestarEvent::Unknown` is now `Unknown { name, payload }`; `SeestarEvent::Internal` is gone (such a message parses as `Unknown`).
- Event and state fields that were `String` are typed: `EventState` and `ChargerStatus` gained `Other(String)` (and `EventState::Idle`), and `ScopeTrack.state` is a `TrackState`. Matches on these enums need a wildcard arm.
- `Command::SetViewPlan` takes a typed `ViewPlan` instead of a JSON value; `StopStage` has more variants.
- `DeviceStateResult`, `ViewStateResult` and related response structs were corrected against real replies; some fields changed type or became optional.
- `SeestarError` has new variants: `VerificationRequired`, `ConnectionLimit`, `MalformedResponse`, `ImageRequest`, `ImagePayload`.

`scopinator`:

- `EventType` has a new variant, `ViewChanged`; `EventPayload` has `View`.
- `SeestarMount::abort_slew` now cancels the goto and then leaves the view.
- Reading coordinates returns an error when the device omits them, instead of zeros.

### Notes

- **Interop key:** firmware 9.75 needs `SeestarConfig::interop_key`. Firmware 6.70 works with or without it.
- `ClockSync::Utc` sets the telescope's time zone to UTC, which changes the zone the official app shows. Use `ClockSync::With` to send local time with a zone name.
- Goto coordinates are passed through as given; J2000 versus JNow has not been settled against hardware.
- What was observed on each telescope is in [`docs/protocol/hardware-observations.md`](https://github.com/astrophotograph/scopinator-rs/blob/v0.6.0/docs/protocol/hardware-observations.md).

## [0.5.0] - 2026-06-22

Helpers for auto-focus, scope startup and polar alignment. Nothing breaks.

### Added

- `SeestarClient::start_auto_focus()` and `stop_auto_focus()`. Auto-focus needs polar alignment
  to succeed.
- `SeestarClient::startup(lat, lon)` sets the telescope clock (UTC) and the observing location,
  and returns the `pi_is_verified` status. `SetTimeParams::from_unix_utc(secs)`.
- `Command::StartPolarAlign(PolarAlignParams)` and `SeestarClient::start_polar_align` for
  three-point polar alignment in EQ mode.

## [0.4.0] - 2026-06-22

A typed API for manual movement, checked against a Seestar S50 on firmware 6.70 in EQ mode.

### Added

- `Direction` (`North`, `South`, `East`, `West`) with `Direction::angle()`, the
  `scope_speed_move` angle for each cardinal in EQ mode (West 0°, North 90°, East 180°,
  South 270°). Alt-az mode is not verified.
- `SpeedMoveParams::toward` and `SpeedMoveParams::stop`.
- `SeestarClient::jog` and `stop_jog`: open-loop slewing that needs no polar alignment.
- Examples `probe_move`, `probe_goto` and `probe_escape`.

## [0.3.0] - 2026-06-22

Image-socket commands and frame classification. **Breaking.**

### Changed

- `Command::BeginStreaming` and `Command::StopStreaming` are removed: they are image-socket
  (port 4800) commands, and the control port rejects them. Use
  `SeestarClient::begin_streaming()` or `send_imaging(ImagingCommand::...)`.
- `FrameKind` has a new variant, `Status`.

### Added

- `ImagingCommand` (`BeginStreaming`, `StopStreaming`, `TestConnection`),
  `SeestarClient::send_imaging` and `begin_streaming`.
- `FrameKind::is_image()`.
- Examples `probe_proxy` and `probe_trigger`.

### Fixed

- Every frame that was not a stack was labelled a preview, so heartbeat echoes looked like
  images. Frames are now `Preview`, `Stack` or `Status`.

## [0.2.0] - 2026-06-19

Command and event modelling checked against captured sessions from a Seestar S30 (firmware
7.06) and S50 (firmware 6.70). **Breaking.**

### Changed

- `Command::IscopeStopView` takes `Option<StopViewParams>`; firmware 6.70 sends it with no
  parameters.
- `Command` has new variants (`PlaySound`, `PiStationState`, `SetSequenceSetting`).
- `SeestarConfig` has a new field, `response_timeout`.

### Added

- `SequenceSettingParams`, `PlaySoundParams`.
- `SeestarClient::connect_with_ports_and_config` and a per-command response timeout
  (30 s by default).
- `command_method_names()` and `known_event_names()`.
- A sanitized session corpus with replay tests, property tests, fuzz targets, and a parity
  harness against pyscopinator.

## [0.1.2] - 2026-04-17

### Added

- `send_imaging_command`, for writing to port 4800 ([#3](https://github.com/astrophotograph/scopinator-rs/pull/3)).

## [0.1.1] - 2026-04-16

### Added

- RSA interop-key authentication for Seestar firmware 7.18 and later ([#1](https://github.com/astrophotograph/scopinator-rs/pull/1)).
- More settings ([#2](https://github.com/astrophotograph/scopinator-rs/pull/2)).

[Unreleased]: https://github.com/astrophotograph/scopinator-rs/compare/v0.6.0...HEAD
[0.6.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/astrophotograph/scopinator-rs/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/astrophotograph/scopinator-rs/commits/v0.1.1
