#!/usr/bin/env python3
"""Emit a normalized conformance report for the session corpus, as pyscopinator
sees it. Mirror of the Rust `conformance_report` example; `compare.py` diffs the
two to verify scopinator-rs and pyscopinator stay in parity.

Run inside pyscopinator's environment:

    cd ~/Projects/erewhon/pyscopinator
    uv run python ~/Projects/erewhon/scopinator-rs/conformance/parity/pyscopinator_report.py \
        [<sessions_dir>] > rust_or_py_report.json

Schema (version 1) — identical to the Rust generator:
  { "schema": 1, "impl": "pyscopinator",
    "sessions": [ { "session": str, "firmware_int": int|null,
                    "messages": [ <obs>, ... ] } ] }
  client:    { "i", "dir": "client", "method", "modeled" }
  telescope: { "i", "dir": "telescope", "class", "parse_ok",
               "event"?, "method"?, "id"?, "code"? }

Classification is structural (matching the Rust side) so the comparison isolates
the genuine per-library signals: `modeled` (is the command typed?) and
`parse_ok` (does the library's typed model accept the message?).
"""
from __future__ import annotations

import glob
import inspect
import json
import os
import sys


def pyscopinator_command_methods():
    """Set of method strings pyscopinator models as typed command classes."""
    from scopinator.seestar.commands import parameterized, settings, simple
    from scopinator.seestar.commands.common import BaseCommand

    methods = set()
    for mod in (simple, parameterized, settings):
        for _name, obj in inspect.getmembers(mod, inspect.isclass):
            if issubclass(obj, BaseCommand) and obj is not BaseCommand:
                field = obj.model_fields.get("method")
                if field is not None and isinstance(field.default, str):
                    methods.add(field.default)
    return methods


def make_event_validator():
    """Return a fn(dict)->bool: does pyscopinator parse this event payload?"""
    import pydantic
    from scopinator.seestar.events import EventTypes

    adapter = pydantic.TypeAdapter(EventTypes)

    def parse(d):
        try:
            adapter.validate_python(d)
            return True
        except Exception:
            return False

    return parse


def make_response_validator():
    """Return a fn(dict)->bool: does pyscopinator parse this response payload?"""
    from scopinator.seestar.commands.common import CommandResponse

    def parse(d):
        try:
            CommandResponse(**d)
            return True
        except Exception:
            return False

    return parse


def is_event(msg):
    return "Event" in msg


def is_response(msg):
    return "id" in msg and ("code" in msg or "result" in msg)


def main(argv=None):
    argv = argv or sys.argv[1:]
    if argv:
        corpus = argv[0]
    else:
        here = os.path.dirname(os.path.abspath(__file__))
        corpus = os.path.normpath(os.path.join(here, "..", "sessions"))

    modeled = pyscopinator_command_methods()
    parse_event = make_event_validator()
    parse_response = make_response_validator()

    sessions = []
    for d in sorted(glob.glob(os.path.join(corpus, "*"))):
        control = os.path.join(d, "control.jsonl")
        if not os.path.isfile(control):
            continue
        messages = []
        firmware_int = None
        with open(control) as fh:
            for i, line in enumerate(l for l in fh if l.strip()):
                rec = json.loads(line)
                msg = json.loads(rec["raw"])
                if firmware_int is None:
                    res = msg.get("result")
                    if isinstance(res, dict):
                        dev = res.get("device")
                        if isinstance(dev, dict):
                            firmware_int = dev.get("firmware_ver_int")

                if rec.get("direction") == "client":
                    method = msg.get("method", "")
                    messages.append({
                        "i": i,
                        "dir": "client",
                        "method": method,
                        "modeled": method in modeled,
                    })
                elif is_event(msg):
                    messages.append({
                        "i": i,
                        "dir": "telescope",
                        "class": "event",
                        "event": msg.get("Event"),
                        "parse_ok": parse_event(msg),
                    })
                elif is_response(msg):
                    messages.append({
                        "i": i,
                        "dir": "telescope",
                        "class": "response",
                        "method": msg.get("method"),
                        "id": msg.get("id"),
                        "code": msg.get("code"),
                        "parse_ok": parse_response(msg),
                    })
                else:
                    messages.append({
                        "i": i, "dir": "telescope", "class": "unknown", "parse_ok": False,
                    })

        sessions.append({
            "session": os.path.basename(d),
            "firmware_int": firmware_int,
            "messages": messages,
        })

    json.dump({"schema": 1, "impl": "pyscopinator", "sessions": sessions},
              sys.stdout, indent=2)
    sys.stdout.write("\n")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
