#!/usr/bin/env python3
"""Generate bridgewatch's tray icons and its app icon.

Why this is hand-rolled rather than Pillow or an SVG toolchain: every one of
those is a dependency a contributor would have to acquire before they could
change an icon, and none of them is reliably present. Pillow is not in the
standard library and `pip install` is refused outright on a PEP 668 distribution;
`rsvg-convert` and ImageMagick are separate installs; macOS `qlmanage`
rasterises SVG only at sizes it chooses. What is left is the standard library,
which is enough: `zlib` writes the PNG and the shapes are supersampled coverage
masks. It also makes `--check` meaningful in CI, where none of the above exists
either.

    python3 src-tauri/icons/generate.py          # rewrite every committed PNG
    python3 src-tauri/icons/generate.py --check  # fail if they would change

The outputs are committed. This script exists so the shapes are editable, not
because the build runs it.

⛔ The app icon is written as ``icon-source.png``, not ``icon.png``. ``npx tauri
icon`` OVERWRITES ``icons/icon.png`` with a 512px copy of whatever it was given,
so a 1024px master living under that name is destroyed by the very command that
consumes it — and the next run would then downscale a downscale. The second step
is therefore:

    npx tauri icon src-tauri/icons/icon-source.png

which produces ``icon.png`` (512), ``icon.icns``, ``icon.ico`` and the PNG sizes
``tauri.conf.json`` names. ``--check`` covers only what THIS script writes.

Two sets are produced:

* ``tray/template/`` — black on transparent, shape only. macOS renders a tray
  image with ``icon_as_template(true)`` by using its ALPHA only, inverting for
  dark menu bars, so a coloured template image would be silently discarded.
* ``tray/color/``    — the same shapes filled red / amber / green / blue / grey,
  for Linux's AppIndicator, which has no template concept.

Each set holds one file per icon state (``IconState::as_str()``, which the core
documents as the icon file's stem) plus one per symbol name, because
``[icon].states`` remaps a state onto a symbol: ``states = { failed = "octagon" }``
resolves ``octagon.png`` and it has to exist.

Sizes: ``<name>.png`` is 44px (2x) and ``<name>@1x.png`` is 22px. The shipped
code loads the 44px file on every platform — Tauri's ``Image`` takes one bitmap
and both macOS and AppIndicator scale it themselves — and falls back to the
``@1x`` name, which is what makes a hand-made ``[icon].theme`` directory holding
only small assets work.
"""

from __future__ import annotations

import argparse
import math
import struct
import sys
import tempfile
import zlib
from pathlib import Path

# Supersampling factor. 4 means 16 samples per output pixel, which is enough to
# keep a 22px glyph's diagonals from stair-stepping.
SS = 4

# ---------------------------------------------------------------------------
# PNG
# ---------------------------------------------------------------------------


def write_png(path: Path, width: int, height: int, rgba: bytearray) -> bytes:
    """Write an 8-bit RGBA PNG and return the bytes written."""

    def chunk(tag: bytes, data: bytes) -> bytes:
        return (
            struct.pack(">I", len(data))
            + tag
            + data
            + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
        )

    # Filter type 0 (None) on every scanline: these are tiny and flat, so the
    # cleverer filters buy nothing and make the output harder to reason about.
    raw = b"".join(
        b"\x00" + bytes(rgba[y * width * 4 : (y + 1) * width * 4]) for y in range(height)
    )
    png = (
        b"\x89PNG\r\n\x1a\n"
        + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 6, 0, 0, 0))
        + chunk(b"IDAT", zlib.compress(raw, 9))
        + chunk(b"IEND", b"")
    )
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(png)
    return png


# ---------------------------------------------------------------------------
# Shape predicates, in a normalised [0,1] x [0,1] space with y running DOWN
# ---------------------------------------------------------------------------


class Shape:
    """A predicate plus the box outside which it is known to be false.

    The box is the whole optimisation: a full-canvas scan per shape is minutes
    of pure Python at 1024px, and every glyph here is mostly empty space.
    """

    __slots__ = ("pred", "bbox")

    def __init__(self, pred, bbox):
        self.pred = pred
        # Clamp and pad by half a supersample so a shape is never clipped by
        # its own bound.
        x0, y0, x1, y1 = bbox
        self.bbox = (max(0.0, x0 - 0.01), max(0.0, y0 - 0.01),
                     min(1.0, x1 + 0.01), min(1.0, y1 + 0.01))

    def __call__(self, x, y):
        return self.pred(x, y)


def union_bbox(shapes):
    return (
        min(s.bbox[0] for s in shapes),
        min(s.bbox[1] for s in shapes),
        max(s.bbox[2] for s in shapes),
        max(s.bbox[3] for s in shapes),
    )


def disc(cx, cy, r):
    rr = r * r
    return Shape(lambda x, y: (x - cx) ** 2 + (y - cy) ** 2 <= rr,
                 (cx - r, cy - r, cx + r, cy + r))


def _seg_distance(px, py, x0, y0, x1, y1):
    dx, dy = x1 - x0, y1 - y0
    length = dx * dx + dy * dy
    t = 0.0 if length == 0 else max(0.0, min(1.0, ((px - x0) * dx + (py - y0) * dy) / length))
    return math.hypot(px - (x0 + t * dx), py - (y0 + t * dy))


def capsule(x0, y0, x1, y1, w):
    """A stroked segment with round caps, which is how every stroke here is drawn."""
    half = w / 2
    return Shape(
        lambda x, y: _seg_distance(x, y, x0, y0, x1, y1) <= half,
        (min(x0, x1) - half, min(y0, y1) - half, max(x0, x1) + half, max(y0, y1) + half),
    )


def stroke(points, w):
    """A polyline: the union of its segments' capsules."""
    segs = [capsule(*points[i], *points[i + 1], w) for i in range(len(points) - 1)]
    return Shape(lambda x, y: any(s(x, y) for s in segs), union_bbox(segs))


def ring(cx, cy, r, w, a0=None, a1=None):
    """An annulus, optionally limited to an arc.

    Angles are degrees counter-clockwise from the +x axis with y read UPWARDS,
    so they match the way anybody would describe the glyph out loud, even though
    the raster's y runs down.
    """
    inner, outer = r - w / 2, r + w / 2

    def pred(x, y):
        dx, dy = x - cx, cy - y
        d = math.hypot(dx, dy)
        if not (inner <= d <= outer):
            return False
        if a0 is None:
            return True
        a = math.degrees(math.atan2(dy, dx)) % 360
        lo, hi = a0 % 360, a1 % 360
        return lo <= a <= hi if lo <= hi else (a >= lo or a <= hi)

    return Shape(pred, (cx - outer, cy - outer, cx + outer, cy + outer))


def polygon(points):
    """Even-odd fill of a closed polygon."""

    def pred(x, y):
        inside = False
        n = len(points)
        for i in range(n):
            x0, y0 = points[i]
            x1, y1 = points[(i + 1) % n]
            if (y0 > y) != (y1 > y):
                xint = x0 + (y - y0) * (x1 - x0) / (y1 - y0)
                if x < xint:
                    inside = not inside
        return inside

    return Shape(
        pred,
        (min(p[0] for p in points), min(p[1] for p in points),
         max(p[0] for p in points), max(p[1] for p in points)),
    )


def regular_polygon(cx, cy, r, sides, rotation=0.0):
    return polygon(
        [
            (
                cx + r * math.cos(math.radians(rotation + i * 360 / sides)),
                cy - r * math.sin(math.radians(rotation + i * 360 / sides)),
            )
            for i in range(sides)
        ]
    )


def arrowhead(cx, cy, r, angle_deg, span, reach):
    """A triangle at `angle_deg` on the circle, pointing along the tangent.

    Used for the two ends of the `arrows` glyph: a plain arc reads as a broken
    circle, and the heads are the only thing that says "rotating".
    """
    a = math.radians(angle_deg)
    # Radial unit vector, in raster coordinates (y down).
    nx, ny = math.cos(a), -math.sin(a)
    # Tangent, i.e. the radial vector turned a quarter turn.
    tx, ty = -ny, nx
    px, py = cx + r * nx, cy + r * ny
    return polygon(
        [
            (px + nx * span, py + ny * span),
            (px - nx * span, py - ny * span),
            (px + tx * reach, py + ty * reach),
        ]
    )


# ---------------------------------------------------------------------------
# The glyphs
# ---------------------------------------------------------------------------

# Each entry is a list of (op, predicate) applied in order: "add" unions,
# "sub" knocks out. A knockout rather than a second colour is deliberate — a
# template image has no colours to knock out WITH.
CHECK_PATH = [(0.31, 0.52), (0.44, 0.65), (0.71, 0.37)]


def symbols() -> dict:
    return {
        # "no idea": a question mark.
        "question": [
            ("add", ring(0.5, 0.34, 0.185, 0.105, a0=0, a1=200)),
            ("add", stroke([(0.685, 0.34), (0.60, 0.53), (0.50, 0.61)], 0.105)),
            ("add", disc(0.5, 0.80, 0.075)),
        ],
        # "failed": a stop sign with an X knocked out of it.
        "octagon": [
            ("add", regular_polygon(0.5, 0.5, 0.47, 8, rotation=22.5)),
            ("sub", capsule(0.35, 0.35, 0.65, 0.65, 0.115)),
            ("sub", capsule(0.65, 0.35, 0.35, 0.65, 0.115)),
        ],
        # "deployed, but": a warning triangle with a bang knocked out.
        "triangle": [
            ("add", polygon([(0.5, 0.07), (0.97, 0.89), (0.03, 0.89)])),
            ("sub", capsule(0.5, 0.36, 0.5, 0.63, 0.105)),
            ("sub", disc(0.5, 0.775, 0.063)),
        ],
        # "deployed": a solid disc with the check knocked out, so the glyph is
        # heavy enough to read as good news at 22px.
        "check": [
            ("add", disc(0.5, 0.5, 0.47)),
            ("sub", stroke(CHECK_PATH, 0.125)),
        ],
        # "green, but nothing deployed": the same check, outlined not filled.
        "check-outline": [
            ("add", ring(0.5, 0.5, 0.41, 0.095)),
            ("add", stroke([(0.33, 0.51), (0.45, 0.63), (0.68, 0.39)], 0.095)),
        ],
        # "running": two arcs chasing each other.
        "arrows": [
            ("add", ring(0.5, 0.5, 0.33, 0.105, a0=35, a1=165)),
            ("add", arrowhead(0.5, 0.5, 0.33, 35, 0.115, 0.185)),
            ("add", ring(0.5, 0.5, 0.33, 0.105, a0=215, a1=345)),
            ("add", arrowhead(0.5, 0.5, 0.33, 215, 0.115, 0.185)),
        ],
        # "cancelled": the no-entry sign.
        "slash": [
            ("add", ring(0.5, 0.5, 0.40, 0.105)),
            ("add", capsule(0.255, 0.255, 0.745, 0.745, 0.105)),
        ],
        # "parked at a gate": an hourglass. Nothing is wrong and nothing is
        # moving, which is the state every other monitor calls "running".
        "hourglass": [
            ("add", polygon([(0.24, 0.20), (0.76, 0.20), (0.5, 0.5)])),
            ("add", polygon([(0.5, 0.5), (0.24, 0.80), (0.76, 0.80)])),
            ("add", capsule(0.22, 0.155, 0.78, 0.155, 0.10)),
            ("add", capsule(0.22, 0.845, 0.78, 0.845, 0.10)),
            ("add", capsule(0.5, 0.42, 0.5, 0.58, 0.055)),
        ],
    }


# The five colours the coloured set uses, and which state wears which.
COLORS = {
    "red": (0xD8, 0x3C, 0x3E),
    "amber": (0xDE, 0x96, 0x21),
    "green": (0x2C, 0x9E, 0x59),
    "blue": (0x2B, 0x7A, 0xD6),
    "grey": (0x80, 0x86, 0x8E),
}

# state -> (symbol, colour). The eight names are IconState::as_str().
STATES = {
    "unknown": ("question", "grey"),
    "failed": ("octagon", "red"),
    "deployed_with_failure": ("triangle", "amber"),
    "deployed": ("check", "green"),
    "running": ("arrows", "blue"),
    "canceled": ("slash", "grey"),
    "parked_gate": ("hourglass", "amber"),
    "succeeded_no_deploy": ("check-outline", "green"),
}

# The colour a symbol gets when it is rendered under its own name, for
# `[icon].states` remapping. A symbol used by exactly one state inherits it.
SYMBOL_COLORS = {symbol: colour for _, (symbol, colour) in STATES.items()}


# ---------------------------------------------------------------------------
# Rasterising
# ---------------------------------------------------------------------------


def coverage(ops, size: int, ss: int = SS) -> list:
    """Supersampled coverage in 0..255, one value per output pixel."""
    n = size * ss
    mask = bytearray(n * n)
    for op, shape in ops:
        on = 1 if op == "add" else 0
        bx0, by0, bx1, by1 = shape.bbox
        j0, j1 = int(by0 * n), min(n, int(by1 * n) + 1)
        i0, i1 = int(bx0 * n), min(n, int(bx1 * n) + 1)
        pred = shape.pred
        for j in range(j0, j1):
            y = (j + 0.5) / n
            row = j * n
            for i in range(i0, i1):
                if pred((i + 0.5) / n, y):
                    mask[row + i] = on
    out = []
    scale = 255 / (ss * ss)
    for py in range(size):
        for px in range(size):
            total = 0
            for dy in range(ss):
                row = (py * ss + dy) * n + px * ss
                total += sum(mask[row : row + ss])
            out.append(int(total * scale + 0.5))
    return out


_COVERAGE_CACHE: dict = {}


def cached_coverage(key, ops, size: int, ss: int = SS) -> list:
    """The template and coloured sets differ only in RGB, so the mask is shared."""
    ck = (key, size, ss)
    if ck not in _COVERAGE_CACHE:
        _COVERAGE_CACHE[ck] = coverage(ops, size, ss)
    return _COVERAGE_CACHE[ck]


def render(key, ops, size: int, rgb) -> bytearray:
    alpha = cached_coverage(key, ops, size)
    r, g, b = rgb
    buf = bytearray(size * size * 4)
    for i, a in enumerate(alpha):
        # Straight (un-premultiplied) alpha: PNG is defined that way and both
        # macOS and GTK expect it.
        buf[i * 4 : i * 4 + 4] = bytes((r, g, b, a))
    return buf


def app_icon(size: int) -> bytearray:
    """The app icon: a bridge, in the same flat style as the tray glyphs.

    A bridge because that is what a "bridge" is in this program — a trigger job
    spanning from one pipeline to another — and this shape (two towers, an arch
    between them, a deck) because it still reads at 16px, which a truss or a
    cable-stay does not.
    """
    deck_y = 0.635
    cable = [
        (x / 24, 0.325 + 1.72 * (x / 24 - 0.5) ** 2)
        for x in range(4, 21)
    ]
    glyph = [
        # The two towers.
        ("add", capsule(0.295, 0.245, 0.295, deck_y, 0.062)),
        ("add", capsule(0.705, 0.245, 0.705, deck_y, 0.062)),
        # The arch between the towers.
        ("add", stroke(cable, 0.052)),
        # The deck.
        ("add", capsule(0.115, deck_y, 0.885, deck_y, 0.070)),
        # Spandrel posts, arch down to deck: the thing that makes this a bridge
        # rather than two posts and a curve.
        ("add", capsule(0.415, 0.352, 0.415, deck_y, 0.030)),
        ("add", capsule(0.5, 0.328, 0.5, deck_y, 0.030)),
        ("add", capsule(0.585, 0.352, 0.585, deck_y, 0.030)),
        # Piers.
        ("add", capsule(0.295, deck_y, 0.295, 0.79, 0.048)),
        ("add", capsule(0.705, deck_y, 0.705, 0.79, 0.048)),
        ("add", capsule(0.145, 0.79, 0.855, 0.79, 0.046)),
    ]
    # A rounded square, which is the shape macOS expects to be handed. One
    # predicate rather than six unioned shapes: at 1024px each full-canvas pass
    # is a million samples of pure Python.
    radius = 0.22

    def rounded_rect(x, y):
        dx = max(abs(x - 0.5) - (0.5 - radius), 0.0)
        dy = max(abs(y - 0.5) - (0.5 - radius), 0.0)
        return dx * dx + dy * dy <= radius * radius

    # 2x here, 4x for the tray: a 1024px tile antialiased to five levels has no
    # visible difference, and 4x would be four times the work for it.
    ss = 2
    back = coverage([("add", Shape(rounded_rect, (0.0, 0.0, 1.0, 1.0)))], size, ss)
    fore = coverage(glyph, size, ss)
    br, bgc, bb = 0x14, 0x2A, 0x45  # deep slate, so the white glyph carries
    buf = bytearray(size * size * 4)
    for i, a in enumerate(back):
        f = fore[i] * a // 255  # the glyph is clipped to the tile
        # Composite white over the tile, then apply the tile's own alpha.
        r = (br * (255 - f) + 0xFF * f) // 255
        g = (bgc * (255 - f) + 0xFF * f) // 255
        b = (bb * (255 - f) + 0xFF * f) // 255
        buf[i * 4 : i * 4 + 4] = bytes((r, g, b, a))
    return buf


# ---------------------------------------------------------------------------


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--out", type=Path, default=Path(__file__).resolve().parent,
        help="icons directory to write (default: this script's own directory)",
    )
    parser.add_argument(
        "--check", action="store_true",
        help="do not write; exit 1 if any committed file differs",
    )
    parser.add_argument(
        "--app-icon-size", type=int, default=1024,
        help="edge of src-tauri/icons/icon-source.png (default 1024)",
    )
    args = parser.parse_args()

    sym = symbols()
    stale: list[str] = []

    def emit(path: Path, width: int, height: int, buf: bytearray) -> None:
        if args.check:
            with tempfile.TemporaryDirectory() as tmp:
                produced = write_png(Path(tmp) / "probe.png", width, height, buf)
            if not path.exists() or path.read_bytes() != produced:
                stale.append(str(path))
            return
        write_png(path, width, height, buf)

    for mode in ("template", "color"):
        for name, ops in sym.items():
            rgb = (0, 0, 0) if mode == "template" else COLORS[SYMBOL_COLORS[name]]
            for size, suffix in ((44, ""), (22, "@1x")):
                emit(args.out / "tray" / mode / f"{name}{suffix}.png", size, size,
                     render(name, ops, size, rgb))
        for state, (symbol, colour) in STATES.items():
            rgb = (0, 0, 0) if mode == "template" else COLORS[colour]
            for size, suffix in ((44, ""), (22, "@1x")):
                emit(args.out / "tray" / mode / f"{state}{suffix}.png", size, size,
                     render(symbol, sym[symbol], size, rgb))

    n = args.app_icon_size
    emit(args.out / "icon-source.png", n, n, app_icon(n))

    if args.check:
        if stale:
            print("out of date:\n  " + "\n  ".join(stale), file=sys.stderr)
            return 1
        print("icons are up to date")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
