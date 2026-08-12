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

# Why a slot the caller asked for is not on the model. Reported per slot rather than
# left to be inferred from a part count - a component that vanishes without a word is
# indistinguishable from one we drew wrong.
DROP_UNKNOWN = "unknown"        # neither a style we know nor a blueprint the game ships
DROP_NO_SOCKET = "no_socket"    # this class has no attach point for that family
DROP_MISSING = "missing_asset"  # a name we know, whose blueprint the archive lacks
DROP_COVERED = "covered"        # deliberate: a full helmet replaces the hair and face

STYLE_SLOTS = ("hat", "face", "weapon")
# The character-creation slots (see customhead.py): a catalogue backs them, and each
# attaches at the point the game's own prefabs bind that mesh to.
RACE_SLOTS = ("head", "hair", "eyes")
# Slots a raw blueprint can fill.
BLUEPRINT_SLOTS = ("head", "hair", "eyes", "hat", "face", "weapon")
# slot -> the socket family that names its attach point on any class.
SLOT_FAMILY = {"hat": "Hat", "face": "Face"}
# head/hair/eyes aren't equipment sockets - they're attach points on the rig, addressed
# by name, in order of preference.
#
# Hair falls back to `head` and that is an IDENTITY, not a guess: on every rig carrying
# both, `AP_hair` and `AP_head` are the same transform to 1e-5 (38 of 40 corpus-wide; the
# two that differ are bosses, not classes). Five class rigs - Bard, Boomeranger, Lunar
# Lancer, Pirate Captain, Revenant - ship no `AP_hair` at all, and the game plainly does
# show hair on them, so the point they share is where it goes.
DIRECT_APS = {"head": ("head",), "hair": ("hair", "head"), "eyes": ("face",)}


def attach_point(slot: str, skeleton: str) -> str | None:
    """The attach point this rig uses for a character-creation slot, or None."""
    for ap in DIRECT_APS.get(slot, ()):
        if assembly.has_ap(skeleton, ap):
            return ap
    return None
# "I want nothing here" - distinct from "I didn't choose", which takes the race default.
NONE = "none"

# The head and eyes are drawn at the same half scale as everything else on the head - the
# size that was confirmed correct by eye. DO NOT "derive" this from voxel counts: doing
# that once put the head back to double size while fixing something else entirely.
#
# Hair is the one that is still wrong (too big), and it gets its own number so it can be
# corrected WITHOUT touching the head again.
PIECE_SCALE = {"head": 0.5, "eyes": 0.5, "hair": 0.5}
# Eyebrows live in the head blueprint but take the HAIR colour, as they do in game.
# They are the only near-pure-red voxels a human head carries (4 of 992: two shades,
# mirrored), and the strict threshold keeps a lizard's or ghost's saturated skin out
# of it - a head with no brows (skull, robot) simply has none to find.
BROW_MAX_OFF = 24

# slot -> the colour parameter that tints it (see assembly.recolor). Hair and eyes only,
# which is exactly what Trove's own customizer offers: ui/charcustomize.swf has RACE,
# HAIRSTYLE, EYECOLOR and HAIRCOLOR and no skin colour. The data agrees - hair and eyes
# are masks at ~1.0 saturation, while a head is authored in a real skin tone at ~0.16, so
# there is nothing to mask. You change a character's skin by changing its race.
SLOT_COLOR = {"hair": "hair_color", "eyes": "eye_color"}
_HEX_RE = re.compile(r"^#?([0-9a-f]{6})$")


def color_ref(value: str | None) -> tuple[int, int, int] | None:
    """``#rrggbb`` -> (r, g, b), or None if it isn't one."""
    m = _HEX_RE.match((value or "").strip().lower())
    if not m:
        return None
    v = int(m.group(1), 16)
    return ((v >> 16) & 255, (v >> 8) & 255, v & 255)


@dataclass(frozen=True)
class Outfit:
    """A resolved, canonical selection - what the URL means after validation."""

    cls: cat.DressClass
    costume: cat.Option
    race: cat.Race | None = None
    styles: dict[str, cat.Option] = field(default_factory=dict)   # slot -> option
    blueprints: dict[str, str] = field(default_factory=dict)      # slot -> raw blueprint ref
    weapon_family: str = ""                                       # for a raw weapon blueprint
    colors: dict[str, tuple[int, int, int]] = field(default_factory=dict)   # slot -> rgb
    hair_scale: float = 0.5                                       # calibration knob
    dropped: list[str] = field(default_factory=list)              # slots we couldn't honour
    # Why each one is missing, so a caller is never left comparing voxel counts to work
    # out that the hat it asked for is not there: [{slot, value, reason}], reason being
    # one of DROP_UNKNOWN / DROP_NO_SOCKET / DROP_MISSING / DROP_COVERED.
    issues: list[dict] = field(default_factory=list)

    @property
    def issue_header(self) -> str:
        """``hat=unknown,face=covered`` - the same list on one response header, for a
        caller that renders the model and never asks /outfit."""
        return ",".join(f"{i['slot']}={i['reason']}" for i in self.issues)

    @property
    def ident(self) -> str:
        """Cache identity: the same outfit always assembles to the same model.

        EVERY slot has to appear. It listed only the equipment styles once, so two
        outfits differing solely by a catalogue-chosen head, hair or eyes shared one key
        and the first one built was served for all of them."""
        slots = tuple(dict.fromkeys(STYLE_SLOTS + RACE_SLOTS + BLUEPRINT_SLOTS))
        picks = ",".join(f"{s}={self.styles[s].key}" for s in slots if s in self.styles)
        raw = ",".join(f"{s}~{self.blueprints[s]}" for s in slots if s in self.blueprints)
        fam = f"@{self.weapon_family}" if self.weapon_family else ""
        tints = ",".join(f"{s}#{self.colors[s][0]:02x}{self.colors[s][1]:02x}"
                         f"{self.colors[s][2]:02x}" for s in sorted(self.colors))
        fam = f"{fam}({tints})" if tints else fam
        race = self.race.key if self.race else ""
        if self.hair_scale != PIECE_SCALE["hair"]:
            fam = f"{fam}~h{self.hair_scale}"
        return f"{self.cls.key}/{self.costume.key}/{race}/{picks}/{raw}{fam}"

    def tint_for(self, slot: str):
        """The recolour for a slot: its own colour, or - for a head - the hair colour
        applied to the eyebrows alone."""
        own = self.colors.get(slot)
        if own:
            return own
        if slot == "head" and self.colors.get("hair"):
            return (self.colors["hair"], BROW_MAX_OFF)
        return None

    def piece_scale(self, slot: str) -> float:
        """Voxel-size multiplier for a character-creation piece."""
        return self.hair_scale if slot == "hair" else PIECE_SCALE[slot]

    def as_dict(self) -> dict:
        out = {"class": self.cls.key, "costume": self.costume.key,
               "race": self.race.key if self.race else None}
        for slot in BLUEPRINT_SLOTS:
            out[slot] = (self.styles[slot].key if slot in self.styles
                         else self.blueprints.get(slot))
        for slot, param in SLOT_COLOR.items():
            rgb = self.colors.get(slot)
            out[param] = f"#{rgb[0]:02x}{rgb[1]:02x}{rgb[2]:02x}" if rgb else None
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
    race: str | None = None, colors: dict[str, str | None] | None = None,
    hair_scale: float | None = None,
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

    # The race supplies the head and eyes the game draws whether or not anything covers
    # them - a character is never faceless. An unknown race falls back to the first that
    # has a head, rather than to none.
    chosen_race = catalogue.races.get((race or "").lower())
    if chosen_race is None:
        chosen_race = next((r for r in catalogue.races.values() if r.first("head")), None)

    styles: dict[str, cat.Option] = {}
    raw: dict[str, str] = {}
    issues: list[dict] = []

    def drop(slot: str, value: str, reason: str) -> None:
        issues.append({"slot": slot, "value": value, "reason": reason})

    for slot in BLUEPRINT_SLOTS:
        key = (picks.get(slot) or "").strip().lower()
        if not key:
            continue
        if key == NONE:
            raw[slot] = NONE                  # explicit "leave this empty"
            continue
        opt = catalogue.get(slot, key)
        if opt is not None:
            # A head/hair/eyes piece isn't equipment: it has no slot number and rides an
            # attach point every rig has, so there is no compatibility to check.
            if slot not in RACE_SLOTS and not sockets_mod.sockets_for_slot(
                    cls.sockets, opt.slot_id):
                drop(slot, key, DROP_NO_SOCKET)
                continue
            # A catalogue entry is a promise the game names this appearance, not that the
            # archive still carries its blueprint. Checking here is what turns "the hat
            # silently isn't there" into an answer.
            if not await blueprint_path(opt.blueprint, opt.prefab, branch, opt.ref):
                drop(slot, key, DROP_MISSING)
                continue
            styles[slot] = opt
            continue
        ref = blueprint_ref(key)
        # Check it EXISTS here rather than at placement time, so /outfit answers "is this
        # name usable" - which is the question a partner wiring up an embed is asking.
        if ref is None or not await blueprint_path(ref.rsplit("/", 1)[-1], ref, branch,
                                                   ref if "/" in ref else ""):
            drop(slot, key, DROP_UNKNOWN)
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
    # A full helmet replaces the hair and the face style; the eyes stay, showing through
    # it the way the game draws them. Dropping the face style is what lets them: the eyes
    # ride the same attach point, so with the mask gone the race's own eyes take it back.
    hat = styles.get("hat")
    if hat is not None and hat.covers_head:
        for slot in ("hair", "face"):
            asked = styles.pop(slot, None) or raw.pop(slot, None)
            if asked is not None:
                drop(slot, picks.get(slot) or "", DROP_COVERED)
            else:
                raw.pop(slot, None)

    # Nothing chosen for a character-creation slot -> the race's own default, so the
    # model matches what the game shows a player who never opened the customizer. Hair
    # is not defaulted: "no hair" is a real look, and the game's own default is Bald.
    if chosen_race:
        for slot in ("head", "eyes"):
            if slot in styles or slot in raw:
                continue
            # A face style occupies the same attach point as the eyes, so equipping one
            # replaces them - which is exactly what the game does when you put on a mask.
            if slot == "eyes" and ("face" in styles or "face" in raw):
                continue
            default = chosen_race.first(slot)
            if default:
                raw[slot] = default.lower()
    # A slot the rig genuinely has nowhere to put is reported rather than dropped in
    # silence. With hair's fallback in place this is now rare.
    for slot in RACE_SLOTS:
        if (slot in styles or slot in raw) and not attach_point(slot, cls.skeleton):
            styles.pop(slot, None)
            raw.pop(slot, None)
            if not any(i["slot"] == slot for i in issues):
                drop(slot, picks.get(slot) or "", DROP_NO_SOCKET)

    tints = {}
    for slot, param in SLOT_COLOR.items():
        rgb = color_ref((colors or {}).get(param))
        if rgb:
            tints[slot] = rgb
    hs = PIECE_SCALE["hair"]
    if hair_scale is not None and 0.05 <= hair_scale <= 1.0:
        hs = round(float(hair_scale), 4)
    return Outfit(cls=cls, costume=chosen, race=chosen_race, styles=styles,
                  blueprints=raw, weapon_family=fam, colors=tints, hair_scale=hs,
                  dropped=[i["slot"] for i in issues], issues=issues)


async def blueprint_path(basename: str, hint: str, branch: str | None = None,
                         ref: str = "") -> str | None:
    """Where a part's blueprint actually lives. The archive keys on full logical paths
    (``2025/equipment/…``), so a bare name resolves to nothing without this.

    ``ref`` is the reference the prefab itself wrote, relative to ``blueprints/``, and it
    is the answer whenever the archive holds that exact file - the game is telling us
    which copy it means. It matters because a basename does NOT identify a blueprint:
    Candy Barbarian's starter and its Demonic Inferno costume both list a part called
    `c_p_candybarbarian_torso`, one at the root of `blueprints/` and one under the
    costume's own folder. Guessing between them by name dressed the starter in Demonic
    Inferno, byte for byte.

    Without a usable ref - a mod's part, a raw blueprint a caller passed in - fall back
    to the unambiguous basename index, then to the archived copy closest to the prefab
    that asked for it (the creature renderer's rule)."""
    branch = branch or settings.trove_render_branch
    all_paths = await game_file_paths(branch)
    candidates = all_paths.get(f"{basename}.blueprint", [])
    if ref:
        exact = f"blueprints/{ref}.blueprint"
        if exact in candidates:
            return exact
    index = await blueprint_by_basename(branch)
    found = index.get(basename)
    if found:
        return found
    return nearest_path(candidates, hint)


async def _placements(outfit: Outfit, branch: str) -> list[tuple]:
    """``[(AP key, blueprint bytes, scale, tint|None)]`` for everything on the rig."""

    async def read(basename: str, hint: str, ref: str = "") -> bytes | None:
        path = await blueprint_path(basename, hint, branch, ref)
        return await get_blueprint_bytes(path, branch) if path else None

    out: list[tuple] = []
    for basename, ap_key in outfit.costume.parts.items():
        raw = await read(basename, outfit.costume.prefab,
                         outfit.costume.refs.get(basename, ""))
        if raw:
            out.append((ap_key, raw, 1.0, None))

    for slot in STYLE_SLOTS:
        opt = outfit.styles.get(slot)
        if not opt:
            continue
        raw = await read(opt.blueprint, opt.prefab, opt.ref)
        if not raw:
            continue
        # One equipped style, drawn once per socket the class declares for its family -
        # which is what makes the Candy Barbarian hold two identical swords. The socket
        # decides the scale: hat/face art is authored at double the body's resolution,
        # a weapon's is not (see assembly.scale_for).
        for socket in sockets_mod.sockets_for_slot(outfit.cls.sockets, opt.slot_id):
            out.append((socket["ap"], raw,
                        assembly.scale_for(socket["ap"], outfit.cls.skeleton), None))

    for slot, opt in outfit.styles.items():
        if slot not in RACE_SLOTS:
            continue                          # equipment styles are placed above
        ap = attach_point(slot, outfit.cls.skeleton)
        data = await read(opt.blueprint, opt.prefab) if ap else None
        if data:
            out.append((ap, data, outfit.piece_scale(slot), outfit.tint_for(slot)))

    for slot, ref in outfit.blueprints.items():
        if ref == NONE:
            continue
        # The caller named the file; the class's socket table still names the bone.
        # head/hair are plain attach points rather than equipment sockets.
        if slot in DIRECT_APS:
            ap = attach_point(slot, outfit.cls.skeleton)
            aps = [ap] if ap else []
        elif slot in SLOT_FAMILY:
            aps = [s["ap"] for s in sockets_mod.sockets_for_family(
                outfit.cls.sockets, SLOT_FAMILY[slot])]
        else:
            aps = [s["ap"] for s in sockets_mod.sockets_for_family(
                outfit.cls.sockets, outfit.weapon_family)]
        if not aps:
            continue
        # A caller who names a full path gets that exact file; a bare basename falls
        # through to the archive index, with the hint disambiguating a name Trove reuses.
        data = await read(ref.rsplit("/", 1)[-1], ref, ref if "/" in ref else "")
        if not data:
            continue
        tint = outfit.tint_for(slot)
        scale = (outfit.piece_scale(slot) if slot in RACE_SLOTS
                 else assembly.scale_for(aps[0], outfit.cls.skeleton))
        for ap in aps:
            out.append((ap, data, scale, tint))
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
