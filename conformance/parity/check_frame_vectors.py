"""Check conformance/vectors/frame_headers.json against an independent decoder.

The decoder is ``struct.unpack`` over the header layout documented in
zmerlynn/seestar-spec (``spec/media/image-sockets.md``, "Reply framing"). It
shares no code with the Rust parser in
``crates/scopinator-seestar/src/protocol/frame.rs``, whose unit test reads the
same vector file. Standard library only. Exit 0 on success.

Run: python3 conformance/parity/check_frame_vectors.py
Regenerate the expected values after adding a vector:
     python3 conformance/parity/check_frame_vectors.py --write
"""

from __future__ import annotations

import json
import struct
import sys
from pathlib import Path
from typing import Any

VECTORS = Path(__file__).resolve().parents[1] / "vectors" / "frame_headers.json"
HEADER_SIZE = 80
MAGIC = 0x03C3
IMG_TYPE_JPEG = 4

# magic, version, header length, payload length, first payload byte, quirk
# byte, reserved, imgType, dataType, request id, width, height, hfdX, hfdY,
# hfd, canDebayer, image id (signed).
LAYOUT = ">HHHIBBBBBBHHHHHHh"


def payload_kind(data_type: int, img_type: int) -> str:
    if data_type == 0:
        return "text"
    if data_type == 1:
        return "error_text"
    if data_type in (2, 3) and img_type == IMG_TYPE_JPEG:
        return "jpeg"
    if data_type == 2:
        return "raw"
    if data_type == 3:
        return "zip"
    return "unknown"


def decode(raw: bytes) -> dict[str, Any]:
    (
        magic, version, header_len, size, _first, _quirk, _reserved,
        img_type, data_type, request_id, width, height,
        hfd_x, hfd_y, hfd, can_debayer, image_id,
    ) = struct.unpack(LAYOUT, raw[: struct.calcsize(LAYOUT)])
    return {
        "magic": magic,
        "version": version,
        "header_len": header_len,
        "size": size,
        "img_type": img_type,
        "data_type": data_type,
        "request_id": request_id,
        "width": width,
        "height": height,
        "hfd_x": hfd_x,
        "hfd_y": hfd_y,
        "hfd": hfd,
        "can_debayer": can_debayer,
        "image_id": image_id,
        "valid_magic": magic == MAGIC,
        "payload_kind": payload_kind(data_type, img_type),
    }


def main() -> int:
    write = "--write" in sys.argv[1:]
    doc = json.loads(VECTORS.read_text())
    failures = 0
    for v in doc["vectors"]:
        raw = bytes.fromhex(v["header_hex"])
        if len(raw) != HEADER_SIZE:
            print(f"FAIL {v['name']}: header is {len(raw)} bytes")
            failures += 1
            continue
        got = decode(raw)
        if write:
            v["expected"] = got
        elif got != v["expected"]:
            print(f"FAIL {v['name']}: got {got}, expected {v['expected']}")
            failures += 1
    if write:
        VECTORS.write_text(json.dumps(doc, indent=2) + "\n")
        print(f"wrote {len(doc['vectors'])} vectors")
        return 0
    print(f"{len(doc['vectors']) - failures}/{len(doc['vectors'])} vectors ok")
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
