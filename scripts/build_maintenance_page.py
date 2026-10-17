"""Draw the voxel construction island into app/geo_maintenance/maintenance.html.

Same projection and face shading as the homepage island (home_voxel.js isoSVG).
Rerun after editing the scene: python scripts/build_maintenance_page.py
"""
from __future__ import annotations

import math
import random
import re
from dataclasses import dataclass, field
from pathlib import Path

PAGE = Path(__file__).resolve().parent.parent / "app" / "geo_maintenance" / "maintenance.html"
S = 14
SHADE = (0x16, 0x12, 0x1E)
LEFT, RIGHT = 0.8, 0.63
EPS = 1e-6

GRASS = ["#7db86a", "#88c273", "#74ad62"]
GRASS_EDGE = "#6aa159"
DIRT = "#9b7f60"
STRATA = ["#b39d86", "#c7b194", "#dcc8a2"]
PATH = "#e4be8b"
STONE, STONE_TOP = "#cabdab", "#d9cebc"
CRANE, CRANE_DARK = "#ffcf5c", "#e9b13e"
STEEL = "#9aa4b8"
WOOD, WOOD_TOP = "#b98552", "#c8955a"
CONE, WHITE = "#f4a261", "#eef1f8"
CONCRETE = "#8a8f9c"
CABLE = "#2b3350"
GLASS = "#6f7a9c"
BEACON = "#ffb454"


def rgb(h: str) -> tuple[int, int, int]:
    n = int(h[1:], 16)
    return n >> 16 & 255, n >> 8 & 255, n & 255


def shade(h: str, t: float) -> str:
    c = rgb(h)
    return "#" + "".join(f"{round(c[i] * t + SHADE[i] * (1 - t)):02x}" for i in range(3))


def pr(x: float, y: float, z: float) -> tuple[float, float]:
    return (x - y) * S, (x + y) * S / 2 - z * S * 1.2


@dataclass
class Box:
    x: float
    y: float
    z: float
    w: float
    d: float
    h: float
    c: str
    top: str | None = None
    faces: str = "tlr"
    cls: str | None = None
    kids: list[Box] = field(default_factory=list)

    @property
    def x2(self) -> float:
        return self.x + self.w

    @property
    def y2(self) -> float:
        return self.y + self.d

    @property
    def z2(self) -> float:
        return self.z + self.h

    def screen_bounds(self) -> tuple[float, float, float, float]:
        pts = [pr(x, y, z) for x in (self.x, self.x2) for y in (self.y, self.y2) for z in (self.z, self.z2)]
        xs, ys = [p[0] for p in pts], [p[1] for p in pts]
        return min(xs), min(ys), max(xs), max(ys)


def group(boxes: list[Box], cls: str) -> Box:
    """Boxes drawn as one unit (and one <g>) so a CSS animation can move them together."""
    x, y, z = min(b.x for b in boxes), min(b.y for b in boxes), min(b.z for b in boxes)
    return Box(x, y, z, max(b.x2 for b in boxes) - x, max(b.y2 for b in boxes) - y,
               max(b.z2 for b in boxes) - z, "", cls=cls, kids=boxes)


def behind(a: Box, b: Box) -> bool | None:
    """True if a must be painted before b; None when no axis separates them."""
    for lo_a, hi_a, lo_b, hi_b in ((a.x, a.x2, b.x, b.x2), (a.y, a.y2, b.y, b.y2), (a.z, a.z2, b.z, b.z2)):
        if hi_a <= lo_b + EPS:
            return True
        if hi_b <= lo_a + EPS:
            return False
    return None


def paint_order(boxes: list[Box]) -> list[Box]:
    n = len(boxes)
    bounds = [b.screen_bounds() for b in boxes]
    after: list[set[int]] = [set() for _ in range(n)]
    need = [0] * n
    for i in range(n):
        for j in range(i + 1, n):
            bi, bj = bounds[i], bounds[j]
            if bi[2] <= bj[0] or bj[2] <= bi[0] or bi[3] <= bj[1] or bj[3] <= bi[1]:
                continue
            first = behind(boxes[i], boxes[j])
            if first is None:
                continue
            a, b = (i, j) if first else (j, i)
            if b not in after[a]:
                after[a].add(b)
                need[b] += 1
    depth = [b.x + b.y + b.z + (b.w + b.d + b.h) / 2 for b in boxes]
    ready = sorted((i for i in range(n) if need[i] == 0), key=lambda i: depth[i])
    out, done = [], [False] * n
    while len(out) < n:
        if not ready:
            ready = [min((i for i in range(n) if not done[i]), key=lambda i: depth[i])]
        i = ready.pop(0)
        if done[i]:
            continue
        done[i] = True
        out.append(boxes[i])
        for j in after[i]:
            need[j] -= 1
            if need[j] == 0 and not done[j]:
                ready.append(j)
        ready.sort(key=lambda k: depth[k])
    return out


# ── Scene ────────────────────────────────────────────────────────────────

def island(rand: random.Random) -> list[Box]:
    n, cx, cy, r = 14, 7.0, 7.0, 6.6
    cols: dict[tuple[int, int], int] = {}
    for x in range(n):
        for y in range(n):
            d = math.hypot(x + 0.5 - cx, y + 0.5 - cy) + rand.uniform(-0.35, 0.35)
            if d <= r:
                cols[(x, y)] = -(1 + int((r - d) * 1.15))
    site = {(x, y) for x in range(5, 10) for y in range(4, 10)} | {(x, 9) for x in range(9, 13)}
    occupied = {(x, y, z) for (x, y), bottom in cols.items() for z in range(bottom, 1)}
    out = []
    for (x, y), bottom in cols.items():
        for z in range(bottom, 1):
            faces = ("t" if (x, y, z + 1) not in occupied else "") \
                + ("l" if (x, y + 1, z) not in occupied else "") \
                + ("r" if (x + 1, y, z) not in occupied else "")
            if not faces:
                continue
            side = GRASS_EDGE if z == 0 else DIRT if z >= -2 else STRATA[(z + 3) % 3]
            top = (PATH if (x, y) in site else rand.choice(GRASS)) if z == 0 else None
            out.append(Box(x, y, z, 1, 1, 1, side, top, faces))
    return out


def tower() -> list[Box]:
    layout = {
        1: [(6, 5), (7, 5), (8, 5), (6, 6), (8, 6), (6, 7), (7, 7), (8, 7)],
        2: [(6, 5), (7, 5), (8, 5), (6, 6), (6, 7), (8, 6)],
        3: [(6, 5), (7, 5), (6, 6)],
    }
    return [Box(x, y, z, 1, 1, 1, STONE, STONE_TOP) for z, cells in layout.items() for x, y in cells]


def scaffold() -> list[Box]:
    t, x0, x1, y0, y1, top = 0.12, 5.75, 9.25, 4.75, 8.25, 4.7
    out = []
    for px in (x0, 7.5, x1):
        out.append(Box(px - t / 2, y1 - t / 2, 1, t, t, top - 1, STEEL))
    for py in (y0, 6.5):
        out.append(Box(x1 - t / 2, py - t / 2, 1, t, t, top - 1, STEEL))
    for z in (2.35, 3.75):
        out.append(Box(x0, y1 - 0.02, z, x1 - x0, 0.42, 0.1, WOOD, WOOD_TOP))
        out.append(Box(x1 - 0.02, y0, z, 0.42, y1 - y0 + 0.4, 0.1, WOOD, WOOD_TOP))
    out.append(Box(x0, y1 - t / 2, top - 0.05, x1 - x0, t, t, STEEL))
    out.append(Box(x1 - t / 2, y0, top - 0.05, t, y1 - y0, t, STEEL))
    return out


def crane() -> list[Box]:
    t = 0.13
    x0, y0, s, top = 2.4, 6.1, 0.8, 9.0
    out = [Box(x0 - 0.2, y0 - 0.2, 1, s + 0.4, s + 0.4, 0.35, CONCRETE)]
    for px in (x0, x0 + s - t):
        for py in (y0, y0 + s - t):
            out.append(Box(px, py, 1.35, t, t, top - 1.35, CRANE))
    z = 2.2
    while z < top - 0.3:
        out.append(Box(x0, y0, z, s, t, t, CRANE_DARK))
        out.append(Box(x0, y0 + s - t, z, s, t, t, CRANE))
        out.append(Box(x0, y0, z, t, s, t, CRANE_DARK))
        out.append(Box(x0 + s - t, y0, z, t, s, t, CRANE))
        z += 0.95
    jy, jd = y0 + 0.1, s - 0.2
    out += [
        Box(x0 - 0.05, y0 - 0.05, top, s + 0.1, s + 0.1, 0.25, CRANE_DARK, CRANE),
        Box(x0 + s, y0 + 0.05, top - 0.75, 0.6, s - 0.1, 0.75, CRANE, CRANE_DARK),
        Box(x0 + 0.2, y0 + 0.2, top + 0.25, s - 0.4, s - 0.4, 1.6, CRANE),
        Box(x0 - 2.6, jy, top + 0.25, 2.6, jd, 0.32, CRANE),
        Box(x0 - 2.5, jy - 0.05, top - 0.55, 0.9, jd + 0.1, 0.8, CONCRETE),
        Box(x0 + s, jy, top + 0.25, 8.2, jd, 0.32, CRANE),
    ]
    out.append(Box(x0 + s + 0.6, y0 + 0.05, top - 0.55, 0.02, 0.5, 0.4, GLASS, faces="r"))
    out.append(Box(x0 + 0.25, y0 + 0.25, top + 1.85, s - 0.5, s - 0.5, 0.26, BEACON, cls="mt-beacon"))
    return out


def hook() -> Box:
    hx, hy, jib = 7.1, 6.35, 9.25
    return group([
        Box(hx - 0.25, hy - 0.2, jib - 0.12, 0.6, 0.5, 0.12, CABLE),
        Box(hx, hy, 6.45, 0.06, 0.06, jib - 0.12 - 6.45, CABLE),
        Box(hx - 0.15, hy - 0.15, 6.2, 0.36, 0.36, 0.25, CONCRETE),
        Box(hx - 0.4, hy - 0.4, 5.25, 0.86, 0.86, 0.86, STONE, STONE_TOP),
    ], "mt-hook")


def cone(x: float, y: float) -> list[Box]:
    return [
        Box(x - 0.32, y - 0.32, 1, 0.64, 0.64, 0.08, CONE),
        Box(x - 0.2, y - 0.2, 1.08, 0.4, 0.4, 0.22, CONE),
        Box(x - 0.17, y - 0.17, 1.3, 0.34, 0.34, 0.13, WHITE),
        Box(x - 0.12, y - 0.12, 1.43, 0.24, 0.24, 0.2, CONE),
    ]


def barrier(x0: float, x1: float, y: float) -> list[Box]:
    out = [Box(px, y, 1, 0.16, 0.16, 1.0, WHITE) for px in (x0, x1 - 0.16)]
    seg, x, i = 0.42, x0 - 0.15, 0
    while x < x1 + 0.1:
        w = min(seg, x1 + 0.15 - x)
        out.append(Box(x, y + 0.02, 1.55, w, 0.12, 0.28, "#ffd166" if i % 2 == 0 else "#2b3350"))
        x, i = x + seg, i + 1
    return out


def pallet(x: float, y: float) -> list[Box]:
    out = [Box(x, y, 1, 1.5, 1.5, 0.16, WOOD, WOOD_TOP)]
    s = 0.68
    for dx, dy, dz in ((0, 0, 0), (1, 0, 0), (0, 1, 0), (1, 1, 0), (0, 0, 1), (1, 0, 1), (0, 1, 1)):
        out.append(Box(x + 0.05 + dx * (s + 0.04), y + 0.05 + dy * (s + 0.04), 1.16 + dz * s, s, s, s, STONE, STONE_TOP))
    return out


def planks(x: float, y: float) -> list[Box]:
    return [Box(x, y + i * 0.3, 1 + k * 0.12, 2.2, 0.26, 0.12, WOOD, WOOD_TOP)
            for k, i in ((0, 0), (0, 1), (0, 2), (1, 0.5), (1, 1.5))]


def toolbox(x: float, y: float) -> list[Box]:
    return [
        Box(x, y, 1, 0.7, 0.36, 0.32, "#e0525a", "#f07178"),
        Box(x + 0.1, y + 0.14, 1.32, 0.06, 0.08, 0.14, CABLE),
        Box(x + 0.54, y + 0.14, 1.32, 0.06, 0.08, 0.14, CABLE),
        Box(x + 0.1, y + 0.14, 1.46, 0.5, 0.08, 0.06, CABLE),
    ]


def oak(x: float, y: float, s: float = 1.0) -> list[Box]:
    return [
        Box(x - 0.1 * s, y - 0.1 * s, 1, 0.2 * s, 0.2 * s, 0.6 * s, "#7a5a42"),
        Box(x - 0.42 * s, y - 0.42 * s, 1 + 0.6 * s, 0.84 * s, 0.84 * s, 0.6 * s, "#4f8f45", "#66a857"),
        Box(x - 0.28 * s, y - 0.28 * s, 1 + 1.2 * s, 0.56 * s, 0.56 * s, 0.3 * s, "#5a9a4d", "#72b561"),
    ]


def scene() -> list[Box]:
    rand = random.Random(7)
    return (island(rand) + tower() + scaffold() + crane() + [hook()]
            + cone(10.4, 11.2) + cone(4.6, 11.0) + cone(11.5, 7.6)
            + barrier(9.2, 12.0, 10.4) + pallet(3.4, 9.0) + planks(9.9, 3.3)
            + toolbox(5.4, 10.2) + oak(4.8, 1.9, 1.2) + oak(2.4, 9.8) + oak(11.0, 5.6, 0.9))


# ── SVG ──────────────────────────────────────────────────────────────────

def faces(b: Box) -> list[tuple[list[tuple[float, float]], str]]:
    x, y, z, x2, y2, z2 = b.x, b.y, b.z, b.x2, b.y2, b.z2
    out = []
    if "l" in b.faces:
        out.append(([pr(x, y2, z), pr(x2, y2, z), pr(x2, y2, z2), pr(x, y2, z2)], shade(b.c, LEFT)))
    if "r" in b.faces:
        out.append(([pr(x2, y, z), pr(x2, y2, z), pr(x2, y2, z2), pr(x2, y, z2)], shade(b.c, RIGHT)))
    if "t" in b.faces:
        out.append(([pr(x, y, z2), pr(x2, y, z2), pr(x2, y2, z2), pr(x, y2, z2)], b.top or b.c))
    return out


def path(pts: list[tuple[float, float]], fill: str) -> str:
    d = "L".join(f"{p[0]:.1f} {p[1]:.1f}" for p in pts)
    return f'<path d="M{d}Z" fill="{fill}" stroke="{fill}"/>'


def render() -> str:
    body, xs, ys = [], [], []

    def emit(b: Box, out: list[str]) -> None:
        for pts, fill in faces(b):
            xs.extend(p[0] for p in pts)
            ys.extend(p[1] for p in pts)
            out.append(path(pts, fill))

    for b in paint_order(scene()):
        if b.kids:
            inner: list[str] = []
            for k in paint_order(b.kids):
                emit(k, inner)
            ox, oy = pr(b.x + b.w / 2, b.y + b.d / 2, b.z2)
            body.append(f'<g class="{b.cls}" style="transform-origin:{ox:.1f}px {oy:.1f}px">{"".join(inner)}</g>')
        elif b.cls:
            inner = []
            emit(b, inner)
            body.append(f'<g class="{b.cls}">{"".join(inner)}</g>')
        else:
            emit(b, body)

    x0, y0, x1, y1 = min(xs) - 24, min(ys) - 24, max(xs) + 24, max(ys) + 16
    rand = random.Random(3)
    stars = []
    for _ in range(26):
        sx, sy = rand.uniform(x0, x1), rand.uniform(y0, y0 + (y1 - y0) * 0.55)
        size = 2.5 if rand.random() < 0.2 else 1.5
        stars.append(f'<rect x="{sx:.0f}" y="{sy:.0f}" width="{size}" height="{size}" fill="#e8edf7" '
                     f'opacity="{rand.uniform(0.25, 0.7):.2f}"/>')
    w, h = x1 - x0, y1 - y0
    return (f'<svg class="mt-art" viewBox="{x0:.0f} {y0:.0f} {w:.0f} {h:.0f}" width="{w:.0f}" height="{h:.0f}" '
            f'aria-hidden="true" focusable="false">{"".join(stars)}'
            f'<g stroke-width=".6" stroke-linejoin="round">{"".join(body)}</g></svg>')


def main() -> None:
    html = PAGE.read_text(encoding="utf-8")
    html, n = re.subn(r"<!-- island -->.*?<!-- /island -->",
                      lambda _: f"<!-- island -->\n{render()}\n<!-- /island -->", html, flags=re.S)
    if n != 1:
        raise SystemExit("island markers not found in " + str(PAGE))
    PAGE.write_text(html, encoding="utf-8", newline="\n")
    print(f"wrote {PAGE} ({len(html) // 1024} KB)")


if __name__ == "__main__":
    main()
