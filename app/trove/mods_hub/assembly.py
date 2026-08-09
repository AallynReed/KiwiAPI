"""Assemble a mod's blueprint voxel parts onto a creature's pre-baked rig (rest pose
+ animations), producing the model payload the web viewer consumes.

The rigs (``rigs/<name>.rig.json``) are baked OFFLINE on Windows from the game's
Granny files (skeleton + animations -> per-bone world matrices at the attach points).
This module is pure-Python and runs at runtime on any platform - no DLL, no Granny.
A rig is matched to a mod by suffix: the part ``..._leg_l_01`` attaches at AP key
``leg_l_01``; the rig sharing the most keys with the mod's parts wins.
"""
from __future__ import annotations

import base64
import glob
import json
import os
import re
import struct
import zlib
from functools import lru_cache

_RIG_DIR = os.path.join(os.path.dirname(__file__), "rigs")
_RIG_NAME_RE = re.compile(r"^[a-z0-9_]+$")   # skeleton / animation names; also blocks path traversal


@lru_cache(maxsize=1)
def _rigs() -> dict:
    out = {}
    for p in glob.glob(os.path.join(_RIG_DIR, "*.rig.json")):
        name = os.path.basename(p)[:-len(".rig.json")]
        with open(p, encoding="utf-8") as f:
            out[name] = json.load(f)
    return out


def load_animation(skeleton: str, name: str) -> dict | None:
    """Lazily load ONE baked animation's frames - ``{fps, frames:[{ap_key:[16]}]}`` from
    ``rigs/anim/<skeleton>/<name>.json`` - or None. The rig JSON only carries animation
    METADATA (name/fps/frame-count) so the model payload stays small; the viewer fetches
    the frames on demand when a clip is played. Names are validated (no path traversal)."""
    if not (_RIG_NAME_RE.match(skeleton or "") and _RIG_NAME_RE.match(name or "")):
        return None
    path = os.path.join(_RIG_DIR, "anim", skeleton, name + ".json")
    if not os.path.isfile(path):
        return None
    with open(path, encoding="utf-8") as f:
        return json.load(f)


def _decode_v5_grid(raw: bytes) -> list[tuple]:
    """Decode a v5 .blueprint to grid voxels ``(x, y, z, packed_rgb, kind, level, spec)``
    in part-local space, including the header's px,py,pz origin offset (so parts sit at
    their bone). ``kind``/``level``/``spec`` carry the material (glass alpha, glow, tint,
    specular map) via the same ground-truth mapping as the catalog renderer, so the
    viewer shades it like the game instead of as flat opaque cubes."""
    from app.trove.render.voxel import KIND_CODE, material_for
    d = zlib.decompressobj()
    body = d.decompress(raw[9:]) + d.flush()
    px, py, pz, sx, sy, sz, count, start = struct.unpack_from("<8i", body, 0)
    off = 32
    deltas = struct.unpack_from(f"<{count-1}i", body, off) if count > 1 else (); off += 4*(count-1)
    types = struct.unpack_from(f"<{count}H", body, off); off += 2*count
    colors = struct.unpack_from(f"<{count}I", body, off); off += 4*count
    idx = [start]
    for dd in deltas:
        idx.append(idx[-1] + dd)
    plane = sx*sz
    out = []
    for L, t, c in zip(idx, types, colors, strict=False):
        y = L // plane; rem = L % plane; z = rem // sx; x = rem % sx
        r, g, b, kind, level, spec = material_for((c >> 16) & 255, (c >> 8) & 255, c & 255,
                                                  (c >> 24) & 255, t)
        out.append((px+x, py+y, pz+z, (r << 16) | (g << 8) | b, KIND_CODE[kind], level, spec))
    return out


def has_baked_rig(name: str) -> bool:
    """Whether a rig of this skeleton name is baked + available."""
    return name in _rigs()


def animations_for(name: str) -> list[str]:
    """Animation names baked for a rig (empty if rest-pose-only / unknown)."""
    rig = _rigs().get(name)
    return list(rig.get("animations", {}).keys()) if rig else []


def assemble(tmod_files: list[dict], rig_name: str | None, ap_overrides: dict[str, str]) -> dict | None:
    """tmod_files = ``read_tmod(...)["files"]`` (with content_base64). Returns the
    web-viewer model payload, or None if the rig isn't known/baked or nothing places.

    ``rig_name`` + ``ap_overrides`` (blueprint basename -> exact AP key) come ONLY from
    the authoritative binfab resolution (``rig_index.resolve``). There is NO name-overlap
    heuristic: we either KNOW where each part attaches (from the game's own prefab data)
    or we don't render it. A part with no override is skipped - we never guess a creature
    or an attach point, which would render onto the wrong model."""
    from app.trove.render.voxel import is_empty_blueprint

    rigs = _rigs()
    if not rig_name or rig_name not in rigs:
        return None                              # unknown rig -> don't render (no guess)
    rig = rigs[rig_name]
    ap_overrides = ap_overrides or {}

    parts = []
    for f in tmod_files:
        p = f["path"].lower()
        if not p.endswith(".blueprint") or "content_base64" not in f:
            continue
        key = ap_overrides.get(p.split("/")[-1][:-len(".blueprint")])   # exact AP, or None
        if not key or key not in rig["rest"]:
            continue                             # not a known part of this rig -> skip
        raw = base64.b64decode(f["content_base64"])
        if (raw[:5] != b"kiwib" or struct.unpack_from("<I", raw, 5)[0] != 5
                or is_empty_blueprint(raw)):
            continue
        vox = _decode_v5_grid(raw)
        if not vox:
            continue
        part = {"name": key,
                "x": [v[0] for v in vox], "y": [v[1] for v in vox], "z": [v[2] for v in vox],
                "rgb": [v[3] for v in vox], "kind": [v[4] for v in vox], "level": [v[5] for v in vox]}
        spec = [v[6] for v in vox]
        if any(spec):                    # all-rough is the common case: don't ship the array
            part["spec"] = spec
        parts.append(part)
    if not parts:
        return None
    _unbury_enclosed_emissive(parts, rig["rest"], rig["voxel_scale"])
    return {"voxel_scale": rig["voxel_scale"], "rig": rig_name, "parts": parts,
            "rest": rig["rest"], "animations": rig["animations"]}


def assemble_voxels(parts: list[tuple[str, bytes]], rig_name: str) -> dict:
    """``[(AP key, raw .blueprint bytes)]`` -> ``{(x,y,z): (r,g,b,kind,level,spec)}`` in
    rig space, ready for ``render.voxel.render_voxels``.

    The web viewer gets its parts in LOCAL space plus the rest-pose matrices and does the
    transform on the GPU; a server-side still image has to bake the rest pose in here.
    Same matrices, same voxel scale, so the two agree. Returns ``{}`` for an unknown rig
    or when nothing places - the caller then falls back to a single blueprint rather than
    showing a half-creature.
    """
    import numpy as np

    rig = _rigs().get(rig_name)
    if not rig:
        return {}
    scale = rig["voxel_scale"] or 1.0
    rest = rig["rest"]
    out: dict = {}
    for ap_key, raw in parts:
        mat = rest.get(ap_key)
        if mat is None or not raw:
            continue
        if raw[:5] != b"kiwib" or struct.unpack_from("<I", raw, 5)[0] != 5:
            continue
        voxels = _decode_v5_grid(raw)
        if not voxels:
            continue
        m = np.array(mat).reshape(4, 4).T @ np.diag([scale] * 3 + [1.0])
        n = len(voxels)
        local = np.array([[v[0] for v in voxels], [v[1] for v in voxels],
                          [v[2] for v in voxels], [1.0] * n], dtype=float)
        # back into voxel units so the renderer's grid maths still applies
        world = (m @ local)[:3].T / scale
        for (wx, wy, wz), v in zip(world, voxels, strict=False):
            rgb = v[3]
            out[(int(round(wx)), int(round(wy)), int(round(wz)))] = (
                (rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255, v[4], v[5], v[6])
    return out


def _unbury_enclosed_emissive(parts: list[dict], rest: dict, voxel_scale: float) -> None:
    """Push pure-emissive accent parts (eyes, runes) that are FULLY enclosed by other
    parts' voxels OUTWARD so they protrude past the covering geometry and render with
    NORMAL depth - visible from the front, occluded from behind (no see-through). A
    mod's solid head with no carved eye-socket otherwise buries the glowing eyes.
    Surface glow (a dragon's body) keeps an exposed face -> not enclosed -> untouched.
    Rest-pose; the offset is baked into each voxel's local coords so it rides the bone
    through animation. Mutates ``parts`` (shifts the enclosed parts' x/y/z)."""
    import numpy as np

    cell = voxel_scale or 1.0
    tol = cell * 0.6                              # voxels from different bones don't share an
    grid: dict = {}                              # axis grid (rotation) -> per-axis box match
    worlds: list = []
    mats: list = []
    for p in parts:
        m = np.array(rest[p["name"]]).reshape(4, 4).T @ np.diag([voxel_scale] * 3 + [1.0])
        mats.append(m)
        n = len(p["x"])
        loc = np.array([p["x"], p["y"], p["z"], [1.0] * n], dtype=float)
        w = (m @ loc)[:3].T
        worlds.append(w)
        for pt in w:
            grid.setdefault((round(pt[0] / cell), round(pt[1] / cell), round(pt[2] / cell)), []).append(pt)
    model_centre = np.vstack(worlds).mean(0)

    neigh27 = [(dx, dy, dz) for dx in (-1, 0, 1) for dy in (-1, 0, 1) for dz in (-1, 0, 1)]
    axes = [(cell, 0, 0), (-cell, 0, 0), (0, cell, 0), (0, -cell, 0), (0, 0, cell), (0, 0, -cell)]

    def occupied(q) -> bool:
        b = (round(q[0] / cell), round(q[1] / cell), round(q[2] / cell))
        for dx, dy, dz in neigh27:
            for pt in grid.get((b[0] + dx, b[1] + dy, b[2] + dz), ()):  # any voxel near q?
                if abs(pt[0] - q[0]) < tol and abs(pt[1] - q[1]) < tol and abs(pt[2] - q[2]) < tol:
                    return True
        return False

    for p, w, m in zip(parts, worlds, mats, strict=False):
        kinds = p["kind"]
        if not kinds or sum(1 for k in kinds if k in (2, 3)) / len(kinds) < 0.5:
            continue                      # only un-bury (mostly) emissive accent parts
        enclosed = True
        for pt in w:
            if any(not occupied(pt + ax) for ax in axes):   # a neighbour cell is empty
                enclosed = False                            # -> this face is already visible
                break
        if not enclosed:
            continue
        outward = w.mean(0) - model_centre              # push away from the body centre
        norm = float(np.linalg.norm(outward))
        if norm < 1e-9:
            continue
        delta = np.linalg.inv(m[:3, :3]) @ ((outward / norm) * (1.5 * voxel_scale))  # ~1.5 voxels proud
        dx, dy, dz = float(delta[0]), float(delta[1]), float(delta[2])
        p["x"] = [x + dx for x in p["x"]]
        p["y"] = [y + dy for y in p["y"]]
        p["z"] = [z + dz for z in p["z"]]
