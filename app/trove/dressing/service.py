"""Turn a chosen outfit into the 3D model payload the viewer already knows how to draw.

The selection is stable game identifiers - a class, a costume and a style per slot - so a
shared link keeps working across game updates without anything being stored: nothing here
is a database row id, and nothing is remembered after the response.

Resolution is the game's, end to end. The class prefab says which skeleton to use and
which sockets it has; the rig map says where each costume part attaches; the style's own
slot number says which socket it fits. A style whose family this class has no socket for
is reported back as dropped rather than placed somewhere plausible.

**A slot also takes a raw blueprint.** Not every appearance in the game is a style prefab
the catalogue can list, and a partner embedding the viewer may have a blueprint name and
nothing else - so any slot accepts one directly, resolved through the archive exactly like
a costume part (``equipment_hat_x[author]``, or a full path when the name is ambiguous).
That is an instruction, not a guess: the caller says which slot, and the class's own socket
table still decides which attach point that slot means. ``head`` and ``hair`` are
blueprint-only - Trove has no styles for them - and exist because a face style is a face,
not a head: ``equipment_face_movember_01`` is a moustache and needs something to sit on.
"""

from __future__ import annotations

import asyncio
import logging
import re
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
# Slots a raw blueprint can fill. head/hair have no catalogue behind them at all.
BLUEPRINT_SLOTS = ("head", "hair", "hat", "face", "weapon")
# slot -> the socket family that names its attach point on any class.
SLOT_FAMILY = {"hat": "Hat", "face": "Face"}
# head/hair aren't equipment sockets - they're attach points every player rig carries,
# so they're addressed by AP name directly.
DIRECT_APS = {"head": "head", "hair": "hair"}


@dataclass(frozen=True)
class Outfit:
    """A resolved, canonical selection - what the URL means after validation."""

    cls: cat.DressClass
    costume: cat.Option
    styles: dict[str, cat.Option] = field(default_factory=dict)   # slot -> option
    blueprints: dict[str, str] = field(default_factory=dict)      # slot -> raw blueprint ref
    weapon_family: str = ""                                       # for a raw weapon blueprint
    dropped: list[str] = field(default_factory=list)              # slots we couldn't honour

    @property
    def ident(self) -> str:
        """Cache identity: the same outfit always assembles to the same model."""
        picks = ",".join(f"{s}={self.styles[s].key}" for s in STYLE_SLOTS if s in self.styles)
        raw = ",".join(f"{s}~{self.blueprints[s]}" for s in BLUEPRINT_SLOTS
                       if s in self.blueprints)
        fam = f"@{self.weapon_family}" if self.weapon_family else ""
        return f"{self.cls.key}/{self.costume.key}/{picks}/{raw}{fam}"

    def as_dict(self) -> dict:
        out = {"class": self.cls.key, "costume": self.costume.key}
        for slot in BLUEPRINT_SLOTS:
            out[slot] = (self.styles[slot].key if slot in self.styles
                         else self.blueprints.get(slot))
        return out


# A blueprint reference: a name, optionally under folders, optionally carrying Trove's
# `[author]` style suffix. Bounded and charset-locked BEFORE anything looks it up, and
# resolved only through the archive's own index - never joined onto a filesystem path.
_BP_RE = re.compile(r"^[a-z0-9_][a-z0-9_.\-\[\]]*(?:/[a-z0-9_][a-z0-9_.\-\[\]]*)*$")
_MAX_BP = 200


def blueprint_ref(value: str | None) -> str | None:
    """``value`` as a usable blueprint reference, or None if it isn't shaped like one."""
    v = (value or "").strip().lower().replace("\\", "/")
    if not v or len(v) > _MAX_BP or ".." in v:
        return None
    v = v.removesuffix(".blueprint")
    return v if v and _BP_RE.match(v) else None


async def resolve(
    class_key: str, costume: str | None, picks: dict[str, str | None],
    branch: str | None = None, weapon_family: str | None = None,
) -> Outfit | None:
    """Validate a selection against the catalogue and normalise it.

    Returns None only when the class itself is unknown - an unknown or incompatible
    costume falls back to that class's first costume, and an unusable style is dropped
    and named in ``dropped``, so a stale link still shows a character.

    A slot value that isn't a catalogue key is tried as a blueprint reference, so the
    catalogue keeps winning for the names it owns and a partner can still name anything
    the game ships."""
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
    raw: dict[str, str] = {}
    dropped: list[str] = []
    for slot in BLUEPRINT_SLOTS:
        key = (picks.get(slot) or "").strip().lower()
        if not key:
            continue
        opt = catalogue.get(slot, key) if slot in STYLE_SLOTS else None
        if opt is not None:
            if not sockets_mod.sockets_for_slot(cls.sockets, opt.slot_id):
                dropped.append(slot)          # this class has no socket for that family
                continue
            styles[slot] = opt
            continue
        ref = blueprint_ref(key)
        # Check it EXISTS here rather than at placement time, so /outfit answers "is this
        # name usable" - which is the question a partner wiring up an embed is asking.
        if ref is None or not await blueprint_path(ref.rsplit("/", 1)[-1], ref, branch):
            dropped.append(slot)              # neither a style we know nor a name we have
            continue
        raw[slot] = ref

    # Only needed to place a raw WEAPON blueprint, which carries no slot number of its
    # own. Named explicitly it resolves through the class's socket table like anything
    # else; unnamed it takes the family the class declares first, which is unambiguous
    # for every class except the Boomeranger (bow and melee).
    fam = (weapon_family or "").strip()
    if fam and not sockets_mod.sockets_for_family(cls.sockets, fam):
        fam = ""
    if not fam and "weapon" in raw:
        fam = cls.weapons[0] if cls.weapons else ""
    return Outfit(cls=cls, costume=chosen, styles=styles, blueprints=raw,
                  weapon_family=fam, dropped=dropped)


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
        # which is what makes the Candy Barbarian hold two identical swords. The socket
        # decides the scale: hat/face art is authored at double the body's resolution,
        # a weapon's is not (see assembly.scale_for).
        for socket in sockets_mod.sockets_for_slot(outfit.cls.sockets, opt.slot_id):
            out.append((socket["ap"], raw, assembly.scale_for(socket["ap"])))

    for slot, ref in outfit.blueprints.items():
        # The caller named the file; the class's socket table still names the bone.
        # head/hair are plain attach points rather than equipment sockets.
        if slot in DIRECT_APS:
            aps = [DIRECT_APS[slot]]
        elif slot in SLOT_FAMILY:
            aps = [s["ap"] for s in sockets_mod.sockets_for_family(
                outfit.cls.sockets, SLOT_FAMILY[slot])]
        else:
            aps = [s["ap"] for s in sockets_mod.sockets_for_family(
                outfit.cls.sockets, outfit.weapon_family)]
        if not aps:
            continue
        # A bare basename is looked up in the archive index; a full path resolves as
        # given. The hint disambiguates a name Trove reuses across skins.
        data = await read(ref.rsplit("/", 1)[-1], ref)
        if not data:
            continue
        for ap in aps:
            out.append((ap, data, assembly.scale_for(ap)))
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
