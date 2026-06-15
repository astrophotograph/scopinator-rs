#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = []
# ///
"""Sanitize a raw seestar-proxy session capture into a committable test fixture.

Raw captures contain real secrets — Wi-Fi passwords, home GPS coordinates,
device serial numbers, CPU IDs, local network addressing, and client hostnames.
None of that may land in a public repo. This script reads a session's
``control.jsonl``, recursively scrubs sensitive values (by key name) inside each
embedded JSON-RPC message while preserving the message *structure* (types and
shapes), and writes a redacted copy plus a trimmed ``manifest.json``.

Sanitization is deterministic: the same secret always maps to the same
placeholder, so replay tests remain stable across regenerations. Placeholders
keep the original JSON type (a redacted string stays a string, GPS stays a
2-float list) so deserialization exercises the same code paths as real data.

Usage:
    python3 tools/sanitize_session.py <raw_session_dir> <out_fixture_dir>

Verify afterward with:
    python3 tools/analyze_sessions.py --pii <out_fixture_parent_dir>
"""
from __future__ import annotations

import json
import os
import sys

# Keys whose *values* are sensitive, mapped to a same-typed placeholder.
# Matching is by exact key name, applied recursively at any depth, in both
# client commands and telescope responses/events.
REDACT_STRING = {
    "passwd": "redacted-pw",
    "ssid": "redacted-ssid",
    "sn": "00000000",
    "cpuId": "0000000000000000",
    "cli_name": "redacted.client.local",
    "ip": "0.0.0.0",
    "gateway": "0.0.0.0",
    "netmask": "0.0.0.0",
    "bssid": "00:00:00:00:00:00",
    "mac": "00:00:00:00:00:00",
}
# Keys whose value is a [lon, lat] pair (home location).
REDACT_LATLON = {"location_lon_lat"}


def scrub(value):
    """Recursively redact sensitive values, preserving structure/types."""
    if isinstance(value, dict):
        out = {}
        for k, v in value.items():
            if k in REDACT_LATLON and isinstance(v, list):
                out[k] = [0.0 for _ in v] if v else v
            elif k in REDACT_STRING and isinstance(v, str):
                out[k] = REDACT_STRING[k]
            else:
                out[k] = scrub(v)
        return out
    if isinstance(value, list):
        return [scrub(v) for v in value]
    return value


def sanitize_line(line):
    """Sanitize one control.jsonl record; return the redacted JSON line or None."""
    line = line.strip()
    if not line:
        return None
    try:
        rec = json.loads(line)
        msg = json.loads(rec["raw"])
    except (json.JSONDecodeError, KeyError, TypeError):
        # Drop anything we can't parse — fixtures should be clean, known-good data.
        return None
    rec["raw"] = json.dumps(scrub(msg), separators=(",", ":"))
    return json.dumps(rec, separators=(",", ":"))


def detect_meta(control_path):
    """Pull firmware/model out of the (already-parsed) capture for the manifest."""
    fw_str = fw_int = model = None
    client = telescope = 0
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
            if rec.get("direction") == "client":
                client += 1
            else:
                telescope += 1
            r = msg.get("result")
            if isinstance(r, dict):
                d = r.get("device")
                if isinstance(d, dict) and d.get("firmware_ver_string"):
                    fw_str = d["firmware_ver_string"]
                    fw_int = d.get("firmware_ver_int")
                    model = d.get("product_model")
    return fw_str, fw_int, model, client, telescope


def main(argv=None):
    argv = argv or sys.argv[1:]
    if len(argv) != 2:
        print(__doc__)
        return 2
    src, dst = argv
    src_control = os.path.join(src, "control.jsonl")
    if not os.path.exists(src_control):
        print(f"no control.jsonl in {src}", file=sys.stderr)
        return 1

    os.makedirs(dst, exist_ok=True)
    out_lines = []
    for line in open(src_control):
        red = sanitize_line(line)
        if red is not None:
            out_lines.append(red)
    with open(os.path.join(dst, "control.jsonl"), "w") as fh:
        fh.write("\n".join(out_lines) + ("\n" if out_lines else ""))

    fw_str, fw_int, model, client, telescope = detect_meta(src_control)
    manifest = {
        "source_session": os.path.basename(os.path.normpath(src)),
        "telescope_model": model,
        "firmware_version": fw_str,
        "firmware_ver_int": fw_int,
        "client_message_count": client,
        "telescope_message_count": telescope,
        "sanitized": True,
        "note": "Secrets redacted by tools/sanitize_session.py. Do not restore raw values.",
    }
    with open(os.path.join(dst, "manifest.json"), "w") as fh:
        json.dump(manifest, fh, indent=2)
        fh.write("\n")
    print(f"{os.path.basename(dst)}: {client} client + {telescope} telescope msgs, "
          f"model={model} fw={fw_str} ({fw_int})")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
