# pyscopinator sends wire forms the telescope rejects

Draft for filing against pyscopinator (github.com/astrophotograph/pyscopinator). Written
2026-11-15 from a read of pyscopinator at `1ee4495` and from
[`docs/protocol/hardware-observations.md`](../protocol/hardware-observations.md) (Seestar S50 on
firmware 6.70, Seestar S30 on firmware 9.75, both observed 2026-11-14). Status: **filed as Forge follow-up tasks on 2026-11-15** (project Scopinator), not as GitHub issues.
scopinator-rs had every one of these and fixed them on 2026-11-14.

Each item is a separate defect and can be filed as its own issue.

## 1. No verification handshake: nothing works on firmware 9.75

pyscopinator only knows the `verify` marker (`seestar/client.py`,
`_transform_message_for_verify`). On firmware 9.75 the marker no longer passes the gate: every
request on an unverified socket is silently consumed, marked or not. A client has to answer
`get_verify_str` with an RSA PKCS#1 v1.5 SHA-1 signature in `verify_client`; after that it needs no
marker. Without this, every `send_and_recv` on 9.75 waits out its timeout.

## 2. List params are wrapped instead of extended

`_transform_message_for_verify` turns list params into `[params, "verify"]` for every method except
`set_wheel_position`, where it appends. The device wants the marker appended to the list for every
method. Commands this breaks:

| Command | Sent | Device |
|---|---|---|
| `set_sequence_setting` | `[[{"group_name": ...}], "verify"]` | 107 "expected object param" |
| `pi_set_time` | `[[{...}], "verify"]` | works only as `[{...}, "verify"]` |
| `scope_sync`, `set_control_value` (tuple params) | `[[a, b], "verify"]` | not observed; the flat form is the one the spec gives |

Fix: append for all lists and drop the `set_wheel_position` special case. (Adjudicated 2026-11-15:
the Rust serializer, which appends for every method, is the correct one.)

## 3. `set_setting` with the marker reports failure, and can skip members

The marker is added as a last object member, `{"...": ..., "verify": true}`. The firmware applies
members in wire order, does not roll back, and treats `verify` as an unknown member: the settings
are applied, and the reply is 109 "unexpected param". On a verified socket the marker is rejected
the same way. So every marked `set_setting` looks like a failure to the caller. There is no marked
form that returns code 0; a caller in marker mode has to treat 109 as "applied" and read the
setting back.

## 4. Methods that do not exist

All return 103 "method not found" on port 4700 on both firmwares:

- `goto_target` (`commands/parameterized.py`, used by the CLI). A goto is
  `iscope_start_view {"mode": "star", "target_ra_dec": [ra_hours, dec_deg], "target_name": ...}`.
- `get_annotated_result` (`commands/simple.py`). The method is `get_annotate_result`.
- `get_stack_setting`, `set_stack_setting` (`commands/simple.py`, `commands/settings.py`; the
  latter is called from `client.py`).

## 5. A goto does nothing when the device clock is unset

Seen on the S30: its clock read 2020, the goto was accepted with code 0, and the mount did not
move. After `pi_set_time` the same request slewed. pyscopinator has `PiSetTime` but nothing sets
the clock on connect. scopinator-rs made it opt-in (`ClockSync`).
