#!/usr/bin/env python3
"""Generate the app's Windows icon, `crates/gazelle-audio-server/assets/gazelle.ico`.

The design is the one the tray and the window already draw at run time
(`crates/gazelle-audio-server/src/icon.rs`): a light "G" — a ring open on the upper right
plus a bar — on a dark grey disc. **This script is a port of that function, arithmetic for
arithmetic**, so the shipped `.ico` and the drawn icon cannot drift apart: a Rust test
(`tests/icon.rs`) decodes the committed file and asserts every pixel equals `icon::rgba`.

Design cues only. Nothing here is taken from Antelope's artwork.

Why a script and not a hand-drawn file: a binary in the tree with no provenance is a thing
nobody can change. Run this, commit what it writes, and the source is the source.

Usage:

    python refs/tools/scripts/make_icon.py                     # write the committed path
    python refs/tools/scripts/make_icon.py --out some.ico
    python refs/tools/scripts/make_icon.py --check             # exit 1 if the file is stale

Deterministic: the same bytes every run, on any machine, with no third-party module. There is
no PNG or image library here — 16, 32 and 48 are BMP DIBs, which the format takes raw, and 256
is a PNG written with `zlib` from the standard library.
"""

import argparse
import binascii
import math
import os
import struct
import sys
import zlib

# The sizes Windows asks for: the small icon (16), the shell's list and taskbar (32), the
# large icon (48) and the extra-large/jumbo tile Explorer scales from (256).
SIZES = (16, 32, 48, 256)

# The two colours, as `icon.rs` has them.
DISC = (0x2B, 0x2D, 0x31)
MARK = (0xE8, 0xA8, 0x38)
SAMPLES = 4


def f32(value):
    """Round to f32, so this matches `icon.rs` operation for operation rather than nearly."""
    return struct.unpack("<f", struct.pack("<f", value))[0]


# The thresholds, rounded to f32 first: `icon.rs` compares f32 values against f32 literals, and
# comparing an f32 against the f64 spelling of the same decimal is a different test.
EDGE = f32(0.97)
RING = (f32(0.40), f32(0.66))
BAR_Y = (f32(-0.06), f32(0.12))
BAR_X = (f32(0.05), f32(0.66))
OPEN = (f32(0.0), f32(60.0))


def rgba(size):
    """RGBA, top row first, antialiased by sampling each pixel 4x4 — `icon::rgba` in Python."""
    out = []
    for py in range(size):
        for px in range(size):
            disc = 0.0
            mark = 0.0
            for sy in range(SAMPLES):
                for sx in range(SAMPLES):
                    # -1..1 across the icon, y up.
                    x = f32(f32(f32(f32(px + f32(f32(sx + 0.5) / SAMPLES)) / size) * 2.0) - 1.0)
                    y = f32(1.0 - f32(f32(f32(py + f32(f32(sy + 0.5) / SAMPLES)) / size) * 2.0))
                    r = f32(f32(f32(x * x) + f32(y * y)) ** 0.5)
                    if r <= EDGE:
                        disc = f32(disc + 1.0)
                        angle = f32(degrees(atan2(y, x)))
                        # A ring open on the right above the bar, plus the bar itself.
                        ring = RING[0] <= r <= RING[1] and not (OPEN[0] <= angle < OPEN[1])
                        bar = BAR_Y[0] <= y <= BAR_Y[1] and BAR_X[0] <= x <= BAR_X[1]
                        if ring or bar:
                            mark = f32(mark + 1.0)
            n = float(SAMPLES * SAMPLES)
            coverage = f32(disc / n)
            m = f32(mark / disc) if disc > 0.0 else 0.0
            pixel = [round_half_away(f32(DISC[i] + f32(f32(MARK[i] - DISC[i]) * m))) for i in range(3)]
            pixel.append(round_half_away(f32(coverage * 255.0)))
            out.append(tuple(pixel))
    return out


def atan2(y, x):
    """`f32::atan2`: the f64 result rounded to f32, which is what Rust's implementation gives."""
    return f32(math.atan2(y, x))


def degrees(radians):
    return f32(math.degrees(radians))


def round_half_away(value):
    """Rust's `f32::round`: halfway cases away from zero, not Python's banker's rounding."""
    return int(math.floor(value + 0.5)) if value >= 0 else int(math.ceil(value - 0.5))


# ---------------------------------------------------------------------------------------------
# The two image encodings an .ico may hold
# ---------------------------------------------------------------------------------------------


def bmp(size, pixels):
    """A 32-bit BITMAPINFOHEADER DIB: bottom-up BGRA, followed by the (all-zero) AND mask.

    The mask is meaningless for a 32-bit image — the alpha channel is the mask — but the format
    still reserves it, and a reader that ignores alpha would otherwise show the whole square.
    """
    header = struct.pack(
        "<IiiHHIIiiII",
        40,  # biSize
        size,
        size * 2,  # biHeight counts the colour rows and the mask rows
        1,  # biPlanes
        32,  # biBitCount
        0,  # BI_RGB
        size * size * 4,
        0,
        0,
        0,
        0,
    )
    body = bytearray()
    for y in range(size - 1, -1, -1):
        for x in range(size):
            r, g, b, a = pixels[y * size + x]
            body += bytes((b, g, r, a))
    # The AND mask: 1 bit per pixel, each row padded to 4 bytes. All zero — "use the colour".
    stride = ((size + 31) // 32) * 4
    body += bytes(stride * size)
    return bytes(header) + bytes(body)


def png(size, pixels):
    """A minimal 8-bit RGBA PNG. 256x256 entries are conventionally stored this way."""

    def chunk(kind, payload):
        return struct.pack(">I", len(payload)) + kind + payload + struct.pack(">I", binascii.crc32(kind + payload) & 0xFFFFFFFF)

    raw = bytearray()
    for y in range(size):
        raw.append(0)  # filter type 0 (None), so the bytes are the pixels
        for x in range(size):
            raw += bytes(pixels[y * size + x])
    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)
    # Fixed level and no timestamp, so two runs give identical bytes.
    idat = zlib.compress(bytes(raw), 9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def ico(sizes=SIZES):
    """The whole `.ico`: a directory of entries, then each image."""
    images = []
    for size in sizes:
        pixels = rgba(size)
        images.append((size, png(size, pixels) if size >= 256 else bmp(size, pixels)))

    out = bytearray(struct.pack("<HHH", 0, 1, len(images)))
    offset = 6 + 16 * len(images)
    for size, body in images:
        out += struct.pack(
            "<BBBBHHII",
            0 if size >= 256 else size,  # 256 is written as 0; the format has one byte
            0 if size >= 256 else size,
            0,  # no palette
            0,
            1,  # planes
            32,  # bits per pixel
            len(body),
            offset,
        )
        offset += len(body)
    for _, body in images:
        out += body
    return bytes(out)


def main():
    here = os.path.dirname(os.path.abspath(__file__))
    default = os.path.normpath(os.path.join(here, "..", "..", "..", "crates", "gazelle-audio-server", "assets", "gazelle.ico"))

    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--out", default=default, help="where to write the icon (default: the committed path)")
    parser.add_argument("--check", action="store_true", help="do not write; exit 1 if --out differs from what this would write")
    args = parser.parse_args()

    body = ico()
    if args.check:
        try:
            with open(args.out, "rb") as f:
                current = f.read()
        except FileNotFoundError:
            print(f"{args.out} is missing; run this script without --check", file=sys.stderr)
            return 1
        if current != body:
            print(f"{args.out} is not what this script writes ({len(current)} bytes on disk, {len(body)} generated)", file=sys.stderr)
            return 1
        print(f"{args.out} is up to date ({len(body)} bytes)")
        return 0

    os.makedirs(os.path.dirname(args.out), exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(body)
    print(f"wrote {args.out}: {len(body)} bytes, sizes {', '.join(str(s) for s in SIZES)}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
