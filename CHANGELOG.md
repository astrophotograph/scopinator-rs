# Changelog

Notable changes to the scopinator workspace. All crates share one version and are released
together. Versions before 1.0 may break API in a minor release; each entry says when it does.

## [Unreleased]

## [0.8.0] - 2026-11-27

Stacking moves to where it belongs. A Seestar's camera is its live view; its stack is its
`LiveStacker`; and the sequencer uses whichever the instrument has. **Breaking**: see the
upgrade notes.

### Breaking

- `SeestarCamera` no longer stacks. `start_exposure` starts the star view (if none is running)
  and its frame stream, and does not send `iscope_start_stack`; `get_image` returns the next
  frame of the live view, not the stack; `image_ready` is true while a view runs;
  `abort_exposure` stops the stream and leaves the view. Starting the view does not move the
  mount, and a view left by a goto keeps its target.
- `CameraStatus::stacked_frames` and `dropped_frames` are gone. `LiveStacker::progress` has
  them.
- `ExecutionContext` (the sequencer's devices) is `#[non_exhaustive]` and has more members:
  observer, live stacker, autofocus, focuser, filter wheel and a frame sink. Build it with
  `ExecutionContext::from_backend(&backend).await?`, or `ExecutionContext::new(mount)` and the
  `with_` methods.
- The sequencer's `StartImaging` honours `count`, which it used to ignore. With a live stacker
  it starts a stack and, given a count, waits for that many frames and stops it. Without one it
  takes single exposures with the camera and hands each to the context's `FrameSink`: `count`
  of them, or in the background until `StopImaging` when there is no count. It used to start
  one exposure and return.
- The sequencer's `GoToTarget` uses the instrument's `Observer` when it has one, and waits until
  the telescope is on target. On a Seestar that includes plate solving: a goto that cannot
  solve now fails the command, where it used to count as arrived once the mount stopped.
- `SequencerCommand` is `#[non_exhaustive]`.
- `scopinator_seestar::event::AutoFocusEventData` has `error` and `code` members, so code that
  builds one by hand needs them.

### Added

- A Seestar has a focuser and a filter wheel as a `Backend`: `SeestarFocuser` (absolute, in
  motor steps; `halt` stops a move; a position beyond the travel is `OutOfRange`, where the
  telescope itself would run to the end) and `SeestarFilterWheel` (the telescope's own names,
  `dark`, `IRCUT` and `LP` on an S30, at positions counted from zero). Their ids end in
  `:focuser` and `:filterwheel`. `Backend::focuser()` and `filter_wheel()` returned `None` for
  a Seestar before.
- `Switch`, a device trait for outputs that are switched or set (dew heaters, power ports, a
  flat panel's brightness): `switches`, `get`, `set`, and `get_bool` / `set_bool` on top of
  them, with `SwitchInfo` in `scopinator-types`. `Backend::switch()` hands one out; it is `None`
  unless a backend provides it.
- A Seestar's dew heater is switch 0 of `SeestarSwitch` (`seestar::DEW_HEATER`), on or off.
- Sequencer commands `AutoFocus`, `SetFilter` (by `position` or `name`), `Park` and `Unpark`.
- `FrameSink`, for receiving the frames of a camera imaging run in a sequence.
- `seestar_verify` has `camera`, `focuser`, `wheel` and `switch` stages.
- A Seestar's `FilterChanged` event carries the position the wheel arrived at
  (`EventPayload::FilterPosition`, counted from zero, no name) and comes from the
  `:filterwheel` device. It had no payload.

### Fixed

- A failed Seestar autofocus says why when the telescope does: `autofocus failed: no star is
  detected (code 279)`, where it was only `autofocus failed`.
- After a Seestar goto failed, `Observer::acquisition` read `Centring` and `Mount::is_slewing`
  read true for about two seconds, until the failed goto left the telescope's view state. Both
  now read as over once the failure has been reported.

### Verified

- A Seestar takes and reports coordinates in the equinox of date, as the library has assumed
  since 0.7.0: checked on an S30 (firmware 9.75) under the sky. A goto that plate-solves, an
  autofocus that completes and a stack that returns an image were seen there too. See
  `docs/protocol/hardware-observations.md`.

### Changed

- Dependencies no crate used are gone: `socket2` from the workspace, `scopinator-types` from
  `scopinator-alpaca` and `scopinator-indi`, `serde_json`, `tokio` and `tracing` from
  `scopinator-indi`. `serde_json` is a development dependency of `scopinator-types`.

### Upgrading from 0.7

- To stack on a Seestar, use `backend.live_stacker()`: `start_stacking`, `progress`,
  `stacked_image`, `stop_stacking`. Code that called `camera.start_exposure` to start a stack
  and `camera.get_image` to fetch it now gets the live view and one raw frame of it.
- Replace `ExecutionContext { mount, camera }` with
  `ExecutionContext::new(mount).with_camera(camera)`, or `from_backend`.
- A sequence that relied on `StartImaging` returning at once should leave out `count`. With a
  count, the command now runs until the frames are in.

## [0.7.1] - 2026-11-26

Fixes found by running the smart-telescope traits against a Seestar S30 (firmware 9.75) indoors,
where every plate solve fails. Nothing breaks; all the changes are in `scopinator`. Whether a
Seestar takes goto coordinates in the equinox of date is still unconfirmed: that needs stars.

### Fixed

- A Seestar goto that never plate-solves is reported as failed. The telescope tries for about
  six minutes, says so in one event, and goes on showing its live view as it does after a
  success; `Observer::acquisition` answered `OnTarget` and `wait_for_acquisition` returned `Ok`.
  They now return the failure once ("goto failed at AutoGoto: solve failed (code 251)"), and
  `acquisition` is `Idle` after it, not `OnTarget`. `Mount::is_slewing` reports it the same way.
- A plate solve or autofocus that fails in the middle of a Seestar goto no longer ends it. The
  telescope retries with a longer exposure; `acquisition` and `is_slewing` reported a failed
  goto, or `Idle`, while it was still working.
- `SeestarMount::is_parked` is true only when the mount has stopped. The telescope reports its
  arm closed about 35 seconds before a park finishes, so `wait_for_park` returned early.
  `get_mount_status` says `Slewing`, not `Parked`, while a park or a goto from the park position
  is under way.
- `SeestarStacker::progress` says `Stacking` while the telescope prepares a stack (it focuses
  first, for over a minute). It said `Idle`, so `wait_for_frames` gave up after a few seconds.
- `Observer::acquisition` says `Slewing`, not `Centring`, in the first seconds of a Seestar
  goto, before the mount moves.

### Added

- `crates/scopinator/examples/seestar_verify.rs` runs the smart-telescope traits against a real
  Seestar stage by stage (readiness, goto, plate solve, autofocus, stack, park) and prints what
  the telescope and the traits say at each change. **It moves the telescope.**

## [0.7.0] - 2026-11-26

One set of device traits for any telescope, and a second backend to prove it: an ASCOM Alpaca server is now a `Backend` beside the Seestar, with real mount, camera, focuser and filter wheel implementations, tested nightly against the ASCOM Omni Simulators. The traits for smart telescopes (`Observer`, `LiveStacker`, `AutoFocus`, `PlateSolver`) are here too, with Seestar implementations. **It has breaking changes** — see the upgrade section.

### Breaking

- `ExposureSettings` has `frame_type: FrameType` (light, dark, bias, flat) in place of
  `light: bool`, and new `roi` and `readout_mode` fields. It is `#[non_exhaustive]`: build it
  with `ExposureSettings::new(seconds)` and the `with_` methods.
- `CameraCapabilities::pixel_size_um` is split into `pixel_size_x_um` and `pixel_size_y_um`.
- The Seestar cameras refuse an exposure that asks for a dark, bias or flat frame or for a
  region of interest, with `NotSupported`. They used to ignore the request and start a view.
- `UnifiedEvent` has a `source` field (the device of the instrument the event is about) and is
  `#[non_exhaustive]`: build one with `UnifiedEvent::new`. `EventPayload::Error` has a `kind`
  field.
- A Seestar goto that fails is reported as `EventType::SlewFailed`, not `EventType::Error`.
- `Focuser::get_position` is now `focuser_position`, returning a `FocuserPosition`;
  `FilterWheel::get_position` and `set_position` are now `filter_position` (a `FilterPosition`,
  or `None` while the wheel turns) and `set_filter_position`. Filter positions count from zero
  for every backend. Both traits have a required `capabilities()`, with `FocuserCapabilities`
  and `FilterWheelCapabilities`. `FocuserPosition::max_position` is an `Option`.
- `ImageData` describes its pixels with `format: PixelFormat` (`Mono8`, `Mono16`, `Bayer8`,
  `Bayer16`, `Rgb8`, `Rgb16`, `Jpeg`) in place of `bit_depth`, `is_color` and `bayer_pattern`,
  which could not tell an RGB image from a Bayer mosaic. It also has `metadata: FrameMetadata`,
  `expected_len()` and `has_expected_len()`, and is built with `ImageData::new`.
- `SeestarCamera::get_image` returns a JPEG frame as `PixelFormat::Jpeg` instead of failing, and
  fails on a frame whose size does not match its header.
- `DeviceManager` holds any kind of telescope. `add_seestar` and `add_seestar_with_config`
  return the backend's id; `seestar_mount` and `seestar_camera` are gone: use
  `manager.get(&id)` and the `Backend` it returns. `list_backends` returns `DeviceId`s and is no
  longer async, and `disconnect_all` returns a `Result` and empties the manager.
- The device traits have required state queries for operations that take time:
  `Mount::is_slewing`, `Mount::is_parked`, `Camera::image_ready`, `Focuser::is_moving` and
  `FilterWheel::is_moving`. The methods that start those operations are now documented to
  return when the device accepts the request, for every backend; see `docs/device-contract.md`.
- `SeestarMount::is_tracking` returns an error when the device state has no `mount.tracking`
  member, instead of `false`.
- `Device` has two new required methods, `id()` and `info()`. A Seestar's mount and cameras now
  have their own ids under the telescope's (`seestar:<ip>:mount`, `seestar:<ip>:camera`,
  `seestar:<ip>:wide`); the mount and main camera used to report the telescope's id as their
  name. Events still carry the telescope's id.
- `MountCapabilities`, `CameraCapabilities`, `DeviceStatus`, `MountStatus`, `CameraStatus` and
  `ScopinatorError` are `#[non_exhaustive]`, so that fields and variants can be added for other
  telescope backends without another breaking release. Code outside the `scopinator` crate can
  no longer build the structs with a literal: start from `Default::default()` and set fields. A
  `match` on `ScopinatorError` needs a wildcard arm.

### Changed

- `scopinator goto --dec -5.4` works. A negative declination was taken for an option and
  rejected unless written `--dec=-5.4`.
- `AlpacaMount::move_axis` keeps to the rates the mount offers. A mount can list its axis rates
  as separate ranges; a speed that fell between two of them was refused by the mount.
- `SeestarMount::slew_to_coordinates` converts its target to the equinox of date before sending
  it, and `get_coordinates` labels the position it returns `Epoch::JNow`. The epoch of a
  `Coordinates` used to be ignored, so a J2000 position was sent as it was, about 0.37 degrees
  off in 2026. That a Seestar works in the equinox of date comes from the protocol
  specification and has not yet been confirmed on a telescope. `MountCapabilities` has a new
  `equatorial_system` field. `scopinator goto` is unchanged: it still sends the coordinates it
  is given.
- The sequencer's `GoToTarget` waits for the mount to arrive before the next command runs. It
  used to return as soon as the mount accepted the slew, so a following `StartImaging` began
  during the slew. It has two new optional fields, `timeout_seconds` (default 300) and
  `settle_seconds`; code that builds the variant must add them, JSON without them still loads.
- `SeestarMount::get_mount_status` reports `Slewing` and `Parked`. It could only say `Tracking`
  or `Idle` before.
- `scopinator goto` sets the telescope's clock when it is clearly wrong, using this computer's
  local time and time zone. Some models lose their clock when powered off, and a goto does
  nothing without it. `--sync-clock` sets it regardless and no longer switches the telescope to
  UTC; `--no-sync-clock` only warns.
- `scopinator goto` waits for the outcome instead of stopping at "accepted": it reports that the
  mount is slewing, or exits with an error naming the failed stage and code (for example
  `ScopeGoto: mount goto failed (code 501)`), or says the mount never started moving.

### Added

- Traits for smart telescopes, which a `Backend` hands out when its instrument has them:
  `Observer` (readiness, and finding and centring a `Target`), `LiveStacker`, `AutoFocus` and
  `PlateSolver`, with their types in `scopinator-types`, wait helpers (`wait_for_ready`,
  `wait_for_acquisition`, `wait_for_frames`, `wait_for_autofocus`, `wait_for_solution`) and
  the `ScopinatorError::NotReady` kind.
- The Seestar backend implements them: `SeestarStacker` (`LiveStacker`: sub-exposure, LP
  filter, gain and restart applied before `iscope_start_stack`; progress from the view state;
  the stacked image), `SeestarObserver` (`Observer`: clock and site readiness, sky,
  solar-system and scenery targets, acquisition stages), `SeestarAutoFocus` and
  `SeestarPlateSolver`. `SeestarCamera` is unchanged for now and still starts a stack.
- `scopinator` (the command-line tool) works with Alpaca mounts: `--host alpaca://HOST:PORT`,
  or `--backend alpaca` with `--port`, on `status`, `goto`, `park` and the new `unpark`.
  `goto` on an Alpaca mount takes J2000 coordinates (`--jnow` for the equinox of date), turns
  tracking on if needed, waits for arrival (`--timeout`) and reports where the mount ended up.
  `discover` lists Alpaca servers as well as Seestars, with addresses in the form `--host`
  takes. Commands for a Seestar behave as before.
- `tools/alpaca-sim.sh` (`just alpaca-sim-test`) runs the Alpaca backend against the ASCOM
  Alpaca Omni Simulators: it downloads a pinned release, starts it on localhost in its default
  state, runs `crates/scopinator/tests/alpaca_omnisim.rs` and stops it. It runs nightly in CI.
  See `docs/alpaca.md`.
- `scopinator::backend::alpaca::AlpacaBackend`: a whole Alpaca server as a `Backend`. It asks
  the server what devices it has, connects its mount, cameras, focuser and filter wheel
  (`AlpacaConfig` chooses among several), and polls them to put the same events on the bus that
  a Seestar reports by itself: slews, parking, tracking, exposures, focuser and filter moves,
  and the server going away and coming back. `DeviceManager::add_alpaca` connects one;
  `AlpacaDiscoverer` makes Alpaca servers show up in `discover_all`.
- `scopinator::backend::alpaca::AlpacaCameraDevice`: an Alpaca camera as a neutral `Camera`,
  taking real single exposures. It checks the settings against the ranges the camera reports,
  applies binning, subframe, gain and offset, and returns the image as 16-bit samples in rows
  with its `PixelFormat` and metadata. `AlpacaFocuserDevice` and `AlpacaFilterWheelDevice` are
  the first implementations of the neutral `Focuser` and `FilterWheel`.
- `scopinator::backend::alpaca::AlpacaMount` (feature `alpaca`): an ASCOM Alpaca telescope as a
  neutral `Mount`. It reads the mount's capabilities at connect, converts slew and sync targets
  to the equatorial system the mount works in, and maps Alpaca's error numbers onto
  `ScopinatorError` kinds (`Parked`, `NotSupported`, `OutOfRange`, ...).
- `scopinator-alpaca` has typed clients for the four Alpaca device types a telescope setup is
  made of: `AlpacaTelescope`, `AlpacaCamera`, `AlpacaFocuser` and `AlpacaFilterWheel`, over the
  existing raw property client. `AlpacaCamera::image_bytes` fetches an image in the binary
  ImageBytes encoding, and `AlpacaImage::to_row_major_u16` reorders it from Alpaca's
  column-by-column layout into rows. `AlpacaError::NotImplemented` separates "the device does
  not have this" (error 0x400) from other device errors.
- `CameraCapabilities` describes the sensor: type, gain, offset and exposure ranges, maximum
  binning, full well, electrons per ADU, and whether the camera has a region of interest and a
  cooler. `Camera` has `set_cooler`, `set_target_temperature` and `cooler_power`, and
  `CameraStatus` reports the cooler. New types `FrameType`, `Roi` and `SensorType`.
- Event types `SlewFailed`, `Parked`, `Unparked`, `PierSideChanged`, `ImageReady`,
  `FocuserMoveCompleted`, `FilterChanged` and `OperationProgress`, and payloads
  `FocuserPosition`, `FilterPosition` and `Progress`. The Seestar backend emits `Parked`,
  `SlewFailed`, `FocuserMoveCompleted`, `FilterChanged` and progress for autofocus and plate
  solving.
- `scopinator::discovery`: a `Discoverer` trait, `discover_all` to search for every kind of
  instrument at once, and `DeviceManager::connect` to connect to what was found.
  `SeestarDiscoverer` wraps the Seestar's UDP discovery.
- More optional `Mount` members, answering `NotSupported` unless a backend provides them:
  `get_alt_az`, `slew_to_alt_az`, `set_tracking_rate`, `side_of_pier`, `find_home`,
  `is_at_home`, `pulse_guide` and `alignment_mode`, with matching capability flags,
  `GuideDirection`, `AlignmentMode` and a checked `AltAzCoordinates::new`. The Seestar mount
  provides `get_alt_az` and `alignment_mode`.
- `Mount::get_site`, `set_site`, `get_utc` and `set_utc`, with `SiteLocation` and the
  capability flags `can_set_site` and `can_set_time`. On a Seestar, `set_utc` keeps the
  telescope's time zone: it sends local time in the zone the telescope already has. This adds
  the `chrono-tz` dependency to the `seestar` feature.
- `Mount::sync_to_coordinates`, `unpark`, `move_axis` and `stop_motion`, which answer
  `NotSupported` unless a backend provides them, with `MountAxis` and
  `MountCapabilities::can_unpark`. The Seestar mount provides sync (`scope_sync`) and axis
  motion (`scope_speed_move`); it has no unpark request.
- The `Backend` trait: one connected instrument, handing out its mount, cameras, focuser and
  filter wheel as the neutral device traits. `SeestarBackend` implements it, and
  `DeviceManager::add` takes any implementation.
- `wait_for_slew`, `wait_for_park`, `wait_for_image`, `wait_for_focuser` and
  `wait_for_filter_wheel`, with `WaitOptions` (poll interval, timeout, settle time), wait for an
  operation to finish on any backend.
- `Coordinates::to_epoch` converts between J2000 and the equinox of date (precession only).
- `DeviceKind` and `DeviceInfo` (id, kind, name, model, firmware, driver) in `scopinator-types`.
- `ScopinatorError` has kinds a caller can match on for any backend: `Busy`, `Parked`,
  `InvalidState`, `OutOfRange` and `ConnectionLost`, plus `Alpaca` and `Indi` variants behind
  those features and `is_not_supported()`.
- `ScopeGotoEventData` carries `error` and `code` for a failed slew.

### Upgrading from 0.6.0

Nearly all of the breaking changes are in `scopinator` and `scopinator-types`. `scopinator-indi` is unchanged. The other two crates have one small break each:

- `scopinator-alpaca`: `AlpacaError` has a new variant, `NotImplemented`, so an exhaustive `match` needs another arm. Alpaca error 0x400 now arrives as `NotImplemented` rather than `Device { code: 0x400, .. }`.
- `scopinator-seestar`: `ScopeGotoEventData` has two new fields, `error` and `code`, which code building one with a literal must add.

`scopinator` and `scopinator-types`:

- **`DeviceManager`:** `seestar_mount(name)` and `seestar_camera(name)` are gone. Keep the id that `add_seestar` returns and ask for the backend: `manager.get(&id)`, then `backend.mount()`, `backend.cameras()` and so on. `list_backends()` is no longer async and returns `DeviceId`s; `disconnect_all()` returns a `Result`.
- **Structs built with a literal:** `ExposureSettings`, `UnifiedEvent`, `ImageData`, `MountCapabilities`, `CameraCapabilities`, `DeviceStatus`, `MountStatus` and `CameraStatus` are `#[non_exhaustive]`. Use `ExposureSettings::new(seconds)` with the `with_` methods, `UnifiedEvent::new`, `ImageData::new`, and `Default::default()` plus field assignment for the rest. A `match` on `ScopinatorError`, `EventType` or `EventPayload` needs a wildcard arm.
- **`ExposureSettings`:** `light: bool` is `frame_type: FrameType`. A Seestar camera now answers `NotSupported` to a dark, bias or flat frame or a region of interest, where it used to ignore the request.
- **`ImageData`:** read `format: PixelFormat` in place of `bit_depth`, `is_color` and `bayer_pattern`. `SeestarCamera::get_image` can return `PixelFormat::Jpeg`; check the format before treating the bytes as samples.
- **`CameraCapabilities`:** `pixel_size_um` is `pixel_size_x_um` and `pixel_size_y_um`.
- **Focuser and filter wheel:** `Focuser::get_position` is `focuser_position` (a `FocuserPosition`); `FilterWheel::get_position` / `set_position` are `filter_position` / `set_filter_position`, counting from zero. Implementations must add `capabilities()`.
- **Implementing the device traits:** `Device` needs `id()` and `info()`; `Mount` needs `is_slewing` and `is_parked`; `Camera` needs `image_ready`; `Focuser` and `FilterWheel` need `is_moving`. `docs/device-contract.md` says what each method promises.
- **Events:** a failed Seestar goto arrives as `EventType::SlewFailed`, not `EventType::Error`. `UnifiedEvent` has a `source` field and `EventPayload::Error` a `kind` field.
- **Device ids:** a Seestar's mount and cameras have their own ids (`seestar:<ip>:mount`, `seestar:<ip>:camera`, `seestar:<ip>:wide`). Events still carry the telescope's id.
- **`SeestarMount::is_tracking`** returns an error, not `false`, when the device does not report tracking.
- **Sequencer:** code that builds `GoToTarget` must add `timeout_seconds` and `settle_seconds` (both `Option`). The command now waits for the mount to arrive. Saved JSON loads unchanged.
- **Seestar goto coordinates** through the `Mount` trait are now converted to the equinox of date: see the notes.

### Notes

- **Not yet confirmed on a telescope:** that a Seestar takes goto coordinates in the equinox of date (JNow) comes from the protocol specification. `SeestarMount::slew_to_coordinates` converts a J2000 target on that basis, a shift of about 0.37 degrees in 2026. If your gotos land that far off after upgrading, pass `Coordinates` already marked `Epoch::JNow` to send them unchanged, and please report it. `SeestarClient::goto` and `scopinator goto` send what they are given, as in 0.6.0.
- Also written from the specification and recorded sessions, without a run on hardware in this release: `Slewing` and `Parked` in `SeestarMount::get_mount_status`, and `SeestarStacker`, `SeestarObserver`, `SeestarAutoFocus` and `SeestarPlateSolver`.
- The Alpaca backend is tested against the ASCOM Alpaca Omni Simulators, not against a physical mount or camera. See `docs/alpaca.md`.
- `SeestarCamera` still starts a stack for an exposure. That changes in 0.8.0, where stacking moves to `SeestarStacker`.

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

[Unreleased]: https://github.com/astrophotograph/scopinator-rs/compare/v0.8.0...HEAD
[0.8.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.7.1...v0.8.0
[0.7.1]: https://github.com/astrophotograph/scopinator-rs/compare/v0.7.0...v0.7.1
[0.7.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.6.0...v0.7.0
[0.6.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.5.0...v0.6.0
[0.5.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.4.0...v0.5.0
[0.4.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.3.0...v0.4.0
[0.3.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.2.0...v0.3.0
[0.2.0]: https://github.com/astrophotograph/scopinator-rs/compare/v0.1.2...v0.2.0
[0.1.2]: https://github.com/astrophotograph/scopinator-rs/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/astrophotograph/scopinator-rs/commits/v0.1.1
