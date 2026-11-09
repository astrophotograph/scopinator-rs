"""Check conformance/vectors/frame_headers.json against the Python wire format.

Python oracle: ``struct.unpack(">HHHIHHBBHH", header[:20])`` as used by
pyscopinator's imaging client. Standard library only. Exit 0 on success.

Forge task: "Add shared frame header differential vectors".
Run: uv --directory ../pyscopinator run python conformance/parity/check_frame_vectors.py
(or plain ``python3 conformance/parity/check_frame_vectors.py`` from the repo root).
"""

from __future__ import annotations

import json
import struct
import sys
from pathlib import Path

VECTORS = Path(__file__).resolve().parents[1] / "vectors" / "frame_headers.json"
HEADER_SIZE = 80


def main() -> int:
    doc = json.loads(VECTORS.read_text())
    failures = 0
    for v in doc["vectors"]:
        raw = bytes.fromhex(v["header_hex"])
        if len(raw) != HEADER_SIZE:
            print(f"FAIL {v['name']}: header is {len(raw)} bytes")
            failures += 1
            continue
        _, _, _, size, _, _, code, id_, width, height = struct.unpack(">HHHIHHBBHH", raw[:20])
        got = {"size": size, "code": code, "id": id_, "width": width, "height": height}
        if got != v["expected"]:
            print(f"FAIL {v['name']}: got {got}, expected {v['expected']}")
            failures += 1
    print(f"{len(doc['vectors']) - failures}/{len(doc['vectors'])} vectors ok")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
