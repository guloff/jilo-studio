#!/usr/bin/env python3
"""Pack the Windows icon (assets/brand/jilo/jilo-studio.ico) from the macOS icon set.

The .icns already holds every size drawn from jilo-studio.svg's geometry (packaging/macos/icon/) as
PNG; Windows (Vista and later) takes PNG images inside an .ico, so the 32, 64, 128 and 256 px
images go in unchanged and Windows scales the smaller sizes from them. Standard library only.

    python packaging/windows/make_ico.py
"""
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
ICNS = ROOT / "assets/brand/jilo/jilo-studio.icns"
ICO = ROOT / "assets/brand/jilo/jilo-studio.ico"
SIZES = (256, 128, 64, 32)
PNG = b"\x89PNG\r\n\x1a\n"


def icns_pngs(data: bytes) -> dict[int, bytes]:
    if data[:4] != b"icns":
        raise SystemExit(f"{ICNS} is not an icns file")
    pngs, i = {}, 8
    while i + 8 <= len(data):
        n = struct.unpack(">I", data[i + 4 : i + 8])[0]
        body = data[i + 8 : i + n]
        if body[:8] == PNG:
            w, h = struct.unpack(">II", body[16:24])
            if w == h:
                pngs.setdefault(w, body)
        i += n
    return pngs


def main() -> None:
    pngs = icns_pngs(ICNS.read_bytes())
    missing = [s for s in SIZES if s not in pngs]
    if missing:
        raise SystemExit(f"{ICNS} has no PNG for {missing}")
    images = [(s, pngs[s]) for s in SIZES]
    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for size, png in images:
        dim = 0 if size >= 256 else size  # 0 means 256 in an ICONDIRENTRY
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(png), offset)
        offset += len(png)
    for _, png in images:
        out += png
    ICO.write_bytes(bytes(out))
    print(f"wrote {ICO.relative_to(ROOT)} ({len(out)} bytes, sizes {', '.join(map(str, SIZES))})")


if __name__ == "__main__":
    main()
