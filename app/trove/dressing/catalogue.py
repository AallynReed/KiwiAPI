"""The dressing room's option lists, built from the live game archive.

Four questions, four sources, all the game's own data:

  which classes exist      ``prefabs/class/*.binfab`` - skeleton + socket table
  which costumes fit one   the rig map (``rig_binding``), whose ``prefabs/skins/`` rows
                           already carry each costume's parts and attach points
  which styles exist       ``prefabs/equipment/**.binfab`` - slot number + blueprint
  what they are called     the archived ``languages/en/`` string tables

A costume belongs to a class because it binds that class's SKELETON, not because its
filename starts with the class name. A style fits a socket because its slot number
matches the socket's, not because the filename says "sword". Neither is inferred.

The whole catalogue is a few thousand small reads, so it is built once and cached
in-process against the codex index signature - the same token ``rig_index`` keys its
own cache on, which moves on every game sync.
"""

from __future__ import annotations

import asyncio
import logging
import re
from dataclasses import dataclass, field

from app.core.config import settings
from app.trove.codexes import binfab, pg_store
from app.trove.dressing import customhead
from app.trove.dressing import sockets as sockets_mod
from app.trove.mods_hub import rig_index

logger = logging.getLogger("kiwi.dressing")

CLASS_ROOT = "prefabs/class/"
SKIN_ROOT = "prefabs/skins/"
EQUIP_ROOT = "prefabs/equipment/"
CUSTOM_HEADS = "prefabs/custom_heads_service.binfab"

# Slot ids the page addresses. "weapon" is one slot even for a class that draws the
# selected style twice - the game gives one equipped style, the sockets say how often
# it is drawn (see sockets.py).
SLOTS = ("costume", "hat", "face", "weapon", "head", "hair", "eyes")
# The character-creation slots: not styles you equip, but the race you picked and the
# head, hair and eyes that come with it (see customhead.py).
RACE_SLOTS = ("head", "hair", "eyes")

_NAME_KEY_RE = re.compile(r"^\$[A-Za-z0-9_]+_(?:skinset_)?name$")
# A full helmet ENCLOSES the head: hair, face style and eyes all go with it. A hat sits
# on top and leaves them showing.
#
# This is a NAME rule, chosen deliberately over the alternatives and worth knowing the
# limits of: the game does not record the difference anywhere in the prefab. All 76
# hat-slot styles were compared field by field and not one identity flag separates the
# helm-named from the hat-named group - every field is either identical across both or
# overlaps. So this reads the model's own filename, which is a convention rather than a
# statement, and will be wrong for any helmet not named like one. `covers_head` is on the
# option in the API so a wrong call is visible and correctable per style.
_HELMET_RE = re.compile(r"_helm(?:_|$|[^a-z])")
_WORD_RE = re.compile(r"[_/]+")


@dataclass(frozen=True)
class Option:
    """One selectable appearance."""

    key: str                              # stable id: the prefab stem
    name: str
    slot: str                             # one of SLOTS
    family: str = ""                      # weapon family ("Melee"/"Bow"/…), else ""
    slot_id: int = 0                      # the game's equipment slot number (styles)
    blueprint: str = ""                   # style model basename (no extension)
    prefab: str = ""                      # source prefab path
    skeleton: str = ""                    # costume only
    covers_head: bool = False             # a full helmet rather than a hat
    credit: str = ""                      # community author, from a `[name]` suffix
    parts: dict[str, str] = field(default_factory=dict)   # costume: basename -> AP key


@dataclass(frozen=True)
class Race:
    """A character-creation race and the pieces it can wear."""

    key: str
    name: str
    pieces: dict[str, list[str]] = field(default_factory=dict)   # kind -> blueprints

    def first(self, kind: str) -> str:
        """The race's default piece of this kind - what the game shows before you pick."""
        got = self.pieces.get(kind) or []
        return got[0] if got else ""


@dataclass(frozen=True)
class DressClass:
    key: str                              # class prefab stem ("knight")
    name: str
    skeleton: str
    sockets: list[dict]
    weapons: list[str]


@dataclass(frozen=True)
class Catalogue:
    classes: dict[str, DressClass] = field(default_factory=dict)
    costumes: dict[str, list[Option]] = field(default_factory=dict)   # class key -> options
    styles: dict[str, list[Option]] = field(default_factory=dict)     # slot -> options
    options: dict[str, Option] = field(default_factory=dict)          # "slot:key" -> option
    races: dict[str, Race] = field(default_factory=dict)              # race key -> race

    def __bool__(self) -> bool:
        return bool(self.classes)

    def get(self, slot: str, key: str) -> Option | None:
        return self.options.get(f"{slot}:{key}")


_cache: dict[str, tuple] = {}
_lock = asyncio.Lock()


_CREDIT_RE = re.compile(r"\[([^\]]+)\]")
# Prefixes the piece blueprints all share; they say nothing to a reader.
_PIECE_PREFIX = re.compile(r"^c_p_(?:head|hair|eyes)_?")


def _humanize(stem: str) -> str:
    """Readable label from a prefab stem, for an option the string tables don't name.
    Only ever a NAME - nothing about the model depends on it.

    Most hair styles are community submissions and carry their author in a ``[name]``
    suffix; the game names barely a third of them, so the rest fall here. Drop the shared
    ``c_p_hair_`` prefix and the credit, both of which are noise in a picker."""
    body = _CREDIT_RE.sub("", stem)
    body = _PIECE_PREFIX.sub("", body)
    return " ".join(w.capitalize() for w in _WORD_RE.split(body) if w) or stem


def _name_key(data: bytes) -> str | None:
    """The prefab's own display-name locale key (``$prefabs_skins_..._skinset_name``)."""
    for _off, _f, s in binfab.harvest_strings(data):
        if _NAME_KEY_RE.match(s):
            return s
    return None


async def _archive_files(branch: str, prefix: str) -> list[tuple[str, str]]:
    """``[(path, content sha)]`` for every archived file under ``prefix``."""
    from app.trove.updates.models import UpdateState

    coll = UpdateState.get_pymongo_collection()
    rows = await coll.find(
        {"branch": branch, "path": {"$gte": prefix, "$lt": prefix + "￿"}},
        {"path": 1, "content_sha256": 1, "_id": 0},
    ).to_list(length=None)
    return [(r["path"], r["content_sha256"]) for r in rows
            if r["path"].endswith(".binfab") and r.get("content_sha256")]


def _read_many(store, shas: list[str]) -> list[bytes | None]:
    return [store.get(sha) for sha in shas]


async def _load(branch: str, prefix: str) -> list[tuple[str, bytes]]:
    """Every ``.binfab`` under ``prefix``, as ``[(path, bytes)]``. The reads are plain
    file reads off the content store, so they go to a thread in one batch."""
    from app.trove.updates.cas import ContentStore

    files = await _archive_files(branch, prefix)
    if not files:
        return []
    store = ContentStore(settings.trove_update_store_dir)
    blobs = await asyncio.to_thread(_read_many, store, [sha for _p, sha in files])
    return [(path, blob) for (path, _sha), blob in zip(files, blobs, strict=False) if blob]


async def _load_one(branch: str, path: str) -> bytes | None:
    """One archived file's bytes by logical path."""
    from app.trove.updates.cas import ContentStore
    from app.trove.updates.models import UpdateState

    doc = await UpdateState.find_one({"branch": branch, "path": path})
    if doc is None:
        return None
    store = ContentStore(settings.trove_update_store_dir)
    return await asyncio.to_thread(store.get, doc.content_sha256)


def _stem(path: str) -> str:
    return path.replace("\\", "/").rsplit("/", 1)[-1].removesuffix(".binfab").lower()


def _build_classes(files: list[tuple[str, bytes]], loc: dict[str, str]) -> dict[str, DressClass]:
    out: dict[str, DressClass] = {}
    for path, data in files:
        parsed = sockets_mod.parse_class(data)
        if not parsed:
            continue                       # not a wearable class form - no sockets declared
        stem = _stem(path)
        name = binfab.clean_localized_text(loc.get(parsed["name_key"], "")) or _humanize(stem)
        out[stem] = DressClass(
            key=stem, name=name, skeleton=parsed["skeleton"], sockets=parsed["sockets"],
            weapons=sockets_mod.weapon_families(parsed["sockets"]),
        )
    return out


def _build_costumes(
    rig_map: rig_index.RigMap, names: dict[str, str], by_skeleton: dict[str, str],
) -> dict[str, list[Option]]:
    """Group the rig map's ``skins/`` rows into one option per costume prefab, filed
    under the class whose skeleton it binds."""
    out: dict[str, list[Option]] = {}
    for prefab, (skeleton, parts) in rig_map.creatures.items():
        if not prefab.startswith(SKIN_ROOT):
            continue
        class_key = by_skeleton.get(skeleton)
        if not class_key:
            continue                       # a costume for a form we don't offer
        stem = _stem(prefab)
        out.setdefault(class_key, []).append(Option(
            key=stem, name=names.get(stem) or _humanize(stem), slot="costume",
            prefab=prefab, skeleton=skeleton, parts=dict(parts),
        ))
    for options in out.values():
        options.sort(key=lambda o: o.name.lower())
    return out


def _build_styles(files: list[tuple[str, bytes]], loc: dict[str, str]) -> dict[str, list[Option]]:
    """One option per equipment-appearance prefab that names a blueprint AND carries a
    slot number. Anything missing either is left out - an appearance we can't file or
    can't draw isn't offered."""
    out: dict[str, list[Option]] = {slot: [] for slot in SLOTS if slot != "costume"}
    for path, data in files:
        ident = binfab.decode_identity(data) or {}
        slot_id = sockets_mod.style_slot(ident.get("flags"))
        if slot_id is None or slot_id == 40:      # banners hang off a pole, not a socket
            continue
        blueprint = _blueprint_of(data)
        if not blueprint:
            continue
        if slot_id == sockets_mod.HAT_SLOT:
            slot = "hat"
        elif slot_id == sockets_mod.FACE_SLOT:
            slot = "face"
        else:
            slot = "weapon"
        stem = _stem(path)
        name = binfab.clean_localized_text(loc.get(ident.get("name_key") or "", ""))
        out[slot].append(Option(
            key=stem, name=name or _humanize(stem), slot=slot,
            family=sockets_mod.SLOTS[slot_id], slot_id=slot_id,
            blueprint=blueprint, prefab=path,
            covers_head=slot == "hat" and bool(_HELMET_RE.search(blueprint)),
        ))
    for options in out.values():
        options.sort(key=lambda o: o.name.lower())
    return out


def _blueprint_of(data: bytes) -> str:
    """The style's model blueprint basename (no folder, no extension)."""
    for _off, _f, s in binfab.harvest_strings(data):
        low = s.lower()
        if low.endswith(".blueprint"):
            return low.replace("\\", "/").rsplit("/", 1)[-1][: -len(".blueprint")]
    return ""


async def _locale_map(branch: str) -> dict[str, str]:
    from app.trove.codexes.indexer import load_locale_map
    from app.trove.updates.cas import ContentStore

    return await load_locale_map(branch, ContentStore(settings.trove_update_store_dir))


def _build_races(data: bytes, loc: dict[str, str]) -> tuple[dict[str, Race], dict[str, list[Option]]]:
    """The races, plus one option list per character-creation slot. A head or eyes option
    is per race (its key is prefixed so two races' pieces can't collide); hair is shared,
    so it is listed once."""
    races: dict[str, Race] = {}
    slots: dict[str, list[Option]] = {k: [] for k in RACE_SLOTS}
    seen: set[tuple[str, str]] = set()
    for row in customhead.parse(data):
        name = binfab.clean_localized_text(loc.get(row["name_key"], "")) or _humanize(row["race"])
        races[row["key"]] = Race(key=row["key"], name=name, pieces=row["pieces"])
        for kind, blueprints in row["pieces"].items():
            for bp in blueprints:
                key = bp.lower()
                if (kind, key) in seen:
                    continue
                seen.add((kind, key))
                label = binfab.clean_localized_text(
                    loc.get(f"$CustomHead_Piece_{bp}", "")) or _humanize(key)
                credit = _CREDIT_RE.search(bp)
                slots[kind].append(Option(key=key, name=label, slot=kind, blueprint=key,
                                          prefab=CUSTOM_HEADS,
                                          credit=credit.group(1) if credit else ""))
    for options in slots.values():
        options.sort(key=lambda o: o.name.lower())
    return races, slots


async def _build(branch: str) -> Catalogue:
    loc = await _locale_map(branch)
    classes = _build_classes(await _load(branch, CLASS_ROOT), loc)
    if not classes:
        return Catalogue()

    # skeleton -> class. A skeleton names exactly one class form in Trove, and a costume
    # that binds it is a costume for that class - which is why this can be a plain map.
    by_skeleton = {c.skeleton: c.key for c in classes.values()}

    skins = await _load(branch, SKIN_ROOT)
    names: dict[str, str] = {}
    for path, data in skins:
        key = _name_key(data)
        text = binfab.clean_localized_text(loc.get(key or "", "")) if key else ""
        if text:
            names[_stem(path)] = text

    rig_map = await rig_index.rig_map(branch)
    costumes = _build_costumes(rig_map, names, by_skeleton)
    styles = _build_styles(await _load(branch, EQUIP_ROOT), loc)

    heads_raw = await _load_one(branch, CUSTOM_HEADS)
    races, race_slots = _build_races(heads_raw, loc) if heads_raw else ({}, {})
    styles.update(race_slots)

    # A class with no costume at all is a form the game never dresses (an ultimate
    # transformation), so it isn't offered - the data decides, not the name.
    classes = {k: c for k, c in classes.items() if costumes.get(k)}
    costumes = {k: v for k, v in costumes.items() if k in classes}

    options: dict[str, Option] = {}
    for group in list(costumes.values()) + list(styles.values()):
        for opt in group:
            options[f"{opt.slot}:{opt.key}"] = opt

    logger.info("dressing[%s]: %d classes, %d costumes, %d races, %s styles", branch,
                len(classes), sum(len(v) for v in costumes.values()), len(races),
                {k: len(v) for k, v in styles.items()})
    return Catalogue(classes=classes, costumes=costumes, styles=styles, options=options,
                     races=races)


async def get(branch: str | None = None) -> Catalogue:
    """The branch's catalogue, rebuilt only when the game index signature moves."""
    branch = branch or settings.trove_render_branch
    if not settings.postgres_enabled:
        return Catalogue()                 # no rig map -> no costumes to dress
    sig = await pg_store.meta_signature(branch)
    cached = _cache.get(branch)
    if cached and cached[0] == sig:
        return cached[1]
    async with _lock:
        cached = _cache.get(branch)         # re-check after awaiting the lock
        if cached and cached[0] == sig:
            return cached[1]
        built = await _build(branch)
        _cache[branch] = (sig, built)
        return built
