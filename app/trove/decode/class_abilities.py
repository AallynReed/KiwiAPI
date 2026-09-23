"""class_abilities.json - each class's abilities: cost, cooldown, damage, healing, effects.

Each class prefab names the ability prefabs it actually uses; an ability spawns a
chain (ability -> projectile -> explosion -> effect) and ``ability.describe`` reads
the numbers off that chain structurally: energy and cooldown from the action
component, DamageParameters (``base`` = flat damage, ``multiplier`` = share of the
damage stat), HealingParameters, and each effect's duration and stat changes.

The curated ``icon`` and ``type`` live in the repo copy of this file and are
carried onto each rebuild.
"""
from __future__ import annotations

import re

from app.trove.decode import store
from app.trove.decode.ability import Prefabs, describe, identity, refs, walk
from app.trove.decode.common import locale, stem
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import strings

TITLE = "Class abilities"
OUTPUT = "class_abilities.json"
PREFIXES = ("prefabs/class/", "prefabs/abilities/", "prefabs/sfx/", "languages/en/")
INDENT, FINAL_NEWLINE = 4, False

# Per-prefab lists that ``_assign_owners`` de-duplicates across a class's abilities.
_OWNED = ("stages", "healing", "effects")


def stage_name(rel: str) -> str:
    """A readable stage label from the prefab stem - the game ships no stage names."""
    return rel.rsplit("/", 1)[-1].replace("_", " ").title()


def _resolve(aloc: dict[str, str], key: str) -> tuple[str | None, str]:
    """A `$prefabs_abilities_…` key -> (name, description). Three shapes ship."""
    for suffix in ("_item_name", "_name", ""):
        name = aloc.get(key + suffix)
        if name:
            desc = "_item_description" if suffix == "_item_name" else "_description"
            return name, aloc.get(key + desc, "")
    return None, ""


def _label(aloc: dict[str, str], folder: str, rel: str, prefabs: Prefabs) -> tuple[str | None, str]:
    """Ability name + description.

    The identity component names its own locale keys, and those are NOT derivable
    from the filename - Vanguardian's `super_buff_melee` is `…_shockwave` ("Force
    Flash"). Other embedded `$prefabs_abilities…` keys come next, and the stem only
    as a last resort, which is all the prefabs that embed nothing have.
    """
    pf = prefabs.get(rel)
    ident = identity(pf)
    if ident.get("name_key") in aloc:
        return aloc[ident["name_key"]], aloc.get(ident.get("description_key", ""), "")
    for text in strings(pf.components if pf else []):
        if not text.startswith("$prefabs_abilities") or text.endswith("_description"):
            continue
        base = text
        for suffix in ("_item_name", "_name"):
            if base.endswith(suffix):
                base = base[: -len(suffix)]
                break
        name, desc = _resolve(aloc, base)
        if name:
            return name, desc
    return _resolve(aloc, f"$prefabs_abilities_{folder}_{rel.rsplit('/', 1)[-1]}")


def _assign_owners(abilities: list[dict]) -> None:
    """Give every damage/heal/effect prefab exactly one owning ability.

    Trove abilities share sub-prefabs - Shadow Hunter's passive and Sun Snare both
    reach the basic attack - so a plain walk lists the same numbers under several
    abilities and anything that sums them double-counts. The ability that reaches a
    prefab most directly owns it; ties go to the order the class prefab names them.
    """
    for key in _OWNED:
        best: dict[str, tuple[int, int]] = {}
        for index, ability in enumerate(abilities):
            for row in ability.get(key, []):
                rank = (row["_depth"], index)
                if row["prefab"] not in best or rank < best[row["prefab"]]:
                    best[row["prefab"]] = rank
        for index, ability in enumerate(abilities):
            kept = [r for r in ability.get(key, []) if best[r["prefab"]] == (r["_depth"], index)]
            for row in kept:
                row.pop("_depth", None)
            ability[key] = kept


def _icon_token(icon: str, folder: str) -> str:
    """The ability-identifying part of a curated icon name.

    `ico_gunslinger_chargeshot_01` -> `chargeshot`, which is what lets a curated
    entry find its prefab after the ability was renamed in game.
    """
    parts = [p for p in icon.split("_") if p]
    parts = [p for p in parts if p not in ("ico", "icon", folder) and not p.isdigit()]
    return "".join(parts)


def _overlay_curated(entry: dict, curated: list[dict], folder: str) -> list[dict]:
    """Carry the hand-written `icon` and `type` onto the decoded abilities.

    Names drift - Boomeranger's `Boomerang` is `Boomerang of the Wind` in game -
    so an exact name match is tried first, then the curated icon token against the
    prefab stem. The icon match only applies when exactly one unclaimed curated
    entry fits, so an ambiguous one is left blank rather than guessed. Curated
    abilities the decode never reached are kept as-is, without a `prefab`.
    """
    by_name = {a["name"]: a for a in curated}
    claimed = set()
    for ability in entry["abilities"]:
        hit = by_name.get(ability["name"])
        if hit:
            claimed.add(hit["name"])
            ability["icon"], ability["type"] = hit.get("icon", ""), hit.get("type", "")

    for ability in entry["abilities"]:
        if ability.get("type"):
            continue
        token = re.sub(r"[^a-z0-9]", "", ability["prefab"].rsplit("/", 1)[-1].lower())
        fits = [a for a in curated if a["name"] not in claimed
                and (tok := _icon_token(a.get("icon", ""), folder)) and tok in token]
        if len(fits) == 1:
            ability["icon"], ability["type"] = fits[0].get("icon", ""), fits[0].get("type", "")
            ability["matched_by"] = "icon"

    for a in curated:
        if a["name"] in claimed or any(x.get("icon") == a.get("icon") and x.get("type") == a.get("type")
                                       for x in entry["abilities"]):
            continue
        entry["abilities"].append({
            "name": a["name"], "description": "", "icon": a.get("icon", ""),
            "type": a.get("type", ""), "stages": a.get("stages", []), "active": False,
        })
    return entry["abilities"]


def _ability(prefabs: Prefabs, aloc: dict[str, str], rel: str, name: str, desc: str, stop: set[str],
             prefix: str, active: bool) -> dict:
    info = describe(prefabs, rel, stop - {rel}, prefix=prefix)
    out = {"name": name, "description": desc, "prefab": rel, "icon": "", "type": "", "active": active}
    for key in ("energy", "cooldown"):
        if key in info:
            out[key] = info[key]
    for key in ("stages", *_OWNED[1:], "vfx"):
        out[key] = info[key]
    for eff in out["effects"]:
        title = aloc.get(eff.pop("name_key", ""), "")
        text = aloc.get(eff.pop("description_key", ""), "")
        if title:
            eff["title"] = title
        if text:
            eff["description"] = text
    return out


def _merge(into: dict, extra: dict) -> None:
    """Several prefabs share a name (Shadow Hunter ships Radiant Arrow three times,
    normal/ultimate/base). They are one ability, so fold the extra prefabs' rows in
    rather than dropping them - they are boundaries, so no parent picks them up."""
    for key in (*_OWNED, "vfx"):
        for row in extra.get(key, []):
            if row not in into[key]:
                into[key].append(row)
    if not into["description"]:
        into["description"] = extra["description"]
    for key in ("energy", "cooldown"):
        if key not in into and key in extra:
            into[key] = extra[key]


def build(tree: GameTree) -> list[dict]:
    prefabs = Prefabs(tree)
    curated_by_class = {c["name"]: c.get("abilities", []) for c in store.baseline(OUTPUT)}
    display: dict[str, str] = {}
    for name in ("prefabs_class.binfab", "ui.binfab", "new.binfab"):
        display.update(locale(tree.read(f"languages/en/{name}")))

    out = []
    for class_path in tree.files("prefabs/class/", ".binfab"):
        folder = stem(class_path)
        if class_path.count("/") != 2 or folder.endswith("_ultimate"):
            continue
        cls = prefabs.get(f"class/{folder}")
        texts = strings(cls.root) if cls and cls.root is not None else []
        key = next((s for s in texts if s.startswith("$DisplayName")), None)
        if not key:
            continue

        # The ability folder is not always the class folder - Fae Trickster's class
        # prefab is `faetrickster` but its abilities live under `abilities/trickster`.
        match = next((re.match(r"abilities/([a-z0-9_]+)/", s) for s in texts
                      if re.match(r"abilities/([a-z0-9_]+)/", s)), None)
        afolder = match.group(1) if match else folder
        prefix = f"abilities/{afolder}/"
        aloc = locale(tree.read(f"languages/en/prefabs_abilities_{afolder}.binfab"))

        # Everything the CLASS prefab points at is a top-level action of that class,
        # named or not. Those are the walk's boundaries: without them one ability
        # absorbs the others' damage (Lunar Lancer's passive reaches the leap, the
        # spear throw and moon blessing, and reports all of it as its own).
        class_refs = [r for r in (refs(cls, prefix) if cls else []) if prefabs.get(r) is not None]

        # A prefab that carries its own locale name IS an ability, wherever it sits
        # in the chain. Some class refs are stance dispatchers with no name
        # (Vanguardian's `energy_blast` points at Plasma Blast and Eyebeam), and a
        # transformed kit hangs further down (Lunar Lancer's Eclipse Spear). So walk
        # everything the class reaches and promote whatever is named, class refs
        # first so the primary kit leads the list.
        reachable: list[str] = []
        for root in class_refs:
            for rel, _, _ in walk(prefabs, root, limit=400, prefix=prefix):
                if rel not in reachable:
                    reachable.append(rel)
        ordered = class_refs + [r for r in reachable if r not in set(class_refs)]
        entries = [(rel, *_label(aloc, afolder, rel, prefabs)) for rel in ordered]
        entries = [(rel, name, desc) for rel, name, desc in entries if name]
        boundaries = set(class_refs) | {rel for rel, _, _ in entries}

        abilities: list[dict] = []
        by_name: dict[str, dict] = {}
        for rel, name, desc in entries:
            ability = _ability(prefabs, aloc, rel, name, desc, boundaries, prefix, active=True)
            if name in by_name:
                _merge(by_name[name], ability)
                continue
            # Reachable from the live class prefab. The tree also keeps pre-revamp
            # copies of most abilities that still load but are no longer wired up.
            by_name[name] = ability
            abilities.append(ability)

        # Named abilities that exist but the class prefab cannot reach: Shadow
        # Hunter's Radiant Arrow, Boomeranger's Bawk Bomb. Reachability - not the
        # folder - is the test. They still load, so list them, flagged inactive.
        for path in tree.files(f"prefabs/{prefix}", ".binfab"):
            rel = path[len("prefabs/"):-len(".binfab")]
            if rel in reachable:
                continue
            name, desc = _label(aloc, afolder, rel, prefabs)
            if not name or name in by_name:
                continue
            by_name[name] = _ability(prefabs, aloc, rel, name, desc, boundaries, prefix, active=False)
            abilities.append(by_name[name])

        entry = {
            "name": display.get(key, key.replace("$DisplayName_", "")),
            "game_folder": folder,
            "abilities": abilities,
        }
        _assign_owners(entry["abilities"])
        entry["abilities"] = _overlay_curated(entry, curated_by_class.get(entry["name"], []), folder)
        out.append(entry)
    return out


def count(data: list) -> int:
    return len(data)
