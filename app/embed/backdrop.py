"""What an aura is worn on, drawn behind its effect in the VFX preview.

An aura plays at the equipment's VFX bone, which the class socket table names next to
the model's own bone (see app/trove/dressing/sockets.py). For a hat and every weapon but
the bow the two are the same bone, so the effect's origin is the model's attachment
point. Bows play at ``VFX_weapon_l``, which the baked rigs don't carry, so a bow gets no
backdrop rather than a guessed one.

Effect space is half the size of the world the Dressing Room builds: a weapon voxel
(1/12 there) is 1/24 here. That is measured, not tuned: the spawn extents of every
weapon-aura family line up with its starter weapon at that size (melee, staff and spear
effects run up +Z from the grip; the pistol's cover its barrel to the voxel), and at
the same factor the hat auras orbit and wrap the default head instead of sitting inside
it.

The weapon is the family's starter (``prefabs/equipment/starterweapon_<class>``), taken
from the first class alphabetically whose starter has an attachment point; a v3/v4
blueprint stores none. The head is the default Knight's race head and eyes, placed where
the rig puts them relative to its hat bone.
"""
from __future__ import annotations

import numpy as np

from app.core.config import settings
from app.trove.codexes import pg_store
from app.trove.dressing import catalogue as cat
from app.trove.dressing import palette
from app.trove.dressing import service as dress
from app.trove.mods_hub import assembly
from app.trove.render import bp_cache
from app.trove.render.source import get_blueprint_bytes

WEAPON_FAMILIES = {"melee": "Melee", "pistol": "Gun", "staff": "Staff", "spear": "Spear", "fist": "Fist"}
KINDS = (*WEAPON_FAMILIES, "hat")
EFFECT_PER_WORLD = 0.5
HAT_CLASS = "knight"

_cache: dict[tuple, dict | None] = {}


def _part(raw: bytes, matrix: np.ndarray) -> dict | None:
    part = assembly._part_at("model", raw, 1.0)
    if not part:
        return None
    return {"m": [round(float(v), 6) for v in matrix.T.flatten()],
            "x": part["x"], "y": part["y"], "z": part["z"], "rgb": part["rgb"]}


async def _read(opt_blueprint: str, hint: str, ref: str, branch: str) -> bytes | None:
    path = await dress.blueprint_path(opt_blueprint, hint, branch, ref)
    return await get_blueprint_bytes(path, branch) if path else None


async def _weapon(family: str, catalogue: cat.Catalogue, branch: str) -> dict | None:
    starters = sorted((o for o in catalogue.styles.get("weapon", [])
                       if o.key.startswith("starterweapon_") and o.family == family),
                      key=lambda o: o.key)
    for opt in starters:
        raw = await _read(opt.blueprint, opt.prefab, opt.ref, branch)
        if not raw or raw[:5] != b"kiwib" or raw[5] != 5:
            continue                      # no stored attachment point to seat it by
        part = _part(raw, np.diag([EFFECT_PER_WORLD / 12] * 3 + [1.0]))
        if part:
            return {"label": opt.name, "parts": [part]}
    return None


async def _head(branch: str) -> dict | None:
    outfit = await dress.resolve(HAT_CLASS, None, {}, branch,
                                 colors={"eye_color": palette.EYE[0]})
    if outfit is None:
        return None
    skeleton = outfit.cls.skeleton
    placements = []
    for slot in ("head", "eyes"):
        ref = outfit.blueprints.get(slot)
        ap = dress.attach_point(slot, skeleton)
        if not ref or ref == dress.NONE or not ap:
            continue
        raw = await _read(ref.rsplit("/", 1)[-1], ref, ref if "/" in ref else "", branch)
        if raw:
            placements.append((ap, raw, outfit.piece_scale(slot, ap), outfit.tint_for(slot)))
    built = assembly.assemble_placements(placements, skeleton) if placements else None
    rest = built["rest"] if built else {}
    if "hat" not in rest:
        return None
    to_hat = np.diag([EFFECT_PER_WORLD] * 3 + [1.0]) @ np.linalg.inv(np.array(rest["hat"], dtype=float).reshape(4, 4).T)
    parts = []
    for p in built["parts"]:
        size = built["voxel_scale"] * float(p.get("scale", 1.0))
        m = to_hat @ np.array(rest[p["name"]], dtype=float).reshape(4, 4).T @ np.diag([size] * 3 + [1.0])
        parts.append({"m": [round(float(v), 6) for v in m.T.flatten()],
                      "x": p["x"], "y": p["y"], "z": p["z"], "rgb": p["rgb"]})
    return {"label": "Head", "parts": parts} if parts else None


async def backdrop(kind: str, branch: str | None = None) -> dict | None:
    """``{label, parts: [{m, x, y, z, rgb}]}``: voxels in local grid units and the
    column-major matrix that places them in effect space. None when there is nothing
    authoritative to draw."""
    branch = branch or settings.trove_render_branch
    if kind not in KINDS or not settings.postgres_enabled:
        return None
    key = (kind, branch, await pg_store.meta_signature(branch), bp_cache.ASSEMBLY_VERSION)
    if key not in _cache:
        catalogue = await cat.get(branch)
        _cache[key] = await (_head(branch) if kind == "hat"
                             else _weapon(WEAPON_FAMILIES[kind], catalogue, branch))
    return _cache[key]
