"""Turn a chosen outfit into the 3D model payload the viewer already knows how to draw.

The selection is five stable game identifiers - a class, a costume and up to three style
prefab stems - so a shared link keeps working across game updates without anything being
stored: nothing here is a database row id, and nothing is remembered after the response.

Resolution is the game's, end to end. The class prefab says which skeleton to use and
which sockets it has; the rig map says where each costume part attaches; the style's own
slot number says which socket it fits. A style whose family this class has no socket for
is reported back as dropped rather than placed somewhere plausible.
"""

from __future__ import annotations

import asyncio
import logging
from dataclasses import dataclass, field

from app.core.config import settings
from app.trove.dressing import catalogue as cat
from app.trove.dressing import sockets as sockets_mod
from app.trove.mods_hub import assembly, rig_index
from app.trove.mods_hub.trove_layout import game_file_paths, nearest_path
from app.trove.render import bp_cache
from app.trove.render.source import blueprint_by_basename, get_blueprint_bytes

logger = logging.getLogger("kiwi.dressing")

STYLE_SLOTS = ("hat", "face", "weapon")


@dataclass(frozen=True)
class Outfit:
    """A resolved, canonical selection - what the URL means after validation."""

    cls: cat.DressClass
    costume: cat.Option
    styles: dict[str, cat.Option] = field(default_factory=dict)   # slot -> option
    dropped: list[str] = field(default_factory=list)              # slots we couldn't honour

    @property
    def ident(self) -> str:
        """Cache identity: the same outfit always assembles to the same model."""
        picks = ",".join(f"{s}={self.styles[s].key}" for s in STYLE_SLOTS if s in self.styles)
        return f"{self.cls.key}/{self.costume.key}/{picks}"

    def as_dict(self) -> dict:
        out = {"class": self.cls.key, "costume": self.costume.key}
        for slot in STYLE_SLOTS:
            out[slot] = self.styles[slot].key if slot in self.styles else None
        return out


async def resolve(
    class_key: str, costume: str | None, picks: dict[str, str | None],
    branch: str | None = None,
) -> Outfit | None:
    """Validate a selection against the catalogue and normalise it.

    Returns None only when the class itself is unknown - an unknown or incompatible
    costume falls back to that class's first costume, and an unusable style is dropped
    and named in ``dropped``, so a stale link still shows a character."""
    catalogue = await cat.get(branch)
    cls = catalogue.classes.get((class_key or "").lower())
    if not cls:
        return None
    wardrobe = catalogue.costumes.get(cls.key) or []
    if not wardrobe:
        return None

    chosen = catalogue.get("costume", (costume or "").lower())
    if chosen is None or chosen.skeleton != cls.skeleton:
        chosen = wardrobe[0]                  # a costume for another class -> this class's default

    styles: dict[str, cat.Option] = {}
    dropped: list[str] = []
    for slot in STYLE_SLOTS:
        key = (picks.get(slot) or "").lower()
        if not key:
            continue
        opt = catalogue.get(slot, key)
        if opt is None or not sockets_mod.sockets_for_slot(cls.sockets, opt.slot_id):
            dropped.append(slot)              # this class has no socket for that family
            continue
        styles[slot] = opt
    return Outfit(cls=cls, costume=chosen, styles=styles, dropped=dropped)


async def blueprint_path(basename: str, hint: str, branch: str | None = None) -> str | None:
    """Where a part's blueprint actually lives. The catalogue stores basenames, but the
    archive keys on full logical paths (``2025/equipment/…``), so a bare name resolves to
    nothing without this - and a basename Trove reuses across skins and NPC sets is
    absent from the unambiguous index, so fall back to every archived path and take the
    one closest to the prefab that asked for it (the creature renderer's rule)."""
    branch = branch or settings.trove_render_branch
    index = await blueprint_by_basename(branch)
    found = index.get(basename)
    if found:
        return found
    all_paths = await game_file_paths(branch)
    return nearest_path(all_paths.get(f"{basename}.blueprint", []), hint)


async def _placements(outfit: Outfit, branch: str) -> list[tuple[str, bytes, float]]:
    """``[(AP key, blueprint bytes, scale)]`` for everything the outfit puts on the rig."""

    async def read(basename: str, hint: str) -> bytes | None:
        path = await blueprint_path(basename, hint, branch)
        return await get_blueprint_bytes(path, branch) if path else None

    out: list[tuple[str, bytes, float]] = []
    for basename, ap_key in outfit.costume.parts.items():
        raw = await read(basename, outfit.costume.prefab)
        if raw:
            out.append((ap_key, raw, 1.0))

    for slot in STYLE_SLOTS:
        opt = outfit.styles.get(slot)
        if not opt:
            continue
        raw = await read(opt.blueprint, opt.prefab)
        if not raw:
            continue
        # One equipped style, drawn once per socket the class declares for its family -
        # which is what makes the Candy Barbarian hold two identical swords.
        for socket in sockets_mod.sockets_for_slot(outfit.cls.sockets, opt.slot_id):
            out.append((socket["ap"], raw, assembly.EQUIPMENT_SCALE))
    return out


async def model(outfit: Outfit, fmt: str = "json", branch: str | None = None) -> bp_cache.Cached | None:
    """The assembled model payload for an outfit, cached like any other assembly.

    An outfit is made of content-addressed game files and a rig map, so the result only
    moves when the game does - which is exactly what the index signature tracks."""
    branch = branch or settings.trove_render_branch

    async def build() -> dict:
        placements = await _placements(outfit, branch)
        if not placements:
            raise bp_cache.NoPayload
        built = await asyncio.to_thread(
            assembly.assemble_placements, placements, outfit.cls.skeleton)
        if built is None:
            raise bp_cache.NoPayload
        return built

    sig = await rig_index.index_signature(branch)
    try:
        if sig is None:
            return await bp_cache.build_uncached(build, fmt)
        return await bp_cache.get_or_build(
            bp_cache.key_for_assembly(sig, f"dress:{outfit.ident}"), build, fmt)
    except bp_cache.NoPayload:
        return None
