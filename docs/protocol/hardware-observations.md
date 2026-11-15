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
unreachable and it says nothing.

During the goto, on port 4800:

| Request | Reply |
|---|---|
| `get_current_img {"compress": false, "img_type": "Raw"}` | dataType 2, imgType 1, 1080×1920, canDebayer 1, 4 147 200 bytes |
| `get_current_img {"compress": true, "img_type": "Raw"}` | dataType **3**: a zip of 1 959 646 bytes with one entry `raw_data` of 4 147 200 bytes |
| `get_current_img` with no params | The same zip reply: compression is the default here |
| The same three requests on port 4804 (wide camera) | `there is no image now` while the main camera has the view |

`ScopeTrack` uses the state strings `on` / `off`, which are outside the usual
start/working/complete set.

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
- A completed goto with plate solving, on any firmware (both devices were indoors).
- A JPEG reply on the image socket (zip was observed on 9.75, not on 6.70).
- `Alert` 429 on exceeding the connection limit.
- Plate solving, stacking and annotation results (the device was indoors).
- A plan running to completion, and a restart of a held plan that exists.
