# Seestar conformance corpus

A language-neutral corpus of **real, sanitized** Seestar control-channel sessions,
plus a parity harness that replays it through both implementations:

- **scopinator-rs** (this repo)
- **pyscopinator** (`~/Projects/erewhon/pyscopinator`)

The corpus is the shared oracle: it pins how each firmware version actually
behaves on the wire, and the harness proves the two libraries agree.

## Why

There is no official Seestar protocol spec or firmware changelog. Across firmware
versions, commands are **added, dropped, or have their parameters changed**, and
new async event types appear. We reconstruct that matrix empirically from
captured sessions, and use it to:

1. keep `scopinator-rs`'s `Command` / `SeestarEvent` / response models honest
   against real traffic (`crates/scopinator-seestar/tests/session_replay.rs`);
2. detect drift when a new firmware capture is added (the replay tests fail,
   pinpointing the added/dropped/changed message);
3. maintain **parity** with pyscopinator so the two ports don't diverge.

## Layout

```
conformance/
  sessions/<name>/control.jsonl   # the corpus (sanitized; see below)
  sessions/<name>/manifest.json   # model / firmware / message counts
  parity/pyscopinator_report.py   # pyscopinator -> normalized report
  parity/compare.py               # diff two reports, flag divergences
  reports/                        # generated reports (gitignored)
```

Session naming encodes the variant: `s50_fw670_a` = Seestar S50, firmware 6.70.
The corpus currently covers **firmware 6.70 (S50)** and **7.06 (S30)**.

### `control.jsonl` format

One JSON record per line:

```json
{"timestamp": 1775012248.33, "direction": "client", "raw": "<json-rpc string>"}
```

`direction` is from the capturing proxy's vantage point:
- `client`    — app → telescope (a command)
- `telescope` — telescope → app (a command response or an async event)

`raw` is the on-wire JSON-RPC message, as a string (JSON within JSON).

## Sanitization (important)

Raw captures contain **real secrets** — Wi-Fi passwords, home GPS coordinates,
device serial numbers, CPU IDs, local network addressing, client hostnames. The
committed corpus is scrubbed by `tools/sanitize_session.py`, which replaces those
values with stable, same-typed placeholders while preserving message structure.

**Never commit a raw capture.** To add sessions:

```bash
# 1. sanitize a raw seestar-proxy capture into the corpus
python3 tools/sanitize_session.py <raw_session_dir> conformance/sessions/<name>

# 2. verify no secrets survived (output must show only placeholders)
python3 tools/analyze_sessions.py --pii conformance/sessions

# 3. see what the new session changed (added/dropped commands & events)
python3 tools/analyze_sessions.py conformance/sessions
```

## Replaying (Rust tests)

```bash
cargo test -p scopinator-seestar --test session_replay
cargo test -p scopinator-seestar --test control_integration
```

`session_replay` parses every telescope message, round-trips every modeled
client command back to its captured wire bytes, and pins the firmware matrix.
`control_integration` drives the real client against a localhost `FakeSeestar`
using real captured payloads (correlation, events, disconnect, reconnect,
timeout, malformed input).

## Parity (scopinator-rs vs pyscopinator)

Both implementations emit a **normalized report** with the same schema; the
comparator aligns them message-by-message.

```bash
# generate both reports and diff in one shot
python3 conformance/parity/compare.py --run

# or generate separately
cargo run -q -p scopinator-seestar --example conformance_report > conformance/reports/rust.json
cd ~/Projects/erewhon/pyscopinator && uv run python \
  ~/Projects/erewhon/scopinator-rs/conformance/parity/pyscopinator_report.py \
  ~/Projects/erewhon/scopinator-rs/conformance/sessions > /tmp/py.json
python3 conformance/parity/compare.py conformance/reports/rust.json /tmp/py.json
```

The comparator distinguishes two kinds of difference:

- **capability gaps** — one library models a command the other doesn't. Expected;
  reported but non-fatal. (Currently: `pi_station_state`, `play_sound` are modeled
  by scopinator-rs only.)
- **hard divergences** — the libraries disagree on message classification or one
  fails to parse a message the other accepts. These fail the check (exit ≠ 0) and
  indicate a real bug or drift to reconcile.

### Report schema (version 1)

```jsonc
{ "schema": 1, "impl": "scopinator-rs" | "pyscopinator",
  "sessions": [ { "session": str, "firmware_int": int|null, "messages": [ obs, … ] } ] }
```

where each `obs` is one of:

```jsonc
// app → telescope
{ "i": int, "dir": "client", "method": str, "modeled": bool }
// telescope → app
{ "i": int, "dir": "telescope", "class": "event"|"response"|"unknown",
  "parse_ok": bool, "event"?: str, "method"?: str, "id"?: int, "code"?: int }
```
