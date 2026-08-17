"""Composite a keyed frame over a dark stage grey, so it can be looked at.

    PMG_FRAME=in.png PMG_DEST=out.png python3 tools/over_dark.py

A graphic destined for a key/fill pair is mostly transparent, and a PNG of one
opens as white-on-white in every viewer. This puts it over the sort of dark
stage it is actually keyed over.

**The renderer emits PREMULTIPLIED alpha** (ADR 0012 — composite in linear,
premultiplied, encode once at output), so the source is ADDED to the background
rather than mixed with it. Compositing it as straight alpha darkens every
antialiased edge and reads as a fringing bug in the renderer that is not there.

No Pillow: this repo has no Python dependencies and a viewing aid is not a
reason to acquire one. zlib and struct are enough for 8-bit RGBA.
"""

import os
import struct
import zlib

BG = (26, 28, 32)


def read_png(path):
    d = open(path, "rb").read()
    pos, w, h, idat = 8, None, None, b""
    while pos < len(d):
        ln = struct.unpack(">I", d[pos:pos + 4])[0]
        typ = d[pos + 4:pos + 8]
        if typ == b"IHDR":
            w, h, depth, colour = struct.unpack(">IIBB", d[pos + 8:pos + 18])
            if depth != 8 or colour != 6:
                raise SystemExit(f"{path}: expected 8-bit RGBA, got depth={depth} colour={colour}")
        elif typ == b"IDAT":
            idat += d[pos + 8:pos + 8 + ln]
        pos += 12 + ln

    raw = zlib.decompress(idat)
    out, prev, i = bytearray(), bytearray(w * 4), 0
    for _ in range(h):
        filt = raw[i]
        i += 1
        line = bytearray(raw[i:i + w * 4])
        i += w * 4
        # The five PNG filters, undone in place. Unrolling this is the whole
        # reason a dependency-free reader is short enough to be worth having.
        for x in range(len(line)):
            a = line[x - 4] if x >= 4 else 0
            b = prev[x]
            c = prev[x - 4] if x >= 4 else 0
            if filt == 1:
                line[x] = (line[x] + a) & 255
            elif filt == 2:
                line[x] = (line[x] + b) & 255
            elif filt == 3:
                line[x] = (line[x] + (a + b) // 2) & 255
            elif filt == 4:
                pa, pb, pc = abs(b - c), abs(a - c), abs(a + b - 2 * c)
                pr = a if (pa <= pb and pa <= pc) else (b if pb <= pc else c)
                line[x] = (line[x] + pr) & 255
        out += line
        prev = line
    return w, h, bytes(out)


def write_png(path, w, h, rgba):
    raw = b"".join(b"\x00" + rgba[y * w * 4:(y + 1) * w * 4] for y in range(h))

    def chunk(tag, data):
        return (struct.pack(">I", len(data)) + tag + data
                + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF))

    open(path, "wb").write(
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 6))
        + chunk(b"IEND", b"")
    )


def main():
    src = os.environ["PMG_FRAME"]
    dest = os.environ["PMG_DEST"]
    w, h, px = read_png(src)
    out = bytearray(w * h * 4)
    for i in range(0, len(px), 4):
        # Premultiplied: src + bg * (1 - a). Not a lerp.
        f = (255 - px[i + 3]) / 255
        out[i] = min(255, int(px[i] + BG[0] * f))
        out[i + 1] = min(255, int(px[i + 1] + BG[1] * f))
        out[i + 2] = min(255, int(px[i + 2] + BG[2] * f))
        out[i + 3] = 255
    write_png(dest, w, h, bytes(out))


main()
