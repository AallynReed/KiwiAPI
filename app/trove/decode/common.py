"""Helpers the ability decoders share."""
from __future__ import annotations

from app.trove.codexes.binfab import harvest_strings
from app.trove.codexes.bonuses import extract_stat_bonuses
from app.trove.codexes.localize import resolve_stat_name
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


def stat_rows(data: bytes, prefab: str | None = None) -> list[dict]:
    """Stat records in the units the site's data files use, carrying their op.

    `amount` keeps the raw wire figure because `value` is normalised for display
    and a `Multiply` loses information doing it: incoming-damage 0.85 reads as a
    15% reduction, but shot-speed 1.5 is a 50% increase - same op, opposite sense.
    """
    rows = []
    for bonus in extract_stat_bonuses(data):
        sid, op, raw = bonus["stat_id"], bonus["operation"], bonus["amount"]
        value = abs(bonus["value"])
        if sid == 0x0F and op == "MultiplySum":      # AttackSpeed stores percent on buffs
            value = raw * 100 if abs(raw) < 1 else raw
        elif sid == 0x0C and op == "Add":            # OutgoingDamageMod stores a fraction
            value = raw * 100
        row = {"name": resolve_stat_name({}, bonus["stat"]), "stat": bonus["stat"],
               "op": op, "value": round(value, 4), "amount": round(raw, 4)}
        if prefab is not None:
            row["prefab"] = prefab
        if row not in rows:
            rows.append(row)
    return rows


def stem(path: str) -> str:
    """`prefabs/item/gem/large/blue_x_t3.binfab` -> `blue_x_t3`."""
    return path.rsplit("/", 1)[-1].rsplit(".", 1)[0]
