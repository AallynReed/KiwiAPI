"""Typed relationship edges between codex entries (the ``codex_link`` table).

Everything the decoders already know about how two things relate, projected into one
shape so it can be asked in both directions. The reverse lookups are the point:
"what recipes produce this item" and "what is this item used to craft" are the same
row set read from opposite ends, and neither needs its own table or its own decode.

Builders here are pure - they take an already-parsed entry (or an already-decoded
table) and return row tuples. Nothing reads files; the indexer supplies the data.
"""

from __future__ import annotations

from app.trove.codexes.types import PREFABS_ROOT

# Relations. Kept as constants so a typo can't silently create an orphan edge kind.
CRAFTS = "crafts"                 # recipe -> the item it produces
INGREDIENT = "ingredient"         # recipe -> an item it consumes
CRAFTABLE_AT = "craftable_at"     # recipe -> a bench / profession
UNLOCKS = "unlocks"               # anything -> what it grants
UPGRADE_COST = "upgrade_cost"     # progression node -> an item it costs
MEMBER_OF = "member_of"           # collectible -> its collection catalogue

ALL_RELATIONS = (CRAFTS, INGREDIENT, CRAFTABLE_AT, UNLOCKS, UPGRADE_COST, MEMBER_OF)


def entry_path(rel: str) -> str:
    """A prefab reference in any of its written forms -> the `codex_entry.path` form.

    Decoders emit references relative to `prefabs/` and without the extension
    (`item/crafting/crystal`); entries are keyed on the full logical path
    (`prefabs/item/crafting/crystal.binfab`). Normalizing here is what lets a link
    join to an entry instead of merely resembling one."""
    p = str(rel or "").replace("\\", "/").strip().lower()
    if not p:
        return ""
    p = p.removesuffix(".binfab")
    if not p.startswith(PREFABS_ROOT):
        p = PREFABS_ROOT + p.lstrip("/")
    return p + ".binfab"


def _row(branch: str, src: str, relation: str, dst: str, ord_: int,
         qty: float | None = None, data: dict | None = None) -> tuple:
    return (branch, src, relation, dst, ord_, qty, data or {})


def recipe_links(entry: dict, branch: str) -> list[tuple]:
    """`crafts` / `ingredient` / `craftable_at` edges for one recipe entry.

    De-duped on (relation, target): a recipe can legitimately list the same bench
    twice, and the ordinal keeps source order for the first occurrence only, so the
    primary key can't collide."""
    data = (entry.get("data") or {}).get("recipe") or {}
    src = entry["path"]
    out: list[tuple] = []
    seen: set[tuple[str, str]] = set()

    def add(relation: str, target: str, ord_: int, qty=None, extra=None) -> None:
        dst = entry_path(target)
        if not dst or (relation, dst) in seen:
            return
        seen.add((relation, dst))
        out.append(_row(branch, src, relation, dst, ord_, qty, extra))

    output = data.get("output") or {}
    if output.get("path"):
        add(CRAFTS, output["path"], 0, _amount(output.get("amount")))

    for ord_, ingredient in enumerate(data.get("ingredients") or []):
        if ingredient.get("path"):
            add(INGREDIENT, ingredient["path"], ord_, _amount(ingredient.get("amount")))

    for ord_, provider in enumerate(data.get("providers") or []):
        path = provider.get("provider_path") or provider.get("provider")
        if path:
            add(CRAFTABLE_AT, path, ord_, None, {
                "lane": provider.get("lane") or "",
                "category": provider.get("category") or "",
                "category_order": provider.get("category_order"),
            })
    return out


def upgrade_links(parsed: dict, branch: str, source_path: str) -> list[tuple]:
    """`upgrade_cost` edges: one per (node, item). The node is addressed by its own
    key rather than the tree file, so a cost belongs to the node that pays it."""
    out: list[tuple] = []
    for node in parsed.get("nodes") or []:
        src = f"{parsed['system_key']}:{node['node_key']}"
        seen: set[str] = set()
        for ord_, cost in enumerate(node.get("costs") or []):
            dst = entry_path(cost.get("item", ""))
            if not dst or dst in seen:
                continue
            seen.add(dst)
            out.append(_row(branch, src, UPGRADE_COST, dst, ord_,
                            _amount(cost.get("quantity")),
                            {"system_kind": parsed.get("system_kind") or "",
                             "system_key": parsed["system_key"],
                             "node_key": node["node_key"],
                             "source_path": source_path}))
    return out


def unlock_links(pairs: list[tuple[str, str]], branch: str) -> list[tuple]:
    """`unlocks` edges from the decoded unlock table."""
    out: list[tuple] = []
    seen: set[tuple[str, str]] = set()
    for ord_, (source, target) in enumerate(pairs):
        src, dst = entry_path(source), entry_path(target)
        if not src or not dst or (src, dst) in seen:
            continue
        seen.add((src, dst))
        out.append(_row(branch, src, UNLOCKS, dst, ord_))
    return out


def membership_links(members: dict[str, str], catalogue_path: str, branch: str) -> list[tuple]:
    """`member_of` edges: collectible -> the catalogue that lists it, carrying the
    group label the catalogue filed it under."""
    dst = entry_path(catalogue_path)
    out: list[tuple] = []
    for ord_, (member, group) in enumerate(sorted(members.items())):
        src = entry_path(member)
        if src and dst:
            out.append(_row(branch, src, MEMBER_OF, dst, ord_, None, {"group": group}))
    return out


def _amount(value) -> float | None:
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if number == number and abs(number) != float("inf") else None
