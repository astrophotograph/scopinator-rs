# Hardware observations

What a real Seestar did when asked. Where pyscopinator, the protocol specification
([zmerlynn/seestar-spec](https://github.com/zmerlynn/seestar-spec), commit `14ac005`, which
describes firmware 9.31 and 9.75) and hardware disagree, **hardware wins, then the spec, then
pyscopinator**. Byte parity with pyscopinator is kept as a drift detector, not as the definition of
correct: the parity corpus pinned several requests the device rejects.

Every entry is labelled OBSERVED with the date, model and firmware, following the spec's own
labelling. Nothing here is inferred from another firmware version.

Two devices have been observed. The sections up to "Image socket" are the first; the second has its
own section, [Seestar S30, firmware 9.75](#seestar-s30-firmware-975).

**Device for the entries below, unless a section says otherwise:** Seestar S50, firmware 6.70
(`firmware_ver_int` 2670), on the LAN, indoors, 2026-11-14. "Marker" means the `verify` marker in `params`. "Verified socket" means a
control connection that completed `verify_client` with an interop key.

## Verification gate (port 4700)

| Observation | Result |
|---|---|
| Request with no marker on an unverified socket (`get_device_state`, `test_connection`, `pi_is_verified`, `get_view_state`, `set_setting {}`, an unknown method) | Silently consumed: no reply, no close |
| Same requests with `["verify"]`, `{"verify": true}` or the bare string `"verify"` | Served |
| `get_verify_str` on an unverified socket | Code 0, a 32-character challenge. The handshake exists on 6.70; it does not start at 7.18 |
| `verify_client {sign, data}` with an RSA-1024 PKCS#1 v1.5 / SHA-1 signature over the challenge | Code 0, `result: 0` |
| Requests on the verified socket with no marker | Served (`get_device_state`, `test_connection`, `pi_get_time`, `get_setting`, `set_setting`, getters) |
| `verify_client` with a corrupted signature | 207 "fail to operate". The socket stays open and unverified; a fresh `get_verify_str` and a good signature on the same socket then succeed |
| A second socket opened while the first is verified | Still unverified: verification is per socket |
| `pi_is_verified` | `true`; this is device activation, not socket verification (as the spec says) |
| `Version` greeting after connect | None within 1.5 s |
| A control socket with no traffic from the client | Closed by the device after roughly 10 s |

## Marker placement

Twelve parameterless getters were sent with each marker form in marker mode.

| Method | `["verify"]` | `{"verify": true}` | `"verify"` |
|---|---|---|---|
| `get_wheel_position`, `get_wheel_setting`, `get_focuser_position`, `scope_get_horiz_coord`, `pi_station_state`, `get_stack_info`, `get_sequence_setting` | code 0 | code 0 | code 0 |
| `get_solve_result`, `get_last_solve_result` | 215 "no solve data" | 215 | 215 (expected indoors) |
| `get_disk_volume` | 255 "file not exist" | **code 0** | 255 |
| `get_annotate_result` | 104 "expected int param" | 207 "fail to operate" (same as no params on a verified socket: nothing annotated) | 104 |
| `get_camera_info` | 104 | 104 | 104 |

`get_camera_info` with no params on a verified socket returns the open camera (chip 1080×1920,
2.9 µm, colour, `GR`). In marker mode `[0, "verify"]` returns the all-zero record for ordinal 0;
there is no marked form for "the open camera".

## `set_setting`

The firmware applies members in wire order, does not roll back, and treats `verify` as an unknown
member.

| Request params | Socket | Reply | Effect |
|---|---|---|---|
| `{"verify": true}` | unverified | 109 "unexpected param" | none |
| `{"verify": true, "stack_after_goto": true}` (marker first) | unverified | 109 | **not applied** |
| `{"stack_after_goto": true, "verify": true}` (marker last) | unverified | 109 | **applied** (readback, `Setting` event) |
| `[{"stack_after_goto": false}, "verify"]` | unverified | 107 "expected object param" | none |
| `{"stack_after_goto": true}` | verified | code 0, `result: 0` | applied |
| `{"stack_after_goto": false, "verify": true}`, either order | verified | 109 | the marker is rejected here too |

So with the marker a `set_setting` can only take effect if the marker is the last member, and it
always reports failure. Members that sort after `verify` (`viewplan_gohome`, `wide_*`) are never
reached if the object is written in sorted order.

## Methods that do not exist

All return 103 "method not found" on port 4700, marked and on a verified socket:

- `goto_target`
- `get_stacked_img` (it is an image-socket method)
- `get_annotated_result` (the method is `get_annotate_result`)
- `get_stack_setting`, `set_stack_setting`

These exist and answer: `get_annotate_result`, `get_sequence_setting`, `get_stack_info`,
`get_user_location`, `scope_get_ra_dec`, `get_wheel_state`, `get_camera_state`.

## `set_sequence_setting`

Writing the current `group_name` back:

| Params | Socket | Reply |
|---|---|---|
| `{"group_name": ...}` | verified | code 0 |
| `[{"group_name": ...}]` | verified | code 0 |
| `[[{"group_name": ...}]]` (the shape in the 7.06 capture) | verified | 107 "expected object param" |
| `[{"group_name": ...}, "verify"]` | unverified | code 0 |
| `{"group_name": ..., "verify": true}` | unverified | code 0 |

`get_sequence_setting` returns `delay_first`, `delay_between_frame`, `group_by_slot`,
`merid_flip`, `group_name`, `park_mount`, `shutdown_pi`, `close_cooler`, `left_size_m`,
`left_time_sec`. The method is not 7.06-only.

## Goto

`iscope_start_view {"mode": "star", "target_ra_dec": [20.69, 45.28], "target_name": "Deneb",
"lp_filter": false}` (RA in hours):

- reply code 0, then `View` working, `AutoGoto` start/working, `ScopeGoto` working about once a
  second; the mount moved (declination −90° to −41° in 12 s) and `mount.move_type` was
  `ScopeGoto`.
- `iscope_stop_view {"stage": "AutoGoto"}` → `ScopeGoto` fail 207, `AutoGotoStep` fail 501
  "mount goto failed", `AutoGoto` fail 253 "aborted", then cancel. The view **stayed** in star mode
  at stage `ContinuousExposure`; a second, parameterless `iscope_stop_view` ended it.
- `scope_park` → `ScopeHome` working; the mount returned to `close: true`.
- `iscope_start_view {"mode": "star"}` with no target did not open the arm.

## `pi_set_time`

`pi_set_time [{"year", "mon", "day", "hour", "min", "sec", "time_zone": "UTC"}, "verify"]` in
marker mode: code 0; `pi_get_time` then returned the new UTC time and `time_zone` "UTC" (the zone
had been "America/Chicago"). Sending local time with the original zone name put it back. So a
non-empty `time_zone` replaces the device's zone, as the spec says.

## View plan

The device held no plan beforehand (`iscope_get_app_state` had no `ViewPlan` member). In marker mode:

| Step | Result |
|---|---|
| `set_view_plan` with `plan_name`, `plan_target_af: false`, `viewplan_gohome: false` and one `tele` target starting at the current local minute, 5 minutes long | Code 0, `result: 0`. Events: `Setting`, `ViewPlan` working, `Target` working (with the `target_id` sent), `View` working in `star` mode, `AutoGoto`, `ScopeGoto`; `mount.move_type` was `ScopeGoto` |
| `iscope_get_app_state` while running | `ViewPlan` with `state` working, `stage` "Target", and the plan with the target's `state` working |
| `stop_func {"name": "ViewPlan"}` | Code 0. The goto was cancelled (`ScopeGoto` fail and cancel, `AutoGoto` fail and cancel), the view ended (`View` cancel, mode `none`), then `Target` cancel and `ViewPlan` cancel. **The plan stayed on the device**, with the target's state `fail` |
| `clear_view_plan` | Code 0. **The device then held no plan** |

So on 6.70 `stop_func` stops a plan and keeps it, and `clear_view_plan` removes it. Stopping a plan
also leaves the view, unlike cancelling a single goto.

## Image socket (port 4800)

| Observation | Result |
|---|---|
| `test_connection` with no marker | Answered: this port has no verification gate |
| Reply header | magic `03 C3`, version 2, header length 80, payload length 17, payload `server connected!` |
| Header byte 15 | Low byte of the request id: id 77 → 77, id 300 → 44. It is not a frame type |
| `get_current_img {"compress": false, "img_type": "Raw"}` | dataType 2, imgType 1 (2 bytes/pixel), 1080×1920, canDebayer 1, 4 147 200 bytes, request id echoed |
| `get_current_img {"compress": true, "img_type": "Raw"}` | The same uncompressed frame (dataType 2). A zip reply was not obtained on this firmware |
| `get_current_img` while parked | Still returns the last preview frame |
| `get_stacked_img` with nothing stacked | Text error (dataType 1): `there is no image now` |

## Seestar S30, firmware 9.75

Seestar S30 (two cameras), firmware 9.75 (`firmware_ver_int` 2975), on the LAN, parked, 2026-11-14.
This is one of the two firmware versions the specification describes. Writes made: `set_setting`
with `stack_after_goto` (toggled and restored), `pi_set_time` (the clock was unset, see Goto), and
one goto that was cancelled and followed by a park.

### Verification gate

| Observation | Result |
|---|---|
| Unverified socket: `test_connection` unmarked, `test_connection ["verify"]`, `get_device_state {"verify": true}` | All silently consumed. **The marker no longer passes the gate** |
| `get_verify_str` on an unverified socket | Code 0, a 32-character challenge (base64 alphabet) |
| `verify_client {sign, data}`, same key and signature scheme as on 6.70 | Code 0, `result: {"key_envelope": {cipher, data, kdf, version, wrap}}` instead of `0` |
| Requests on the verified socket with no marker | Served, without using the key envelope or any encryption |
| `get_device_state ["verify"]` and `{"verify": true}` on the verified socket | Code 0: the marker is tolerated on a getter |
| `set_setting {"stack_after_goto": false, "verify": true}` on the verified socket | 109 "unexpected param", as on 6.70 |
| `set_setting {"stack_after_goto": ...}` and `set_setting {}` on the verified socket | Code 0, `result: 0` |
| `Version` greeting after connect | None within 1.5 s |

So on this firmware a client without the key cannot do anything on port 4700, and a client with it
needs no marker anywhere.

### Methods

- 103 "method not found", as on 6.70: `goto_target`, `get_stacked_img` (port 4700),
  `get_annotated_result`, `get_stack_setting`, `set_stack_setting`.
- With no params on the verified socket: `get_camera_info` code 0 (chip 1080×1920, 2.9 µm);
  `get_disk_volume` code 0; `get_annotate_result` 207 (nothing annotated); `get_sequence_setting`,
  `get_stack_info`, `get_wheel_position`, `get_focuser_position`, `scope_get_equ_coord`,
  `pi_get_time`, `get_view_state` (`{}` when parked), `iscope_get_app_state` all code 0.
- `get_device_state {"keys": ["client"]}` returns `{}`, as the spec warns.

### Device state

- `device` has `product_model` "Seestar S30", `user_product_model`, `focal_len` 150, `fnumber` 5 and
  ten `can_*` flags: `can_4k_quality`, `can_comet_post_stack`, `can_dark_correction` true;
  `can_dual_capture`, `can_motion_monitor`, `can_roi_smart_follow`, `can_star_mode_sel_cam`,
  `can_wide_cam_af`, `can_wide_cam_roi`, `can_wide_mosaic_8k` false. There is no
  `firmware_platform` or `main_version` member.
- `camera`: chip 1080×1920, 2.9 µm, focal length 150 mm, `debayer_pattern` "GR".
- `second_camera` (the wide camera): chip **1920×1080** (landscape), 2.7 µm, focal length 14 mm,
  `debayer_pattern` "RG".
- `get_setting` has 48 members, including `wide_cam`, `wide_4k`, `second_camera`, `sky_replay`,
  `can_encrypt`, `enable_dark_library`, `guest_mode` and, still, `expert_mode`.

### Image sockets

| Observation | Result |
|---|---|
| Port 4800 and port 4804 (wide camera), `test_connection` with no marker and no verification | Both answer `server connected!`; magic `03 C3`, version 2, header length 80, byte 15 = request id |
| `get_stacked_img` on 4800 and on 4804, nothing stacked | Text error (dataType 1) `there is no image now` on both |
| `get_current_img` while parked with no view | `there is no image now` (the S50 on 6.70 returned its last frame) |

### Goto and the device clock

The device clock read 2020-10-10 when first seen (never set).

| Step | Result |
|---|---|
| `iscope_start_view {"mode": "star", "target_ra_dec": [20.69, 45.28], "target_name": "Deneb", "lp_filter": false}` with the clock unset | Code 0, `View` working, `ContinuousExposure` working. **No `AutoGoto` event, no slew, no error**: the mount stayed parked |
| `pi_set_time [{year, mon, day, hour, min, sec, time_zone}]` with the current local time and the zone the device already had | Code 0; `pi_get_time` then read the new time |
| The same `iscope_start_view` again | `ScopeTrack` state `off` with error "below horizon" (270) for the parked position, then `AutoGoto` start/working and `ScopeGoto` working with `cur_ra_dec` and `dist_deg`; the mount moved (declination −90° to −18° in 16 s) |
| `iscope_stop_view {"stage": "AutoGoto"}` | `ScopeGoto` fail 207, `AutoGotoStep` fail 501, `AutoGoto` fail 253 "aborted" and cancel; the view continued in `ContinuousExposure`, as on 6.70 |
| `iscope_stop_view` (no stage), then `scope_park` | View cancelled; `ScopeHome` working then complete after 23 s, `close: true` |

A client must set the clock before a goto. A wrong clock makes the device treat the target as
unreachable, and the reply to the request never says so.

Observed again on 2026-11-15, same telescope:

- **The clock does not survive a power cycle.** It was set on 2026-11-14 and read 2020-10-10
  again the next day, after the telescope had been off overnight.
- **The failure is not always silent, and is never in the reply.** Three gotos with the clock
  wrong, all answered with code 0:
  1. (2026-11-14) no `AutoGoto` event, nothing in the view state; the mount stayed parked.
  2. About 9 s after the request, `get_view_state` showed `View` > `AutoGoto` working >
     `ScopeGoto` with `state: fail`, code 501 "mount goto failed"; the mount stayed parked.
  3. (clock deliberately set back to 2020) nothing failed within 12 s and `mount.move_type` was
     never `ScopeGoto`, but `mount.close` was `false` afterwards.
- With the clock set, the same request slewed and `tracking` came on: `mount.move_type` was
  `ScopeGoto` within 6 s, and the mount reported RA 20.6917 h, Dec 45.279° for a request of
  RA 20.69 h, Dec 45.28°.

So a client cannot rely on any one signal. `scopinator goto` sets the clock when it is clearly
wrong, then waits for the mount to start moving or for a failed stage, from events and by polling.

During the goto, on port 4800:

| Request | Reply |
|---|---|
| `get_current_img {"compress": false, "img_type": "Raw"}` | dataType 2, imgType 1, 1080×1920, canDebayer 1, 4 147 200 bytes |
| `get_current_img {"compress": true, "img_type": "Raw"}` | dataType **3**: a zip of 1 959 646 bytes with one entry `raw_data` of 4 147 200 bytes |
| `get_current_img` with no params | The same zip reply: compression is the default here |
| The same three requests on port 4804 (wide camera) | `there is no image now` while the main camera has the view |

`ScopeTrack` uses the state strings `on` / `off`, which are outside the usual
start/working/complete set.

### A goto that never plate-solves (2026-11-26)

Run with `crates/scopinator/examples/seestar_verify.rs`. The telescope was parked, in EQ mode
(`mount.equ_mode: true`), with its clock at 2020 and its site set. It was indoors, so its frames showed no stars
(noise only: a 1080x1920 raw stream frame, median 2016 ADU, nothing above the noise), so every
plate solve failed. What that showed:

- **Clock.** `pi_get_time` read 2020-10-22 with `time_zone: "America/Chicago"`. After
  `pi_set_time` with local time in that zone it read the right local time and kept the zone.
- **Parked.** `mount: {close: true, move_type: "none", tracking: false}`, `scope_get_equ_coord`
  dec -90, `scope_get_horiz_coord` altitude -29.7.
- **Start of a goto from park.** `iscope_start_view` (star mode, Aldebaran) answered code 0. For
  about 7 s the view state had `AutoGoto` `working` with no stage under it and `mount.close` was
  still `true`; then `ScopeGoto` `working` with `cur_ra_dec` and `dist_deg`, and `close: false`.
  The slew of 102 degrees took 52 s.
- **`target_ra_dec` is echoed as sent**: `[4.62443, 16.562553]`, the equinox-of-date position.
  After the slew `scope_get_equ_coord` read `ra 4.624444, dec 16.5625`, the same numbers. This
  says the mount reports in the system the goto is given in. It does not say which system that
  is: that needs a solved frame.
- **Retries under a working `AutoGoto`.** `AutoGoto` > `Exposure` (2 s) > `AutoFocus` failed with
  code 279 "no star is detected" > `ScopeGoto` again (`dist_deg` 0.08) > `Exposure` >
  `PlateSolve` failed with code 251 "solve failed", and so on with `count` rising from 1 to 7,
  `hint: true` from the third attempt and the exposure growing (2 s, 4.5 s, 6.75 s). Through all
  of it `AutoGoto.state` stayed `working`. **A failed stage under a working `AutoGoto` is a
  retry, not the end of the goto.** (The `ScopeGoto` failure with code 501 above is different:
  the mount never moved.)
- **The end.** After 348 s (408 s in a second run) the telescope sent two `AutoGoto` events with `state: fail`,
  `error: "solve failed"`, `code: 251` (the second with `count: 7`). About two seconds later the
  view state had **no `AutoGoto` at all**: `stage: "ContinuousExposure"`, `state: "working"`,
  tracking on, exactly as after a goto that succeeded. The failure of the goto as a whole is
  visible only in the event. The first of the two events has `count: 0` and `lapse_ms: 0`.
- **`start_solve` on its own**: `PlateSolve` failed after 31 s; `get_solve_result` gave
  `{"duration_ms": 30998, "star_number": 723, "state": "fail"}`.
- **`start_auto_focuse` on its own**: ran 79 s and failed; the focuser stayed at 1253.
- **`iscope_start_stack {restart: true}`**: the view went to `stage: "Initialise"` with
  `Initialise` > `AutoFocus` `working` (the `auto_af` setting was on), for about 85 s; then
  `stage: "Stack"` with `Stack: {state: "working", stacked_frame: 0, dropped_frame: 0,
  frame_errcode: -1, Exposure: {exp_ms: 20000.0}}`. Nothing was stacked; `get_stacked_img`
  answered "there is no image now". `iscope_stop_view {stage: "Stack"}` returned the view to
  `ContinuousExposure`.
- **`get_current_img`** answered "there is no image now" in `ContinuousExposure`; `begin_streaming`
  delivered a raw frame within half a second.
- **`iscope_stop_view`** (no stage): `View` `cancel`, `mode: "none"`.
- **Park.** `scope_park` answered code 0. `mount.close` read `true` within 20 s, while
  `move_type` was still `"ScopeHome"` and the declination still changing (-87.8); `move_type`
  went to `"none"` 55 s after the request, at dec -90. **`close` alone does not mean the park
  has finished.**

### Live view from the park position (2026-11-26)

`iscope_start_view {"mode": "star"}` with no target, sent while parked: code 0, `View` `working`
in star mode at once, and the mount did not move (`close` stayed `true`, altitude -29.7).
`begin_streaming` then delivered raw 1080x1920 16-bit frames, the first within 0.4 s and the next
a second later. `stop_streaming` and `iscope_stop_view` ended it: `View` `cancel`, `mode: "none"`.

### Focuser and filter wheel (2026-11-26)

- `get_device_state {"keys": ["focuser"]}`: `{"focuser": {"max_step": 2600, "state": "idle",
  "step": 1253}}`; `state` is `"moving"` during a move. `get_focuser_position` returns the step
  as a bare number.
- `move_focuser {"step": 1273, "ret_step": true}`: code 0, `{"step": 1273}`. The focuser covers
  roughly 150 to 200 steps a second.
- **A step beyond the travel is not refused.** `move_focuser {"step": 99999}` answered code 0
  with `{"step": 2600}` and the focuser ran towards the end of its travel.
- `stop_focuser` (no params): code 0, `result: 0`; a running move stopped where it was and the
  state went to `idle`.
- `get_wheel_setting`: `{"exp_sec": [2.0, 2.0, 2.0], "names": ["dark", "IRCUT", "LP"]}`.
  `get_wheel_position`: a bare number, `1` with the IR-cut filter in (a star view with
  `lp_filter: false`). **Positions count from zero and index `names`.**
- `get_wheel_state`: `{"id": 0, "state": "idle", "unidirection": false}`; not `idle` for a little
  under two seconds after a move is requested.
- `set_wheel_position [2]`: code 0, `result: 0`, and `get_wheel_position` read 2 afterwards;
  `[1]` put it back. `set_wheel_position [7]`: code 204 "out of limit".

### Dew heater (2026-11-26)

- The device state has `setting.heater_enable` and `setting.expt_heater_enable`, both `false`.
- `set_setting {"heater_enable": true}`: **code 109 "unexpected param"**. The heater is not a
  setting that can be written.
- `pi_output_set2 {"heater": {"state": true, "value": 50}}`: code 0, `result: 0`, and
  `setting.heater_enable` read `true` afterwards; the same with `state: false` put it back.
- `pi_output_get2` did not change with the heater: four entries, each
  `{"is_pwm": false, "state": false, "type": "other", "value": 100.0}`, before and after.
  `pi_output_get`: `[false, false, false, false]`. The power sent as `value` is not reported
  anywhere that was found, so whether an S30 varies its heater is not known.
- Only the flag was observed. That the lens warms was not measured.

### Outdoors on a wedge, in alt-az mode (2026-11-27)

The S30 (9.75) was outside, tilted as for equatorial use (`balance_sensor.data.angle` 62.2), but
reported `mount.equ_mode: false`. Its clock was at 2020-10-23 again after a power cycle. The sky
clouded over within about fifteen minutes, so only the first goto found stars.

- **A goto that solves.** `iscope_start_view` to Alpheratz (sent as JNow 0.162995 h,
  +29.240078): slewing, then centring, then no `AutoGoto` in the view state and the view in
  `ContinuousExposure`, 79 s after the request. `Observer::acquisition` read `OnTarget`. The
  telescope's own solve, from its `PlateSolve` event: 0.163006 h, +29.205744.
- That solve does not say which epoch the telescope works in: it centres until its solution
  matches the numbers it was sent, whatever they mean. The frame after a goto, compared with the
  frame after the same goto with J2000 numbers, would; clouds stopped that.
- **Autofocus that completes**: `start_auto_focuse` from 1253, 45 s, `AutoFocus` `complete`,
  focuser at 1235.
- **`start_solve` by hand** straight after the goto ended as a solve that completed without a
  position (the backend's reading of a `PlateSolve` `complete` event with no `ra_dec`; the
  event itself was not logged).
- **Starting a stack ran a polar-alignment measurement first**: `View.Initialise.3PPA`
  (`percent`, then its own `AutoGoto` with `AutoFocus` under it, and
  `equ_offset: [-0.698765, 1.890288]`), still at 99% after 180 s. No frame was stacked in that
  time. `setting.auto_3ppa_calib` was true.
- **`scope_park {"equ_mode": true}` did not change the mode**: code 0 and the mount parked, from
  open and again from parked, with `mount.equ_mode` false afterwards both times. The app also
  refused to switch that evening; it appears to want clear sky for it.
- **A goto stopped from the app** arrives as an `AutoGoto` `fail` event, "aborted", code 253,
  with a `cancel` event beside it.
- **Under cloud**, the goto of 2026-11-26 again: seven failed solves over 300 s, then the
  `AutoGoto` `fail` event (251). The view state still showed the goto `working` when the event
  came; `is_slewing` and `acquisition` read it as over.

### Equatorial mode under a clear sky: the epoch, autofocus and a stack (2026-11-27)

After a power cycle the same S30 reported `mount.equ_mode: true` (whether the earlier
`scope_park {"equ_mode": true}` took effect at the restart, or the app's attempt did, is not
known). The sky was mostly clear.

- **The telescope works in the equinox of date (JNow).** Alpheratz, J2000 0.13979 h +29.0904,
  JNow 0.162995 h +29.240078. Sent as the JNow numbers (`iscope_start_view`), the goto ended
  with the star 25 pixels from the centre of the 1080x1920 frame. Sent as the J2000 numbers, it
  ended with the star 262 pixels from the centre (dx -228, dy -130): 286 pixels from where it
  was, about 0.32 degrees at 3.99 arcseconds a pixel, against 0.34 degrees of precession. The
  telescope's own solutions were 0.162835 h +29.245097 and 0.143311 h +29.093350: each within
  0.05 degrees of the numbers it was sent, so its solver also reports the equinox of date.
- `scope_get_equ_coord` after the first goto: 0.160833 h +29.233056, 0.03 degrees from the JNow
  position and 0.31 from the J2000 one.
- The goto from park took 77 s to `OnTarget`; from the same field, 17 s.
- **Autofocus**: 45 s, 1253 to 1246.
- **A stack that stacks**: `Initialise` for about 50 s (no `3PPA` stage this time), then
  `Stack` with 20 s exposures. After 130 s: `stacked_frame: 1`, `dropped_frame: 4`.
  `get_stack_info` `{"height": 1920, "width": 1080}`, and the stacked image came back as
  1080x1920 RGB16. `View.Stack.Annotate.result.annotations` named the star with
  `ra_dec_j2000: [2.096909, 29.090435]` (degrees, J2000) and a pixel position.
- **A solve completes twice.** Every solve ends with two `PlateSolve` `complete` events a
  millisecond apart. The first has a `result`: `{"angle": 0.779999, "duration_ms": 2768,
  "focal_len": 149.809067, "fov": [1.1987, 2.12953], "image_id": 65535,
  "ra_dec": [0.164201, 29.244796], "star_number": 817}` (hours and degrees of date; `fov` in
  degrees, the short side first), with `page` `focus` for `start_solve` and `preview`, plus
  `tag: "Exposure-AutoGoto"` and `ac_count`, for a goto's. The second has `lapse_ms` and
  `route`; it repeats `ra_dec` at the top level when a goto asked (`route: ["View",
  "AutoGoto"]`) and has no position when `start_solve` did (`route: []`). Reading only the
  second made every `start_solve` look like a solve without a position.
- `get_solve_result` returns the same `result` object with `"state": "complete"`.
- Park from the target: 52 s.

### The Seestar's own Alpaca server (2026-11-27)

The S30 (9.75) has an ASCOM Alpaca server, off until it is switched on in the app (said to send
`set_setting {"alpaca_enable": true}`; that was not sent from here). With it off, TCP 32323 was
closed and an `alpacadiscovery1` datagram to UDP 32227 got no answer. Switched on, port 32323
answered: `ManufacturerVersion` `1.3.1-1`, API version 1, and five devices, each number 0:
Camera ("Telephoto Camera"), Focuser, FilterWheel, Telescope and Switch.

Read through `AlpacaBackend` (`examples/alpaca_info.rs`), with the telescope parked:

- **Telescope**: can slew, sync, park, unpark, track, move an axis, find home, pulse guide, set
  site and time; cannot slew in alt-az. `EquatorialSystem` is topocentric (the equinox of date,
  as the native protocol works). `AlignmentMode` polar (it was in equatorial mode). `AtPark`
  **false** and `AtHome` true while the native state said `mount.close: true`. `SideOfPier` is
  not implemented.
- **Camera**: 1080x1920, 2.9 micron pixels, GRBG, gain 0 to 400, exposures 0.00003 to 2000 s,
  no binning, no cooler, a sensor temperature. A one-second exposure came back in 5.6 s as
  1080x1920 16-bit Bayer.
- **Focuser**: absolute, 0 to 2600, with a temperature. A 20-step move and back: 1.6 s and
  0.8 s.
- **FilterWheel**: names `Dark`, `IR`, `LP` (the native names are `dark`, `IRCUT`, `LP`),
  focus offsets, position 1. One position on and back: about 2 s each.
- **Switch**: named "Alpaca Switch Simulator" by its driver, with one switch, `Dew heater`, on
  or off. Set to 1 and back to 0, reading back each time.

Then the mount, through `AlpacaBackend` (`alpaca_info --mount`), in equatorial mode:

- `Unpark`: accepted, and the mount read idle at once, still folded.
- `Tracking = true` straight after: **error 1279 (0x4FF), "SET_SCOPE_SET_TRACK_STATE fail: below
  horizon"**. Tracking cannot be switched on from the park position.
- `SlewToCoordinatesAsync` to Alpheratz (sent in the equinox of date, which is what the device
  says it works in): `Slewing` true for 46 s, tracking came on by itself just before the end,
  then `Slewing` false and the mount tracking. It reported 0h 10m 31.0s +29d 14' 23.0", 0.16
  degrees from where it was sent.
- **An Alpaca slew is not centred by plate solving.** A one-second frame there showed a star
  field without Alpheratz in it: the pointing was off by more than the half-degree or so that
  would keep the star in the 1.2 by 2.1 degree frame. The native goto, which solves and
  re-centres, had put the same star 25 pixels from the centre an hour earlier.
- `Park`: 50 s, then `AtPark` true and `AtHome` true. (`AtPark` had read false before this,
  with the telescope folded by the native park.)

### Wide camera

Ports open on the LAN: 4700, 4701, 4702, 4400, 4800, 4801, 4804. Ports 4554 and 4555 (RTSP) refused
connections while no video page was active.

| Observation | Result |
|---|---|
| Port 4702, unverified `test_connection` | Silently consumed: 4702 has its own verification gate |
| Port 4702, `get_verify_str` + `verify_client` with the same key | Code 0; the socket is then served like 4700 |
| 4702 `get_camera_info` | chip 1920×1080, 2.7 µm, colour, `debayer_pattern` "RG" (the wide camera) |
| 4702 `get_setting` | Only the wide camera's own settings (the object that 4700 reports under `second_camera`) |
| 4702 `iscope_start_view {"mode": "star"}` | Code 0. `get_view_state` on 4702 then has `SecondView` with `cam_id` 1, stage `ContinuousExposure`; on 4700 the main `View` is unchanged. The mount did not move |
| Events for that view | `SecondView` and `ContinuousExposure` arrived on **both** 4702 and 4700 (forwarded) |
| 4804 `get_current_img` and `get_camera_img` during the wide view | `there is no image now` |
| 4804 `begin_streaming` during the wide view | `done`, then raw frames: imgType 1, dataType 2, **1920×1080**, canDebayer 1, 4 147 200 bytes, repeating the request id |
| 4702 `iscope_stop_view` | Code 0; `SecondView` cancelled, also reported on 4700 |
| 4700 `iscope_get_app_state` | Has a `selected_cam` member; `get_setting.wide_cam` was false |

So the wide camera can be driven by a second verified control connection to 4702 and streamed from
4804, with its events also visible on 4700. Selecting it through 4700 (`selected_cam`, `wide_cam`)
was not tried.

### View plan

The device held a plan saved from the official app. `iscope_get_app_state` returned it under
`ViewPlan`: `state` cancel, and each target with `alias_name`, `target_name`, `target_ra_dec`,
`start_min`, `duration_min`, `lp_filter`, `skip`, `target_id`, and its own `state`, `code`, `error`
and `lapse_ms`; no `camera_mode`. With the owner's agreement it was replaced:

| Step | Result |
|---|---|
| `set_view_plan` with a new `plan_name` and one `tele` target at the current local minute | Code 0; the held plan was replaced. Events as on 6.70: `Setting`, `ViewPlan` working, `Target` working, `View` working, `AutoGoto`, `ScopeGoto`; the mount moved |
| `stop_func {"name": "ViewPlan"}` | Code 0; goto cancelled, view ended, `Target` cancel, `ViewPlan` cancel; **plan kept**, target state `fail` |
| `clear_view_plan` | Code 0; `iscope_get_app_state` then had no `ViewPlan` member |
| `set_view_plan` with no params (restart the held plan) afterwards | 207 "fail to operate": nothing is held, so the clear did delete the plan |
| `get_view_plan_stack_result` on 4700 and on 4702 | 103 "method not found" on both (also 103 on the S50, firmware 6.70) |

## Not yet observed

- Firmware between 7.06 and 9.75. The marker passes the gate on 6.70 (hardware) and 7.06 (corpus)
  and does not on 9.75 (hardware); where it stopped is unknown, so the client probes for it per
  connection.
- Driving the wide camera through port 4700 (`selected_cam` / `wide_cam`), a wide-camera goto or
  stack, and the RTSP streams.
- A completed goto, autofocus and stack on an S50 (seen on an S30, 9.75, on 2026-11-27).
- A JPEG reply on the image socket (zip was observed on 9.75, not on 6.70).
- `Alert` 429 on exceeding the connection limit.
- A plan running to completion, and a restart of a held plan that exists.
