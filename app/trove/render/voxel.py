"""Headless, GPU-free voxel rasterizer that emulates Trove's ``-tool catalog``
blueprint→PNG render. Pure numpy + Pillow; runs anywhere (no game, no GPU).

The constants are reverse-engineered ground truth from ``Trove_x64.exe``:
  * perspective camera (fixed orientation from angle consts 110deg / -15deg, vert
    FOV ~43deg), world +X -> screen-left / +Z -> screen-right;
  * flat per-visible-face shading = ambient(0.502) + white directional light along
    (1,1,1): only +X/+Y/+Z faces are ever visible -> factors {+Y 1.109, +X 0.929,
    +Z 0.500}, clamp 255;
  * glass (type 18/54) + glowing glass (56): output alpha = (level/255)^2 where
    level = 16+32*w; glowing solid (55) + glowing glass are emissive (no shading);
  * the model is fit to the 256 frame by an empirically-recovered scale law (a
    25-box sweep inverted against the exact camera; cubes exact, 2.88% rms).

Public API: ``render_blueprint_png(data: bytes, dim=256) -> bytes`` (PNG).
"""
from __future__ import annotations

import io
import struct
import zlib

import numpy as np
from PIL import Image

try:                                   # numba is optional: absence falls back to the
    from numba import njit  # pure-numpy per-triangle raster (same pixels).
    _HAVE_NUMBA = True
except Exception:  # pragma: no cover
    _HAVE_NUMBA = False

MAGIC = b"kiwib"


class BlueprintError(ValueError):
    """Raised when bytes are not a decodable kiwib blueprint."""


class BlueprintEmpty(BlueprintError):
    """Blueprint is a placeholder / has no voxels to render."""


class BlueprintTooLarge(BlueprintError):
    """Blueprint has more voxels than the preview cap allows."""


# --------------------------------------------------------------------------- #
# kiwib decode (v3 / v4 / v5)  -- ground truth from TROVE_BLUEPRINT_FORMAT.md
# --------------------------------------------------------------------------- #
def _uleb(data: bytes, pos: int) -> tuple[int, int]:
    r = s = 0
    while True:
        if pos >= len(data):
            raise BlueprintError("truncated LEB128")
        b = data[pos]; pos += 1
        r |= (b & 0x7F) << s
        if not (b & 0x80):
            return r, pos
        s += 7


def _svar(data: bytes, pos: int) -> tuple[int, int]:
    v, pos = _uleb(data, pos)
    return (v >> 1) ^ -(v & 1), pos


def decode(data: bytes) -> list[dict]:
    """Decode a blueprint into voxel dicts ``{x,y,z,r,g,b,w,type}`` in a 0-based,
    X-mirrored (Qubicle) grid -- exactly what the game's QB export shows."""
    if data[:5] != MAGIC:
        raise BlueprintError("not a kiwib blueprint")
    version = struct.unpack_from("<I", data, 5)[0]
    if version in (3, 4):
        count, pos = _uleb(data, 9)
        raw = []
        for _ in range(count):
            x, pos = _svar(data, pos)
            y, pos = _svar(data, pos)
            z, pos = _svar(data, pos)
            if pos + 6 > len(data):
                raise BlueprintError("truncated v3/v4 voxel")
            vtype = struct.unpack_from("<H", data, pos)[0]
            b, g, r, w = data[pos + 2], data[pos + 3], data[pos + 4], data[pos + 5]
            pos += 6
            raw.append((x, y, z, r, g, b, w, vtype))
        if not raw:
            raise BlueprintError("empty blueprint")
        mnx = min(v[0] for v in raw); mny = min(v[1] for v in raw); mnz = min(v[2] for v in raw)
        mxx = max(v[0] for v in raw); sx = mxx - mnx + 1
        return [{"x": (sx - 1) - (x - mnx), "y": y - mny, "z": z - mnz,
                 "r": r, "g": g, "b": b, "w": w, "type": t}
                for (x, y, z, r, g, b, w, t) in raw]
    if version == 5:
        try:
            d = zlib.decompressobj()
            body = d.decompress(data[9:]) + d.flush()   # flush: don't truncate large models
        except zlib.error as e:
            raise BlueprintError(f"bad v5 zlib: {e}") from e
        if len(body) < 32:
            raise BlueprintError("v5 payload too small")
        px, py, pz, sx, sy, sz, count, start = struct.unpack_from("<8i", body, 0)
        if count <= 0 or sx <= 0 or sy <= 0 or sz <= 0:
            raise BlueprintError("v5 empty/invalid bounds")
        off = 32
        deltas = struct.unpack_from(f"<{count - 1}i", body, off) if count > 1 else ()
        off += 4 * (count - 1)
        types = struct.unpack_from(f"<{count}H", body, off); off += 2 * count
        colors = struct.unpack_from(f"<{count}I", body, off); off += 4 * count
        idx = [start]
        for d in deltas:
            idx.append(idx[-1] + d)
        plane = sx * sz
        out = []
        for L, t, c in zip(idx, types, colors, strict=False):
            y = L // plane; rem = L % plane; z = rem // sx; x = rem % sx
            out.append({"x": sx - 1 - x, "y": y, "z": z,
                        "r": (c >> 16) & 0xFF, "g": (c >> 8) & 0xFF, "b": c & 0xFF,
                        "w": (c >> 24) & 0xFF, "type": t})
        return out
    raise BlueprintError(f"unsupported kiwib version {version}")


def is_empty_blueprint(data: bytes) -> bool:
    """True if a blueprint has zero voxels (a placeholder for an unused part) or is
    unreadable - i.e. nothing a viewer could show. Cheap: reads only the count, not
    the whole model. Mirrors BetterTroveTools' ``is_empty_blueprint``."""
    if len(data) < 9 or data[:5] != MAGIC:
        return True
    version = struct.unpack_from("<I", data, 5)[0]
    if version in (3, 4):
        try:
            count, _ = _uleb(data, 9)
        except BlueprintError:
            return True
        return count == 0
    if version == 5:
        try:
            d = zlib.decompressobj()
            body = d.decompress(data[9:]) + d.flush()
        except zlib.error:
            return True
        if len(body) < 28:
            return True
        return struct.unpack_from("<i", body, 24)[0] <= 0
    return True


# --------------------------------------------------------------------------- #
# Material mapping (type/w -> render kind + colour)
# --------------------------------------------------------------------------- #
_TINTS = {1: (150, 130, 100), 2: (40, 170, 0), 3: (120, 120, 120), 4: (110, 78, 165),
          79: (185, 194, 197), 100: (220, 222, 225), 174: (232, 33, 70)}
_AUTHORED = {21, 18, 54, 55, 56, 24}


def _kind(t: int) -> str:
    if t == 55:
        return "E"          # glowing solid -> emissive opaque
    if t == 56:
        return "GE"         # glowing glass -> emissive transparent
    if t in (18, 54):
        return "G"          # glass / tiled glass -> shaded transparent
    return "S"              # solid (and game-internal types) -> shaded opaque


# The 4th colour byte does double duty, which is why the specular map used to go
# missing from the previews. On a TRANSPARENT voxel it is the alpha level
# (16+32*w); on a shaded SOLID it is the specular-map index -- the same 0-7 value
# Trove's ``*_s.qb`` material map carries, and the tile the game's shader picks out
# of ``textures/brdfmap.dds`` (``Lighting_BRDFSpecular``, ``effectColor.x*8.0``).
# Verified against the game's own blueprints: knight armour and silver badges are
# all 1, gold/platinum badges all 3, and tile 3 of the atlas is the rainbow lobe.
# Glowing solids carry a value too, but the game renders them unlit and its own
# specular pass on them is known-broken, so they stay 0 here.
SPEC_NAMES = {0: "rough", 1: "metal", 2: "water", 3: "iridescent", 4: "waxy"}

# Render-kind -> compact wire code the 3D viewers consume (solid / glass / glow /
# glow-glass). Shared so the catalog render, the single-blueprint viewer, and the
# assembled-creature viewer all speak the same material language.
KIND_CODE = {"S": 0, "G": 1, "E": 2, "GE": 3}


def material_for(r: int, g: int, b: int, w: int, t: int) -> tuple[int, int, int, str, int, int]:
    """A voxel's material: ``(r, g, b, kind, level, spec)``. ``kind`` is S/G/E/GE;
    ``level`` is the glass alpha level ``16+32*w`` for transparent kinds, else 255;
    ``spec`` is the specular-map index for shaded solids, else 0. Dark procedurally-
    tinted voxels are mapped to their placeholder colour. The single source of truth
    for every voxel renderer (catalog PNG + both 3D viewers)."""
    if t not in _AUTHORED and max(r, g, b) <= 24:
        r, g, b = _TINTS.get(t, (110, 110, 110))      # procedural placeholder tint
    kind = _kind(t)
    glassy = kind in ("G", "GE")
    level = 16 + 32 * max(0, min(int(w), 7)) if glassy else 255
    return r, g, b, kind, level, (0 if kind != "S" else max(0, min(int(w), 7)))


def to_render_voxels(decoded: list[dict]) -> dict:
    """``{(x,y,z): (r,g,b,kind,level,spec)}`` -- level = glass alpha level 16+32*w."""
    out = {}
    for v in decoded:
        out[(v["x"], v["y"], v["z"])] = material_for(v["r"], v["g"], v["b"], v["w"], v["type"])
    return out


# Voxel-kind -> compact int code the web 3D viewer (blueprint_viewer.js) consumes.
KIND_CODE = {"S": 0, "G": 1, "E": 2, "GE": 3}   # solid / glass / glow / glow-glass
RENDER_VOXEL_CAP = 250_000   # guard the viewer against pathologically huge models


def pack_blueprint(raw: bytes, path: str, *, cap: int = RENDER_VOXEL_CAP) -> dict:
    """Decode raw ``.blueprint`` bytes into the compact parallel-array voxel payload
    the web 3D viewer (blueprint_viewer.js) consumes directly.

    Raises ``BlueprintEmpty`` for placeholders / zero-voxel parts, ``BlueprintTooLarge``
    past ``cap``, and ``BlueprintError`` when the bytes aren't a decodable blueprint.
    CPU-bound - call via ``asyncio.to_thread``."""
    if is_empty_blueprint(raw):
        raise BlueprintEmpty("This blueprint is an empty placeholder (no voxels to show).")
    voxels = to_render_voxels(decode(raw))
    if not voxels:
        raise BlueprintEmpty("That blueprint has no voxels.")
    if len(voxels) > cap:
        raise BlueprintTooLarge(f"This model is too large to preview ({len(voxels):,} voxels).")
    xs: list[int] = []; ys: list[int] = []; zs: list[int] = []
    rgb: list[int] = []; kind: list[int] = []; level: list[int] = []; spec: list[int] = []
    for (x, y, z), (r, g, b, k, lv, sp) in voxels.items():
        xs.append(x); ys.append(y); zs.append(z)
        rgb.append((r << 16) | (g << 8) | b)
        kind.append(KIND_CODE.get(k, 0)); level.append(lv); spec.append(sp)
    out = {
        "path": path,
        "count": len(xs),
        "size": [max(xs) - min(xs) + 1, max(ys) - min(ys) + 1, max(zs) - min(zs) + 1],
        "x": xs, "y": ys, "z": zs, "rgb": rgb, "kind": kind, "level": level,
    }
    if any(spec):                       # all-rough is the common case: don't ship the array
        out["spec"] = spec
    return out


# --------------------------------------------------------------------------- #
# Rasterizer
# --------------------------------------------------------------------------- #
PARAMS = {"az": 40.0, "el": 29.7, "fov_y": 42.9, "cx": 128.0, "cy": 128.0, "dist": 2.55, "ss": 4,
              "f_top": 1.109, "f_x": 0.929, "f_z": 0.500}

_FACES = {
    "x": ((1, 0, 0), [(1, 0, 0), (1, 1, 0), (1, 1, 1), (1, 0, 1)]),
    "y": ((0, 1, 0), [(0, 1, 0), (0, 1, 1), (1, 1, 1), (1, 1, 0)]),
    "z": ((0, 0, 1), [(0, 0, 1), (1, 0, 1), (1, 1, 1), (0, 1, 1)]),
}


def _dir_from_angles(az, el):
    a, e = np.radians(az), np.radians(el)
    return np.array([np.cos(e) * np.sin(a), np.sin(e), np.cos(e) * np.cos(a)])


def _look_at(eye, target, up):
    f = target - eye; f /= np.linalg.norm(f)
    s = np.cross(f, up); s /= np.linalg.norm(s)
    u = np.cross(s, f)
    return np.stack([s, u, -f])


def _project(verts, eye, R, fy, H, cx, cy):
    cam = (verts - eye) @ R.T
    z = -cam[:, 2]; z = np.where(z <= 1e-6, 1e-6, z)
    f = (H * 0.5) / np.tan(np.radians(fy) * 0.5)
    u = cx - f * cam[:, 0] / z            # +X -> screen-left (tool handedness)
    v = cy - f * cam[:, 1] / z
    return np.stack([u, v], axis=1), z


def _is_opaque(v):
    return v[3] in ("S", "E")


def _faces(voxels):
    out = []
    for (x, y, z), v in voxels.items():
        my_opaque = _is_opaque(v)
        for key, (n, offs) in _FACES.items():
            nb = voxels.get((x + n[0], y + n[1], z + n[2]))
            if nb is not None:
                if my_opaque:
                    if _is_opaque(nb):
                        continue          # opaque behind opaque -> cull
                    # neighbor transparent: draw (the solid face shows THROUGH the glass)
                else:
                    continue              # transparent face against any neighbor -> cull
            corners = np.array([(x + dx, y + dy, z + dz) for (dx, dy, dz) in offs], float)
            out.append((key, v, corners))
    return out


def _tri_mask(zbuf, tri, z3):
    H, W = zbuf.shape
    (x0, y0), (x1, y1), (x2, y2) = tri
    minx = max(int(np.floor(min(x0, x1, x2))), 0); maxx = min(int(np.ceil(max(x0, x1, x2))), W - 1)
    miny = max(int(np.floor(min(y0, y1, y2))), 0); maxy = min(int(np.ceil(max(y0, y1, y2))), H - 1)
    if minx > maxx or miny > maxy:
        return None
    gx, gy = np.meshgrid(np.arange(minx, maxx + 1) + 0.5, np.arange(miny, maxy + 1) + 0.5)
    d = (y1 - y2) * (x0 - x2) + (x1 - x2) * (y2 - y0)
    if abs(d) < 1e-9:
        return None
    a = ((y1 - y2) * (gx - x2) + (x2 - x1) * (gy - y2)) / d
    b = ((y2 - y0) * (gx - x2) + (x0 - x2) * (gy - y2)) / d
    c = 1 - a - b
    z = a * z3[0] + b * z3[1] + c * z3[2]
    return slice(miny, maxy + 1), slice(minx, maxx + 1), (a >= 0) & (b >= 0) & (c >= 0), z


def _raster_opaque(buf, zbuf, tri, z3, rgb):
    r = _tri_mask(zbuf, tri, z3)
    if r is None:
        return
    sy, sx, inside, z = r
    sub_z = zbuf[sy, sx]; sub_b = buf[sy, sx]
    win = inside & (z < sub_z)
    sub_z[win] = z[win]
    sub_b[win, 0:3] = rgb
    sub_b[win, 3] = 255.0


def _composite_glass(buf, zbuf, tri, z3, rgb, alpha):
    r = _tri_mask(zbuf, tri, z3)
    if r is None:
        return
    sy, sx, inside, z = r
    win = inside & (z < zbuf[sy, sx])
    if not win.any():
        return
    sub = buf[sy, sx]
    ga = alpha / 255.0
    dst = sub[win]
    dst[:, 0:3] = ga * np.asarray(rgb, float) + (1 - ga) * dst[:, 0:3]
    dst[:, 3] = alpha + (1 - ga) * dst[:, 3]
    sub[win] = dst


# --------------------------------------------------------------------------- #
# Numba fast path: the per-triangle Python loop above is ~95% of render time
# (16k+ triangles, each paying numpy dispatch + meshgrid overhead). These kernels
# compile the SAME barycentric raster + z-test + glass composite into one machine-
# code pass over all triangles, ~20-40x faster. The math is byte-for-byte the numpy
# version's, so output is pixel-identical (asserted by the parity test).
# --------------------------------------------------------------------------- #
if _HAVE_NUMBA:
    @njit(cache=True)
    def _raster_opaque_nb(buf, zbuf, xy, z, rgb):  # noqa: ANN001
        H, W = zbuf.shape
        for i in range(xy.shape[0]):
            x0 = xy[i, 0, 0]; y0 = xy[i, 0, 1]
            x1 = xy[i, 1, 0]; y1 = xy[i, 1, 1]
            x2 = xy[i, 2, 0]; y2 = xy[i, 2, 1]
            z0 = z[i, 0]; z1 = z[i, 1]; z2 = z[i, 2]
            r = rgb[i, 0]; g = rgb[i, 1]; b = rgb[i, 2]
            minx = int(np.floor(min(x0, min(x1, x2))));  maxx = int(np.ceil(max(x0, max(x1, x2))))
            miny = int(np.floor(min(y0, min(y1, y2))));  maxy = int(np.ceil(max(y0, max(y1, y2))))
            if minx < 0: minx = 0
            if miny < 0: miny = 0
            if maxx > W - 1: maxx = W - 1
            if maxy > H - 1: maxy = H - 1
            if minx > maxx or miny > maxy:
                continue
            d = (y1 - y2) * (x0 - x2) + (x1 - x2) * (y2 - y0)
            if abs(d) < 1e-9:
                continue
            for py in range(miny, maxy + 1):
                gy = py + 0.5
                for px in range(minx, maxx + 1):
                    gx = px + 0.5
                    a = ((y1 - y2) * (gx - x2) + (x2 - x1) * (gy - y2)) / d
                    bb = ((y2 - y0) * (gx - x2) + (x0 - x2) * (gy - y2)) / d
                    c = 1.0 - a - bb
                    if a >= 0.0 and bb >= 0.0 and c >= 0.0:
                        zz = a * z0 + bb * z1 + c * z2
                        if zz < zbuf[py, px]:
                            zbuf[py, px] = zz
                            buf[py, px, 0] = r; buf[py, px, 1] = g
                            buf[py, px, 2] = b; buf[py, px, 3] = 255.0

    @njit(cache=True)
    def _composite_glass_nb(buf, zbuf, xy, z, rgb, alpha):  # noqa: ANN001
        H, W = zbuf.shape
        for i in range(xy.shape[0]):
            x0 = xy[i, 0, 0]; y0 = xy[i, 0, 1]
            x1 = xy[i, 1, 0]; y1 = xy[i, 1, 1]
            x2 = xy[i, 2, 0]; y2 = xy[i, 2, 1]
            z0 = z[i, 0]; z1 = z[i, 1]; z2 = z[i, 2]
            r = rgb[i, 0]; g = rgb[i, 1]; b = rgb[i, 2]
            al = alpha[i]; ga = al / 255.0
            minx = int(np.floor(min(x0, min(x1, x2))));  maxx = int(np.ceil(max(x0, max(x1, x2))))
            miny = int(np.floor(min(y0, min(y1, y2))));  maxy = int(np.ceil(max(y0, max(y1, y2))))
            if minx < 0: minx = 0
            if miny < 0: miny = 0
            if maxx > W - 1: maxx = W - 1
            if maxy > H - 1: maxy = H - 1
            if minx > maxx or miny > maxy:
                continue
            d = (y1 - y2) * (x0 - x2) + (x1 - x2) * (y2 - y0)
            if abs(d) < 1e-9:
                continue
            for py in range(miny, maxy + 1):
                gy = py + 0.5
                for px in range(minx, maxx + 1):
                    gx = px + 0.5
                    a = ((y1 - y2) * (gx - x2) + (x2 - x1) * (gy - y2)) / d
                    bb = ((y2 - y0) * (gx - x2) + (x0 - x2) * (gy - y2)) / d
                    c = 1.0 - a - bb
                    if a >= 0.0 and bb >= 0.0 and c >= 0.0:
                        zz = a * z0 + bb * z1 + c * z2
                        if zz < zbuf[py, px]:
                            buf[py, px, 0] = ga * r + (1.0 - ga) * buf[py, px, 0]
                            buf[py, px, 1] = ga * g + (1.0 - ga) * buf[py, px, 1]
                            buf[py, px, 2] = ga * b + (1.0 - ga) * buf[py, px, 2]
                            buf[py, px, 3] = al + (1.0 - ga) * buf[py, px, 3]


def _raster_batched(buf, zbuf, opaque, glassf):
    """Pack the triangle lists into contiguous arrays and rasterize each set in one
    compiled pass. ``glassf`` must already be sorted back-to-front (composite order)."""
    if opaque:
        oxy = np.array([[t[0], t[1], t[2]] for (t, _z, _c) in opaque], np.float64)
        oz = np.array([list(z3) for (_t, z3, _c) in opaque], np.float64)
        orgb = np.array([c for (_t, _z, c) in opaque], np.float64)
        _raster_opaque_nb(buf, zbuf, oxy, oz, orgb)
    if glassf:
        gxy = np.array([[t[0], t[1], t[2]] for (_d, t, _z, _c, _a) in glassf], np.float64)
        gz = np.array([list(z3) for (_d, _t, z3, _c, _a) in glassf], np.float64)
        grgb = np.array([c for (_d, _t, _z, c, _a) in glassf], np.float64)
        ga = np.array([a for (_d, _t, _z, _c, a) in glassf], np.float64)
        _composite_glass_nb(buf, zbuf, gxy, gz, grgb, ga)


def _fit_scale(ext) -> float:
    """Catalog fit law: model grid -> normalized scale (cubes exact, ~2.88% rms)."""
    d = sorted(ext, reverse=True)        # [max, mid, min]
    return float(np.exp(0.1057) * d[0] ** (-0.8502) * d[1] ** (-0.0710) * d[2] ** (-0.0459))


def _fit_tight(center, mn, mx, eye, R, fy, H, cx, cy, margin_px) -> float:
    """Scale so the model's projected bounding box fills the frame (for thumbnails,
    not catalog-faithful). Binary search on the 8 bbox corners."""
    corners = np.array([[a, b, c] for a in (mn[0], mx[0] + 1.0)
                                   for b in (mn[1], mx[1] + 1.0)
                                   for c in (mn[2], mx[2] + 1.0)], float)
    target = H - 2 * margin_px
    lo, hi = 1e-4, 50.0
    for _ in range(40):
        s = (lo + hi) / 2
        scr, _ = _project((corners - center) * s, eye, R, fy, H, cx, cy)
        w = scr[:, 0].max() - scr[:, 0].min()
        h = scr[:, 1].max() - scr[:, 1].min()
        if max(w, h) <= target:
            lo = s
        else:
            hi = s
    return lo


def render_voxels(voxels: dict, dim: int = 256, params: dict | None = None) -> np.ndarray:
    """``{(x,y,z):(r,g,b,kind,level)}`` -> (dim,dim,4) uint8 RGBA."""
    if not voxels:
        raise BlueprintError("no voxels to render")
    P = dict(PARAMS); P.update(params or {})
    SS = int(P["ss"]); H = W = dim * SS
    cx_val = params.get("cx") if (params and "cx" in params) else (dim / 2.0)
    cy_val = params.get("cy") if (params and "cy" in params) else (dim / 2.0)
    cx, cy = cx_val * SS, cy_val * SS

    keys = np.array(list(voxels.keys()), float)
    mn, mx = keys.min(0), keys.max(0)
    center = (mn + mx + 1.0) / 2.0

    eye = _dir_from_angles(P["az"], P["el"]) * P["dist"]
    R = _look_at(eye, np.zeros(3), np.array([0.0, 1.0, 0.0]))
    fy = P["fov_y"]

    if P.get("scale_override") is not None:
        s = float(P["scale_override"])
    elif P.get("fit") == "tight":
        s = _fit_tight(center, mn, mx, eye, R, fy, H, cx, cy, 4 * SS)
    else:
        s = _fit_scale(mx - mn + 1.0)

    buf = np.zeros((H, W, 4), np.float64); zbuf = np.full((H, W), np.inf)
    ff = {"x": P["f_x"], "y": P["f_top"], "z": P["f_z"]}

    def face_color(key, r, g, b, kind):
        if kind in ("E", "GE"):
            return np.array([r, g, b], float)
        return np.minimum(255.0, np.array([r, g, b], float) * ff[key])

    # One batched projection of every face's 4 corners (was a _project call per face).
    faces = _faces(voxels)
    opaque, glassf = [], []
    if faces:
        allc = np.array([c for (_k, _v, c) in faces], np.float64)        # (Nf, 4, 3)
        nf = allc.shape[0]
        scr_all, z_all = _project((allc.reshape(-1, 3) - center) * s, eye, R, fy, H, cx, cy)
        scr_all = scr_all.reshape(nf, 4, 2); z_all = z_all.reshape(nf, 4)
        for i, (key, v, _corners) in enumerate(faces):
            r, g, b, kind, level = v[:5]      # the catalog tool renders flat: no specular pass
            scr = scr_all[i]; z = z_all[i]
            col = face_color(key, r, g, b, kind)
            tris = [((scr[0], scr[1], scr[2]), (z[0], z[1], z[2])),
                    ((scr[0], scr[2], scr[3]), (z[0], z[2], z[3]))]
            if kind in ("G", "GE"):
                a = (level / 255.0) ** 2 * 255.0
                depth = float(np.mean(z))
                for tri, z3 in tris:
                    glassf.append((depth, tri, z3, col, a))
            else:
                for tri, z3 in tris:
                    opaque.append((tri, z3, col))

    glassf.sort(key=lambda e: e[0], reverse=True)       # back-to-front composite order
    if _HAVE_NUMBA:
        _raster_batched(buf, zbuf, opaque, glassf)
    else:
        for tri, z3, col in opaque:
            _raster_opaque(buf, zbuf, tri, z3, col)
        for _, tri, z3, col, a in glassf:
            _composite_glass(buf, zbuf, tri, z3, col, a)

    sub = buf.reshape(dim, SS, dim, SS, 4)
    a = sub[..., 3:4]
    asum = a.sum(axis=(1, 3))
    rgb = (sub[..., :3] * a).sum(axis=(1, 3))
    out_rgb = np.where(asum > 0, rgb / np.maximum(asum, 1e-6), 0.0)
    out_a = a.mean(axis=(1, 3))
    out = np.concatenate([out_rgb, out_a], axis=2)
    return np.clip(out + 0.5, 0, 255).astype(np.uint8)


def _contain_square(rgba: np.ndarray, dim: int, pad_frac: float = 0.06) -> np.ndarray:
    """Crop to the opaque content and centre it in a dim×dim transparent square,
    scaled to fill (minus a small pad) -- so thumbnails are centred with no slack."""
    a = rgba[:, :, 3]
    ys, xs = np.nonzero(a > 8)
    if len(xs) == 0:
        return rgba
    crop = rgba[ys.min():ys.max() + 1, xs.min():xs.max() + 1]
    img = Image.fromarray(crop, "RGBA")
    pad = max(0, int(round(dim * pad_frac)))
    avail = max(1, dim - 2 * pad)
    scale = min(avail / img.width, avail / img.height)
    nw, nh = max(1, round(img.width * scale)), max(1, round(img.height * scale))
    img = img.resize((nw, nh), Image.LANCZOS)
    canvas = Image.new("RGBA", (dim, dim), (0, 0, 0, 0))
    canvas.paste(img, ((dim - nw) // 2, (dim - nh) // 2), img)
    return np.asarray(canvas)


def render_blueprint_png(data: bytes, dim: int = 256, params: dict | None = None,
                         contain: bool = False) -> bytes:
    """Decode a ``.blueprint`` and render it to a transparent PNG (bytes).

    ``contain=True`` (thumbnails): fill the frame, then crop-to-content + centre in a
    square -- ignores the catalog-faithful framing in favour of a tidy, centred image.
    """
    voxels = to_render_voxels(decode(data))
    p = dict(params or {})
    if contain:
        p.setdefault("fit", "tight")
    rgba = render_voxels(voxels, dim, p)
    if contain:
        rgba = _contain_square(rgba, dim)
    out = io.BytesIO()
    Image.fromarray(rgba, "RGBA").save(out, format="PNG", optimize=True)
    return out.getvalue()
