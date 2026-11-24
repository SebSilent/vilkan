#!/usr/bin/env python3
"""Minimal PNG decoder for presented-frame captures (tracked, clean-room).

Decodes the 8-bit RGB (colour type 2), filter-0-per-row PNGs written by the
capture path. Pure standard library; no game data, no title-derived constants.
The world-region rectangle is supplied by the caller (external config for real
runs, a structural fraction for the synthetic suite) — never stored here.
"""
import struct
import zlib
from pathlib import Path


def decode(path):
    """Return (width, height, rows) where rows is a list of raw RGB byte rows."""
    data = Path(path).read_bytes()
    assert data[:8] == b"\x89PNG\r\n\x1a\n", path
    pos, idat, w, h = 8, [], None, None
    while pos < len(data):
        (ln,) = struct.unpack(">I", data[pos:pos + 4])
        kind = data[pos + 4:pos + 8]
        body = data[pos + 8:pos + 8 + ln]
        if kind == b"IHDR":
            w, h, depth, ctype = struct.unpack(">IIBB", body[:10])
            assert (depth, ctype) == (8, 2), (depth, ctype)
        elif kind == b"IDAT":
            idat.append(body)
        pos += 12 + ln
    raw = zlib.decompress(b"".join(idat))
    stride = w * 3 + 1
    assert len(raw) == stride * h, (len(raw), stride * h)
    rows = []
    for y in range(h):
        row = raw[y * stride:(y + 1) * stride]
        assert row[0] == 0, f"filter {row[0]} != 0"
        rows.append(row[1:])
    return w, h, rows