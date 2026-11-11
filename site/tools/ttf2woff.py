#!/usr/bin/env python3
"""Wrap TrueType fonts as WOFF 1.0 (zlib per table) without any third-party package.

Why WOFF and not WOFF2: WOFF2 needs a Brotli encoder and the glyf transform, neither of
which ships with a stock Python. WOFF1 is a thin container every browser reads, and it
roughly halves a TTF, which is enough to keep the page light.

usage: ttf2woff.py SRC.ttf [SRC.ttf ...] --out DIR
"""
import struct
import sys
import zlib
from pathlib import Path


def ttf_to_woff(data: bytes) -> bytes:
    sfnt_version, num_tables = struct.unpack(">4sH", data[:6])
    tables = []
    for i in range(num_tables):
        tag, checksum, offset, length = struct.unpack(">4sIII", data[12 + 16 * i : 28 + 16 * i])
        tables.append((tag, checksum, data[offset : offset + length]))
    tables.sort(key=lambda t: t[0])  # WOFF requires tables in tag order

    header_size, entry_size = 44, 20
    offset = header_size + entry_size * len(tables)
    entries, blobs = [], []
    for tag, checksum, raw in tables:
        comp = zlib.compress(raw, 9)
        if len(comp) >= len(raw):  # the spec says to store a table as-is when zlib does not help
            comp = raw
        entries.append(struct.pack(">4sIIII", tag, offset, len(comp), len(raw), checksum))
        pad = (-len(comp)) % 4
        blobs.append(comp + b"\0" * pad)
        offset += len(comp) + pad
    total_sfnt = 12 + 16 * len(tables) + sum((len(r) + 3) & ~3 for _, _, r in tables)
    header = struct.pack(
        ">4sIIHHIHHIIIII",
        b"wOFF",
        int.from_bytes(sfnt_version, "big"),
        offset,
        len(tables),
        0,
        total_sfnt,
        1,
        0,
        0,
        0,
        0,
        0,
        0,
    )
    return header + b"".join(entries) + b"".join(blobs)


def main() -> None:
    args = sys.argv[1:]
    out = Path(args[args.index("--out") + 1])
    srcs = [Path(a) for a in args if a.endswith(".ttf")]
    out.mkdir(parents=True, exist_ok=True)
    for src in srcs:
        woff = ttf_to_woff(src.read_bytes())
        dst = out / (src.stem + ".woff")
        dst.write_bytes(woff)
        print(f"{src.name}: {src.stat().st_size} -> {len(woff)} bytes")


if __name__ == "__main__":
    main()
