# Heartbeat hardcodes `["verify"]` regardless of firmware

Filed from Forge task "Pin heartbeat verify behavior and write bug report".
Status: **open, not adjudicated.** This report describes both behaviours and gives no verdict.

## Python (oracle)

`pyscopinator/src/scopinator/seestar/connection.py` decides per message whether to inject
`"verify"` (`_should_inject_verify` / `_transform_message_for_verify`): only when the detected
firmware is above 2582 (or unknown). Below that, commands are sent without the marker.

## Rust

- `crates/scopinator-seestar/src/command/serialize.rs`: `serialize_command` uses
  `FirmwareVersion::requires_verify()` (`> 2582`, `VERIFY_THRESHOLD`) and `inject_verify`. For
  `Command::PiGetTime` it yields `{"id":N,"method":"pi_get_time"}` on firmware <= 2582 and
  `{"id":N,"method":"pi_get_time","params":["verify"]}` on firmware > 2582 or unknown.
- `crates/scopinator-seestar/src/connection/control.rs`, `heartbeat_task`: builds
  `{"id":1,"method":"pi_get_time","params":["verify"]}` directly with `serde_json::json!`. It takes no
  firmware input, so the bytes are identical on every firmware, including ones where
  `serialize_command` would omit `params`.

## Affected range

Firmware <= 2582 (where `requires_verify()` is false). Heartbeat bytes on > 2582 and unknown firmware
match `serialize_command`. Whether sub-2582 firmware rejects or ignores the extra param is not
established by this repo's corpus.

## Pinned by

`heartbeat_always_sends_hardcoded_verify` and `heartbeat_ignores_firmware_unlike_serialize_command`
in `crates/scopinator-seestar/src/connection/control.rs` (`heartbeat_tests`, paused tokio time).
They assert the current hardcoded bytes and the divergence from `serialize_command` on 2000 and 2582.

## Possible direction (not a decision)

Build the heartbeat through `serialize_command(&Command::PiGetTime, 1, fw)` with the firmware held in
`ControlState`, which would make both paths agree; the heartbeat task would need access to that state.
