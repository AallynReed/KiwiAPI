"""Helpers the ability decoders share."""
from __future__ import annotations

import re
from collections.abc import Iterable
from typing import Any

from app.trove.codexes.binfab import harvest_strings
from app.trove.codexes.bonuses import STAT_KEYS, _normalize, bonus_key
from app.trove.codexes.localize import resolve_stat_name
from app.trove.decode import fields as F
from app.trove.decode.ability import is_modifier
from app.trove.decode.wire import Obj, WireError, parse


def locale(data: bytes | None) -> dict[str, str]:
    """`$key -> text` from a `languages/<lang>/*.binfab` table.

    A table is one object whose field 0 lists `{0: key, 1: text}` records. Read
    structurally: the old byte-adjacency pairing missed about one entry in ten
    (Shadow Hunter's "Sacred Arrow" among them), so it is only a fallback now.
    """
    if not data:
        return {}
    try:
        pf = parse(data)
    except WireError:
        return _locale_by_adjacency(data)
    out: dict[str, str] = {}
    for row in (pf.root.leaf.get(0) if pf.root is not None else None) or []:
        leaf = row.leaf if isinstance(row, Obj) else {}
        if isinstance(leaf.get(0), str) and isinstance(leaf.get(1), str):
            out.setdefault(leaf[0], leaf[1])
    return out or _locale_by_adjacency(data)


def _locale_by_adjacency(data: bytes) -> dict[str, str]:
    raw = harvest_strings(data)
    out = {}
    for i, (off, field, text) in enumerate(raw):
        if field != 0 or not text.startswith("$") or i + 1 >= len(raw):
            continue
        noff, nfield, ntext = raw[i + 1]
        if nfield == 1 and 0 <= noff - (off + len(text)) <= 4:
            out[text] = ntext
    return out


def stat_rows(values: Iterable[Any]) -> list[dict]:
    """The stat modifier records among parsed ``values``, in the units the site's
    data files use, carrying their op.

    `amount` keeps the raw wire figure because `value` is normalised for display
    and a `Multiply` loses information doing it: incoming-damage 0.85 reads as a
    15% reduction, but shot-speed 1.5 is a 50% increase - same op, opposite sense.
    """
    rows = []
    for v in values:
        if not is_modifier(v):
            continue
        sid, op = v.get(F.MOD_STAT, 0), v.get(F.MOD_OP, 0)
        if sid not in STAT_KEYS or not 0 <= op < len(F.MOD_OPS):
            continue
        key, raw = STAT_KEYS[sid], float(v[F.MOD_VALUE])
        value = abs(_normalize(key, op * 2, raw)[0])
        if sid == 0x0F and op == 0:                  # AttackSpeed stores percent on buffs
            value = raw * 100 if abs(raw) < 1 else raw
        elif sid == 0x0C and op == 1:                # OutgoingDamageMod stores a fraction
            value = raw * 100
        stat = bonus_key(key) if op == 0 else key
        row = {"name": resolve_stat_name({}, stat), "stat": stat, "op": F.MOD_OPS[op],
               "value": round(value, 4), "amount": round(raw, 4)}
        if row not in rows:
            rows.append(row)
    return rows


# Collection groups the game uses while building content; not player facing.
WORKING_GROUPS = re.compile(r"^(InProgress|ReadyForGame|READY_FOR_GAME|NEEDS_AUDIO|Hidden)")


def collection_members(pf, names: dict[str, str]) -> dict[str, tuple[str, str]]:
    """`collections/collection_<x>` -> `{member ref: (group id, group name)}`.

    A collection file lists groups {0 id, 1 `$CollectionName_*` key, 3 member rows
    whose field 0 is the collectible}; the group name is where the collection screen
    files it. Working groups keep their id but get no name. First listing wins.
    """
    out: dict[str, tuple[str, str]] = {}
    root = pf.root if pf is not None else None
    for group in (root.leaf.get(0) if root is not None else None) or []:
        g = group.leaf if isinstance(group, Obj) else {}
        gid = g.get(0) if isinstance(g.get(0), str) else ""
        label = "" if WORKING_GROUPS.match(gid) else names.get(g.get(1) or "", "") or gid
        for row in g.get(3) or []:
            ref = (row.leaf if isinstance(row, Obj) else {}).get(0)
            if isinstance(ref, str):
                out.setdefault(ref.removesuffix(".binfab"), (gid, label))
    return out


def stem(path: str) -> str:
    """`prefabs/item/gem/large/blue_x_t3.binfab` -> `blue_x_t3`."""
    return path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
