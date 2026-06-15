#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = []
# ///
"""Analyze captured Seestar proxy sessions to build a firmware/command/event matrix.

The seestar-proxy capture format is a directory per session:

    session_YYYYMMDD-HHMMSS/
        control.jsonl     # one JSON record per line (see below)
        frames/           # binary imaging frames (not parsed here)
        manifest.json     # capture metadata

Each line of ``control.jsonl`` is::

    {"timestamp": <float>, "direction": "client"|"telescope", "raw": "<json-rpc string>"}

``direction`` is from the proxy's point of view:
  * ``client``    — message the app sent *to* the telescope (a command)
  * ``telescope`` — message the telescope sent *back* (a response or an async event)

The firmware version is embedded in ``get_device_state`` responses as
``result.device.firmware_ver_string`` / ``firmware_ver_int``.

Why this script exists
----------------------
The Rust port must track firmware drift: across versions, commands are added,
dropped, or have their parameter shapes changed, and new event types appear.
There is no official changelog, so we reconstruct the matrix empirically from
real captures. Re-run this whenever new sessions or firmware versions land to
see what changed, and to keep the test fixtures and ``Command``/``SeestarEvent``
models honest.

Usage
-----
    python3 tools/analyze_sessions.py [SESSIONS_DIR ...]      # human report
    python3 tools/analyze_sessions.py --json [SESSIONS_DIR]   # machine readable
    python3 tools/analyze_sessions.py --pii  [SESSIONS_DIR]   # list secrets to redact

If no directory is given it defaults to ``../seestar-proxy`` relative to the
repo root (the usual capture location), falling back to the current directory.

The ``--pii`` mode matters: raw captures contain real Wi-Fi passwords, GPS
coordinates, serial numbers and hostnames. Never commit raw captures as test
fixtures — sanitize them first. This mode enumerates exactly what must be
scrubbed.
"""
from __future__ import annotations

import argparse
import glob
import json
import os
import sys
from collections import defaultdict

# Commands the Rust `Command` enum currently models (method strings).
# Keep this in sync with crates/scopinator-seestar/src/command/mod.rs::method().
# A captured method missing from this set is an "added" command we don't model yet.
KNOWN_RUST_COMMANDS = {
    "test_connection", "pi_is_verified", "pi_reboot", "pi_get_time", "pi_set_time",
    "get_device_state", "get_view_state", "get_camera_info", "get_camera_state",
    "get_setting", "get_stack_setting", "get_stack_info", "get_disk_volume",
    "get_user_location", "get_wheel_position", "get_wheel_setting", "get_wheel_state",
    "get_last_solve_result", "get_solve_result", "get_annotated_result",
    "scope_get_equ_coord", "scope_get_ra_dec", "scope_get_horiz_coord", "scope_sync",
    "scope_park", "scope_move_to_horizon", "scope_speed_move", "scope_set_track_state",
    "goto_target", "iscope_start_view", "iscope_stop_view", "iscope_start_stack",
    "get_focuser_position", "move_focuser", "start_auto_focuse", "stop_auto_focuse",
    "set_user_location", "set_setting", "set_stack_setting", "set_control_value",
    "pi_output_set2", "begin_streaming", "stop_streaming", "get_stacked_img",
    "start_solve", "start_scan_planet", "set_view_plan", "stop_func",
    "set_sequence_setting", "play_sound", "pi_station_state",
}

# Event variants the Rust `SeestarEvent` enum currently models (Event strings).
# Keep in sync with crates/scopinator-seestar/src/event/mod.rs.
KNOWN_RUST_EVENTS = {
    "Alert", "AutoFocus", "AutoGoto", "ContinuousExposure", "DarkLibrary", "DiskSpace",
    "Exposure", "FocuserMove", "Initialise", "PiStatus", "RTSP", "SaveImage",
    "ScopeGoto", "ScopeHome", "ScopeMoveToHorizon", "ScopeTrack", "Stack", "View",
    "WheelMove", "Annotate", "AutoGotoStep", "BatchStack", "Client", "EqModePA",
    "GoPixel", "Internal", "PlateSolve", "ScanSun", "SecondView", "SelectCamera",
    "Setting", "3PPA", "ViewPlan",
}


def fwkey(s):
    return s if s else "??"


def param_shape(p):
    """A compact, hashable description of a params payload's shape."""
    if isinstance(p, dict):
        return "{" + ",".join(sorted(p.keys())) + "}"
    if isinstance(p, list):
        return "[list:" + ",".join(type(x).__name__ for x in p) + "]"
    if p is None:
        return "(none)"
    return type(p).__name__


def find_sessions(sessions_dir):
    """Return session dirs under ``sessions_dir``.

    A "session" is any directory containing a ``control.jsonl``. This matches
    both the raw ``session_YYYYMMDD-HHMMSS`` captures and the named conformance
    corpus dirs (``s50_fw670_a`` etc.). If ``sessions_dir`` itself holds a
    ``control.jsonl`` it is treated as a single session.
    """
    if os.path.exists(os.path.join(sessions_dir, "control.jsonl")):
        return [sessions_dir]
    found = []
    for entry in sorted(glob.glob(os.path.join(sessions_dir, "*"))):
        if os.path.isdir(entry) and os.path.exists(os.path.join(entry, "control.jsonl")):
            found.append(entry)
    return found


def iter_records(control_path):
    """Yield (direction, parsed_message) for each valid line in a control.jsonl."""
    with open(control_path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            try:
                rec = json.loads(line)
                msg = json.loads(rec["raw"])
            except (json.JSONDecodeError, KeyError, TypeError):
                continue
            yield rec.get("direction"), msg


def analyze(sessions_dir):
    result = {
        "firmware_by_session": {},
        "firmware_int": {},        # string -> int
        "client_methods": defaultdict(set),       # method -> {firmware}
        "client_param_shapes": defaultdict(set),   # method -> {shape}
        "events": defaultdict(set),                # event -> {firmware}
        "error_responses": {},     # (method, code, error) -> count
        "models": set(),           # product_model strings seen
        "pii": defaultdict(set),   # field -> {values}
        "session_count": 0,
        "total_records": 0,
    }
    sessions = find_sessions(sessions_dir)
    for d in sessions:
        cf = os.path.join(d, "control.jsonl")
        if not os.path.exists(cf):
            continue
        result["session_count"] += 1
        fw = None
        records = list(iter_records(cf))
        result["total_records"] += len(records)
        # First pass: detect firmware/model/PII from any device-state response.
        for _direction, msg in records:
            r = msg.get("result")
            if isinstance(r, dict):
                dev = r.get("device")
                if isinstance(dev, dict) and "firmware_ver_string" in dev:
                    fw = dev.get("firmware_ver_string")
                    if dev.get("firmware_ver_int") is not None:
                        result["firmware_int"][fw] = dev["firmware_ver_int"]
                    if dev.get("product_model"):
                        result["models"].add(dev["product_model"])
                    if dev.get("sn"):
                        result["pii"]["device.sn"].add(dev["sn"])
                if "location_lon_lat" in r:
                    result["pii"]["location_lon_lat"].add(str(r["location_lon_lat"]))
                ap = r.get("ap")
                if isinstance(ap, dict):
                    for k in ("ssid", "passwd"):
                        if ap.get(k):
                            result["pii"][f"ap.{k}"].add(ap[k])
                st = r.get("station")
                if isinstance(st, dict) and st.get("ssid"):
                    result["pii"]["station.ssid"].add(st["ssid"])
        result["firmware_by_session"][os.path.basename(d)] = fw
        # Second pass: tally methods/events/errors against the detected firmware.
        for direction, msg in records:
            if direction == "client":
                m = msg.get("method")
                if m:
                    result["client_methods"][m].add(fwkey(fw))
                    result["client_param_shapes"][m].add(param_shape(msg.get("params")))
                    p = msg.get("params")
                    if isinstance(p, dict) and p.get("cli_name"):
                        result["pii"]["cli_name"].add(p["cli_name"])
            else:
                ev = msg.get("Event")
                if ev:
                    result["events"][ev].add(fwkey(fw))
                if "error" in msg:
                    key = (msg.get("method"), msg.get("code"), str(msg.get("error")))
                    result["error_responses"][key] = result["error_responses"].get(key, 0) + 1
    return result


def print_report(r):
    fws = sorted(v for v in set(r["firmware_by_session"].values()) if v)
    print("=" * 72)
    print(f"Sessions analyzed: {r['session_count']}   Records: {r['total_records']}")
    print(f"Telescope models:  {sorted(r['models']) or '(unknown)'}")
    print("Firmware versions: " + ", ".join(
        f"{s} (int {r['firmware_int'].get(s, '?')})" for s in fws) or "(none)")
    print("=" * 72)

    print("\n## CLIENT COMMANDS  (firmware seen | param shapes | modeled?)")
    for m in sorted(r["client_methods"]):
        modeled = "" if m in KNOWN_RUST_COMMANDS else "  <-- NOT MODELED IN RUST"
        print(f"  {m:24s} fw={sorted(r['client_methods'][m])} "
              f"params={sorted(r['client_param_shapes'][m])}{modeled}")

    if len(fws) >= 2:
        a, b = fws[0], fws[-1]
        only_a = sorted(m for m, s in r["client_methods"].items() if s == {a})
        only_b = sorted(m for m, s in r["client_methods"].items() if s == {b})
        both = sorted(m for m, s in r["client_methods"].items() if {a, b} <= s)
        print(f"\n  Commands only in {a}: {only_a}")
        print(f"  Commands only in {b}: {only_b}")
        print(f"  Commands in both:    {both}")

    print("\n## ASYNC EVENTS  (firmware seen | modeled?)")
    for e in sorted(r["events"]):
        modeled = "" if e in KNOWN_RUST_EVENTS else "  <-- NOT MODELED IN RUST"
        print(f"  {e:24s} fw={sorted(r['events'][e])}{modeled}")

    print("\n## ERROR RESPONSES  (method | code | message | count)")
    for (m, c, e), n in sorted(r["error_responses"].items(), key=lambda kv: (kv[0][1] or 0)):
        print(f"  code={c:<5} x{n:<4} method={m} error={e!r}")

    unmodeled_cmds = sorted(set(r["client_methods"]) - KNOWN_RUST_COMMANDS)
    unmodeled_evts = sorted(set(r["events"]) - KNOWN_RUST_EVENTS)
    if unmodeled_cmds or unmodeled_evts:
        print("\n## DRIFT — present in captures but NOT modeled in Rust")
        if unmodeled_cmds:
            print(f"  commands: {unmodeled_cmds}")
        if unmodeled_evts:
            print(f"  events:   {unmodeled_evts}")


def print_pii(r):
    print("## SENSITIVE VALUES IN CAPTURES — redact before committing as fixtures")
    if not r["pii"]:
        print("  (none detected)")
    for field in sorted(r["pii"]):
        for v in sorted(r["pii"][field]):
            print(f"  {field}: {v!r}")


def to_jsonable(r):
    out = dict(r)
    out["client_methods"] = {k: sorted(v) for k, v in r["client_methods"].items()}
    out["client_param_shapes"] = {k: sorted(v) for k, v in r["client_param_shapes"].items()}
    out["events"] = {k: sorted(v) for k, v in r["events"].items()}
    out["models"] = sorted(r["models"])
    out["pii"] = {k: sorted(v) for k, v in r["pii"].items()}
    out["error_responses"] = [
        {"method": m, "code": c, "error": e, "count": n}
        for (m, c, e), n in r["error_responses"].items()
    ]
    return out


def default_sessions_dir():
    here = os.path.dirname(os.path.abspath(__file__))
    candidate = os.path.normpath(os.path.join(here, "..", "..", "seestar-proxy"))
    return candidate if find_sessions(candidate) else "."


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("dirs", nargs="*", help="session capture directories (default: ../seestar-proxy)")
    ap.add_argument("--json", action="store_true", help="emit machine-readable JSON")
    ap.add_argument("--pii", action="store_true", help="list only sensitive values to redact")
    args = ap.parse_args(argv)

    dirs = args.dirs or [default_sessions_dir()]
    # Merge analysis across all given directories.
    merged = None
    for d in dirs:
        r = analyze(d)
        if merged is None:
            merged = r
            continue
        merged["session_count"] += r["session_count"]
        merged["total_records"] += r["total_records"]
        merged["firmware_by_session"].update(r["firmware_by_session"])
        merged["firmware_int"].update(r["firmware_int"])
        merged["models"] |= r["models"]
        for k, v in r["client_methods"].items():
            merged["client_methods"][k] |= v
        for k, v in r["client_param_shapes"].items():
            merged["client_param_shapes"][k] |= v
        for k, v in r["events"].items():
            merged["events"][k] |= v
        for k, v in r["pii"].items():
            merged["pii"][k] |= v
        for k, n in r["error_responses"].items():
            merged["error_responses"][k] = merged["error_responses"].get(k, 0) + n

    if merged is None or merged["session_count"] == 0:
        print(f"No sessions found in: {dirs}", file=sys.stderr)
        return 1

    if args.json:
        print(json.dumps(to_jsonable(merged), indent=2, sort_keys=True))
    elif args.pii:
        print_pii(merged)
    else:
        print_report(merged)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
