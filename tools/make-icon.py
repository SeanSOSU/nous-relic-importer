"""Draws assets/icon.ico: the tray's teal disc with a white centre, at the sizes Windows asks for.

Usage: python tools/make-icon.py
"""
import struct
import zlib
from pathlib import Path

TEAL = (0x19, 0x9E, 0x70)


def pixels(size):
    """RGBA rows, top to bottom, with soft edges."""
    c = size / 2
    outer, inner = size * 14.5 / 32, size * 5.5 / 32
    rows = []
    for y in range(size):
        row = []
        for x in range(size):
            d = ((x + 0.5 - c) ** 2 + (y + 0.5 - c) ** 2) ** 0.5
            a = min(max(outer - d, 0.0), 1.0)
            w = min(max(inner - d, 0.0), 1.0)
            r, g, b = (round(ch * (1 - w) + 255 * w) for ch in TEAL)
            row.append((r, g, b, round(a * 255)))
        rows.append(row)
    return rows


def png(size):
    raw = b"".join(b"\0" + bytes(v for px in row for v in px) for row in pixels(size))
    chunk = lambda kind, data: struct.pack(">I", len(data)) + kind + data + struct.pack(">I", zlib.crc32(kind + data))
    header = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", header) + chunk(b"IDAT", zlib.compress(raw, 9)) + chunk(b"IEND", b"")


def dib(size):
    """32-bit BGRA bitmap, bottom-up, with the 1-bit mask an icon resource still carries."""
    header = struct.pack("<IiiHHIIiiII", 40, size, size * 2, 1, 32, 0, 0, 0, 0, 0, 0)
    colour = b"".join(bytes((b, g, r, a)) for row in reversed(pixels(size)) for (r, g, b, a) in row)
    mask_row = ((size + 31) // 32) * 4
    return header + colour + bytes(mask_row * size)


def main():
    images = [(s, dib(s)) for s in (16, 20, 24, 32, 40, 48, 64)] + [(256, png(256))]
    out = struct.pack("<HHH", 0, 1, len(images))
    offset = 6 + 16 * len(images)
    for size, data in images:
        dim = 0 if size == 256 else size
        out += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
    out += b"".join(data for _, data in images)
    target = Path(__file__).resolve().parent.parent / "assets" / "icon.ico"
    target.parent.mkdir(exist_ok=True)
    target.write_bytes(out)
    print(f"wrote {target} ({len(out)} bytes)")


if __name__ == "__main__":
    main()
