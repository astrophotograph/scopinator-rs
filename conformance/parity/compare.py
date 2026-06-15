#!/usr/bin/env python3
# /// script
# requires-python = ">=3.9"
# dependencies = []
# ///
"""Compare two conformance reports and surface parity divergences.

Both scopinator-rs and pyscopinator replay the shared session corpus and emit a
normalized report (see `pyscopinator_report.py` / the Rust `conformance_report`
example). This tool aligns the two reports message-by-message and reports where
the implementations disagree on:

  * `class`    — how a telescope message is classified (event/response/unknown)
  * `parse_ok` — whether the library's typed model accepts the message
  * `modeled`  — whether the library models a given client command
  * `event` / `method` / `id` / `code` — message identity (alignment sanity)

`modeled` divergences are expected when the libraries simply support different
command sets; they are reported separately (as "capability gaps") from hard
divergences (parse failures, misclassification) which indicate a real bug or
drift in one implementation.

Usage:
    # diff two pre-generated reports
    python3 compare.py rust.json pyscopinator.json

    # or generate both and diff in one shot (needs cargo + pyscopinator/uv)
    python3 compare.py --run

Exit code is non-zero if any HARD divergence is found (capability gaps alone do
not fail, but are printed).
"""
from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys

REPO_ROOT = os.path.normpath(os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", ".."))
PYSCOPINATOR_DIR = os.path.normpath(os.path.join(REPO_ROOT, "..", "pyscopinator"))


def load(path):
    with open(path) as fh:
        return json.load(fh)


def index_sessions(report):
    return {s["session"]: s for s in report["sessions"]}


def compare(rust, py):
    """Return (hard_divergences, capability_gaps) as lists of human strings."""
    hard, gaps = [], []
    rsess, psess = index_sessions(rust), index_sessions(py)

    only_rust = sorted(set(rsess) - set(psess))
    only_py = sorted(set(psess) - set(rsess))
    for s in only_rust:
        hard.append(f"session {s!r} only in {rust['impl']}")
    for s in only_py:
        hard.append(f"session {s!r} only in {py['impl']}")

    for name in sorted(set(rsess) & set(psess)):
        rmsgs = {m["i"]: m for m in rsess[name]["messages"]}
        pmsgs = {m["i"]: m for m in psess[name]["messages"]}
        if rsess[name].get("firmware_int") != psess[name].get("firmware_int"):
            hard.append(f"{name}: firmware_int differs "
                        f"({rsess[name].get('firmware_int')} vs {psess[name].get('firmware_int')})")
        for i in sorted(set(rmsgs) & set(pmsgs)):
            r, p = rmsgs[i], pmsgs[i]
            loc = f"{name}#{i}"
            if r["dir"] != p["dir"]:
                hard.append(f"{loc}: direction differs ({r['dir']} vs {p['dir']})")
                continue
            if r["dir"] == "client":
                if r.get("method") != p.get("method"):
                    hard.append(f"{loc}: client method differs "
                                f"({r.get('method')!r} vs {p.get('method')!r})")
                elif r.get("modeled") != p.get("modeled"):
                    only = rust["impl"] if r["modeled"] else py["impl"]
                    gaps.append((r.get("method"), only))
            else:  # telescope
                for key in ("class", "event", "method", "id", "code"):
                    if r.get(key) != p.get(key):
                        # event/method/id/code mismatches are alignment issues;
                        # class mismatch is a real classification divergence.
                        hard.append(f"{loc}: {key} differs ({r.get(key)!r} vs {p.get(key)!r})")
                if r.get("parse_ok") != p.get("parse_ok"):
                    ok_impl = rust["impl"] if r["parse_ok"] else py["impl"]
                    bad_impl = py["impl"] if r["parse_ok"] else rust["impl"]
                    ident = r.get("event") or r.get("method") or "?"
                    hard.append(f"{loc}: {ident!r} parses in {ok_impl} but FAILS in {bad_impl}")
    return hard, gaps


def generate_reports():
    rust_path = os.path.join(REPO_ROOT, "conformance", "reports", "rust.json")
    py_path = os.path.join(REPO_ROOT, "conformance", "reports", "pyscopinator.json")
    os.makedirs(os.path.dirname(rust_path), exist_ok=True)

    print("generating scopinator-rs report…", file=sys.stderr)
    with open(rust_path, "w") as fh:
        subprocess.run(
            ["cargo", "run", "-q", "-p", "scopinator-seestar", "--example", "conformance_report"],
            cwd=REPO_ROOT, stdout=fh, check=True,
        )
    print("generating pyscopinator report…", file=sys.stderr)
    script = os.path.join(REPO_ROOT, "conformance", "parity", "pyscopinator_report.py")
    corpus = os.path.join(REPO_ROOT, "conformance", "sessions")
    with open(py_path, "w") as fh:
        subprocess.run(
            ["uv", "run", "python", script, corpus],
            cwd=PYSCOPINATOR_DIR, stdout=fh, check=True,
        )
    return rust_path, py_path


def main(argv=None):
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("reports", nargs="*", help="two report JSON files (rust, pyscopinator)")
    ap.add_argument("--run", action="store_true", help="generate both reports first")
    args = ap.parse_args(argv)

    if args.run:
        rust_path, py_path = generate_reports()
    elif len(args.reports) == 2:
        rust_path, py_path = args.reports
    else:
        ap.error("provide two report files or --run")

    rust, py = load(rust_path), load(py_path)
    # Normalize which is which by the "impl" field.
    if rust.get("impl") == "pyscopinator":
        rust, py = py, rust

    hard, gaps = compare(rust, py)

    # Collapse per-occurrence gaps to distinct (command, impl) pairs.
    gap_set = sorted(set(gaps))
    if gap_set:
        print(f"\n=== capability gaps ({len(gap_set)}) — libraries model different commands ===")
        for method, only in gap_set:
            print(f"  • command {method!r} modeled by {only} only")
    if hard:
        print(f"\n=== HARD divergences ({len(hard)}) — parse/classification disagreements ===")
        for h in hard[:200]:
            print(f"  ✗ {h}")
        if len(hard) > 200:
            print(f"  … and {len(hard) - 200} more")
        print("\nPARITY: FAIL")
        return 1

    print(f"\nPARITY: OK  (capability gaps: {len(set(gaps))}, hard divergences: 0)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
