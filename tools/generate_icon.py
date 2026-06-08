#!/usr/bin/env python3
"""Generate assets/justquery.ico — a clay rounded square with a white "JQ" monogram.
Pure stdlib (zlib + struct + math). Antialiased via supersampling. Run from repo root:
    python tools/generate_icon.py
Also writes justquery_preview.png (gitignored) at 256px for eyeballing the result.
"""
import zlib, struct, os, math

ACCENT = (0xC9, 0x64, 0x42)  # Claude clay
WHITE = (0xFF, 0xFF, 0xFF)
SIZES = [16, 32, 48, 64, 128, 256]
SS = 4  # supersampling factor for antialiasing


def inside_rounded(x, y, x0, y0, x1, y1, r):
    cx = min(max(x, x0 + r), x1 - r)
    cy = min(max(y, y0 + r), y1 - r)
    dx, dy = x - cx, y - cy
    return dx * dx + dy * dy <= r * r


def dist_seg(px, py, ax, ay, bx, by):
    dx, dy = bx - ax, by - ay
    L2 = dx * dx + dy * dy
    if L2 == 0.0:
        return math.hypot(px - ax, py - ay)
    t = ((px - ax) * dx + (py - ay) * dy) / L2
    t = max(0.0, min(1.0, t))
    return math.hypot(px - (ax + t * dx), py - (ay + t * dy))


# Glyphs in normalised [0,1] square coordinates. The "J" is a polyline (top bar,
# stem, bottom hook); the "Q" is an annulus ring plus a short diagonal tail.
J_PTS = [
    (0.27, 0.30), (0.46, 0.30),                          # top bar
    (0.43, 0.30), (0.43, 0.60),                          # stem
    (0.43, 0.635), (0.415, 0.685), (0.375, 0.715),       # hook
    (0.325, 0.722), (0.275, 0.700), (0.255, 0.655),
]
J_HW = 0.046

Q_CX, Q_CY = 0.66, 0.50
Q_ROUT, Q_RIN = 0.165, 0.075
Q_TAIL = [(0.685, 0.585), (0.795, 0.715)]
Q_HW = 0.046


def in_letters(fx, fy, s):
    x, y = fx / s, fy / s  # back to normalised
    # J polyline
    for i in range(len(J_PTS) - 1):
        ax, ay = J_PTS[i]
        bx, by = J_PTS[i + 1]
        if dist_seg(x, y, ax, ay, bx, by) <= J_HW:
            return True
    # Q ring
    d = math.hypot(x - Q_CX, y - Q_CY)
    if Q_RIN <= d <= Q_ROUT:
        return True
    # Q tail
    if dist_seg(x, y, Q_TAIL[0][0], Q_TAIL[0][1], Q_TAIL[1][0], Q_TAIL[1][1]) <= Q_HW:
        return True
    return False


def make_rgba(size):
    s = float(size)
    margin = s * 0.06
    x0, y0, x1, y1 = margin, margin, s - margin, s - margin
    r = (x1 - x0) * 0.22
    px = bytearray(size * size * 4)
    inv = 1.0 / SS
    samples = SS * SS
    for yy in range(size):
        for xx in range(size):
            sq = 0
            wt = 0
            for sy in range(SS):
                fy = yy + (sy + 0.5) * inv
                for sx in range(SS):
                    fx = xx + (sx + 0.5) * inv
                    if inside_rounded(fx, fy, x0, y0, x1, y1, r):
                        sq += 1
                        if in_letters(fx, fy, s):
                            wt += 1
            a = (255 * sq) // samples
            if sq > 0:
                f = wt / sq  # white coverage within the square
                rr = round(ACCENT[0] + (WHITE[0] - ACCENT[0]) * f)
                gg = round(ACCENT[1] + (WHITE[1] - ACCENT[1]) * f)
                bb = round(ACCENT[2] + (WHITE[2] - ACCENT[2]) * f)
            else:
                rr, gg, bb = ACCENT
            i = (yy * size + xx) * 4
            px[i], px[i + 1], px[i + 2], px[i + 3] = rr, gg, bb, a
    return bytes(px)


def png_bytes(size, rgba):
    def chunk(typ, data):
        return (
            struct.pack(">I", len(data))
            + typ
            + data
            + struct.pack(">I", zlib.crc32(typ + data) & 0xFFFFFFFF)
        )

    ihdr = struct.pack(">IIBBBBB", size, size, 8, 6, 0, 0, 0)  # RGBA, 8-bit
    raw = bytearray()
    row = size * 4
    for y in range(size):
        raw.append(0)  # filter: none
        raw += rgba[y * row:(y + 1) * row]
    idat = zlib.compress(bytes(raw), 9)
    return b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", ihdr) + chunk(b"IDAT", idat) + chunk(b"IEND", b"")


def main():
    rendered = {s: make_rgba(s) for s in SIZES}
    pngs = [(s, png_bytes(s, rendered[s])) for s in SIZES]
    header = struct.pack("<HHH", 0, 1, len(pngs))
    offset = 6 + 16 * len(pngs)
    entries, datas = b"", b""
    for s, data in pngs:
        dim = 0 if s >= 256 else s
        entries += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
        offset += len(data)
        datas += data
    os.makedirs("assets", exist_ok=True)
    with open("assets/justquery.ico", "wb") as f:
        f.write(header + entries + datas)
    print("wrote assets/justquery.ico", os.path.getsize("assets/justquery.ico"), "bytes")
    # preview (gitignored) for visual inspection
    with open("justquery_preview.png", "wb") as f:
        f.write(png_bytes(256, rendered[256]))
    print("wrote justquery_preview.png")


if __name__ == "__main__":
    main()
