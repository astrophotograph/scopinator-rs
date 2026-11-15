# Hardware observations

What a real Seestar did when asked. Where pyscopinator, the protocol specification
([zmerlynn/seestar-spec](https://github.com/zmerlynn/seestar-spec), commit `14ac005`, which
describes firmware 9.31 and 9.75) and hardware disagree, **hardware wins, then the spec, then
pyscopinator**. Byte parity with pyscopinator is kept as a drift detector, not as the definition of
correct: the parity corpus pinned several requests the device rejects.

Every entry is labelled OBSERVED with the date, model and firmware, following the spec's own
labelling. Nothing here is inferred from another firmware version.

**Device for all entries below:** Seestar S50, firmware 6.70 (`firmware_ver_int` 2670), on the LAN,
indoors, 2026-11-14. "Marker" means the `verify` marker in `params`. "Verified socket" means a
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

## Not yet observed

- Any firmware other than 6.70. Whether the marker still passes the gate between 7.06 (corpus) and
  9.31 (spec, which does not mention it) is unknown; the client probes for it per connection.
- A zip-compressed (dataType 3) or JPEG reply on the image socket.
- `Alert` 429 on exceeding the connection limit.
- Plate solving, stacking and annotation results (the device was indoors).
