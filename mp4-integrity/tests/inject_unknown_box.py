#!/usr/bin/env python3
"""Inject a private, well-formed top-level box into an ISO-BMFF file."""

import struct
import sys


def top_level_boxes(data: bytes):
    offset = 0
    while offset + 8 <= len(data):
        size = struct.unpack(">I", data[offset:offset + 4])[0]
        box_type = data[offset + 4:offset + 8]
        if size == 1:  # 64-bit extended size
            size = struct.unpack(">Q", data[offset + 8:offset + 16])[0]
        elif size == 0:
            size = len(data) - offset
        yield offset, size, box_type
        offset += size


def main() -> int:
    src, dst, fourcc = sys.argv[1], sys.argv[2], sys.argv[3].encode("ascii")
    if len(fourcc) != 4:
        print("fourcc must be exactly 4 bytes", file=sys.stderr)
        return 1

    data = open(src, "rb").read()
    insert_at = len(data)
    for offset, _size, box_type in top_level_boxes(data):
        if box_type == b"moov":
            insert_at = offset
            break

    payload = b"private integrity-test marker"
    unknown = struct.pack(">I4s", 8 + len(payload), fourcc) + payload
    with open(dst, "wb") as fh:
        fh.write(data[:insert_at])
        fh.write(unknown)
        fh.write(data[insert_at:])
    print(f"injected {fourcc.decode()} box ({len(unknown)} bytes) at offset {insert_at}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
