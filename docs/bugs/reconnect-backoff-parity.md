# Reconnect backoff: Rust `ReconnectPolicy` vs pyscopinator `connection.py`

Forge task: "Add ReconnectPolicy parity spec test" (Scopinator). Compared by hand on
2026-11-08 against `pyscopinator/src/scopinator/seestar/connection.py`, because the
foreman worker that wrote the spec test could not read the sibling checkout.

## Rust (`crates/scopinator-seestar/src/connection/reconnect.rs`)

- Series: 1, 2, 4, 8, 16, 32, 60, 60, … seconds (doubling, capped at `max_backoff` = 60 s).
- Reset to 1 s on a successful connection.
- Deterministic: no jitter, no alternative mode.
- Pinned by `reconnect_backoff_matches_python_schedule_spec`.

## Python (`_do_reconnect_with_backoff`, connection.py ~171–220)

- Normal path: `min(1.0 * 2 ** min(attempts - 1, 6), 60.0) + uniform(0, 1)`
  → 1, 2, 4, 8, 16, 32, 60, 60, … seconds **plus 0–1 s of random jitter**.
- Reboot-detected path (`_reboot_detected`, set when the device reports a reboot):
  `min(5.0 * 2 ** min(attempts - 1, 3), 60.0) + uniform(0, 2)` → 5, 10, 20, 40, 40, … + 0–2 s.
- `_reconnect_attempts` resets to 0 on success (lines ~78 and ~252) and is clamped
  back to 10 after 50 attempts (log-spam control; at attempt ≥ 7 the delay is already 60 s,
  so the clamp does not change the schedule).

## Divergences (no verdict here)

1. **Jitter.** Python adds up to 1 s (2 s after a reboot) of random delay; Rust adds none.
   Rust reconnects are therefore slightly faster and fully synchronous across clients.
2. **Reboot mode.** Python starts at 5 s and plateaus at 40 s when it has detected a device
   reboot; Rust has no notion of a detected reboot and always uses the 1 s series.
3. **Series and cap agree** on the normal path (1 → 60 s, reset on success), so the Rust spec
   test's literal schedule is the Python deterministic schedule.

Adjudication (whether Rust should add jitter and/or a reboot mode) is the separate human task
"Adjudicate divergences and file parity bugs".
