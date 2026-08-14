"""Progression trees: `prefabs/upgrade/trees/<key>.binfab`.

Three systems share one file shape. They are told apart by how their nodes are
NAMED, not by a hardcoded key list, so a new module, companion rarity or class
indexes itself:

- **geode modules** - nodes `<key>_NN` (Barrier Generator, N-Charge, Omni-Tool, …)
- **geode companions** - nodes `<key>_level_NN`. The per-level *bonuses* are decoded
  by `geode.parse_upgrade_tree`; this module decodes the per-level *costs*.
- **class prestige (Paragon)** - nodes `<NN>_<class>_root_NN` / `_branch_NN`, paid in
  `item/currency/prestige/<class>`.

A node is found structurally: an identifier string that is followed by at least one
`item/crafting|currency/…` reference carrying an adjacent quantity tag. That needs no
anchor on the file's own name - which matters because the prestige trees are stored
as `prestige_bard.binfab` but name their nodes `01_bard_root_01`, so anything keyed
on the filename finds nothing at all.

Each cost is an item reference immediately followed by tag `0x10` and a ZigZag varint
quantity. The adjacency requirement is also what keeps an over-long regex match
harmless: if the match ran past the real string, the next byte is not `0x10` and the
row is dropped rather than counted.

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import read_varint, unzig

TREES_ROOT = "prefabs/upgrade/trees/"
UPGRADES_ROOT = "prefabs/upgrade/upgrades/"

SYSTEM_MODULE = "geode_module"
SYSTEM_COMPANION = "geode_companion"
SYSTEM_PRESTIGE = "class_prestige"

# `/` is allowed inside the item path: prestige costs are `item/currency/prestige/bard`.
_COST_RE = re.compile(rb"item/(?:crafting|currency)/[A-Za-z0-9_\-/]+")
# Candidate node keys: a lowercase/digit identifier. `_upgrade_tree` headers and item
# paths are excluded below rather than in the pattern.
_TOKEN_RE = re.compile(rb"[a-z0-9][a-z0-9_]{2,63}")
_RANK_RE = re.compile(r"(\d{2})$")


def _IDENT_BYTE(byte: int) -> bool:  # noqa: N802 - reads as a predicate at the call site
    return byte == 0x5F or 0x30 <= byte <= 0x39 or 0x41 <= byte <= 0x5A or 0x61 <= byte <= 0x7A


def system_key_from_path(path: str) -> str:
    """`prefabs/upgrade/trees/barrier.binfab` -> `barrier`."""
    return path.replace("\\", "/").rsplit("/", 1)[-1].removesuffix(".binfab").lower()


def _system_kind(node_keys: list[str]) -> str:
    if any("_level_" in k for k in node_keys):
        return SYSTEM_COMPANION
    if any("_root_" in k or "_branch_" in k for k in node_keys):
        return SYSTEM_PRESTIGE
    return SYSTEM_MODULE


def _costs(data: bytes, start: int, end: int) -> list[dict]:
    """`[{item, quantity}, …]` for one byte span, in source order."""
    out: list[dict] = []
    for match in _COST_RE.finditer(data, start, end):
        tag = match.end()
        if tag >= end or data[tag] != 0x10:      # no adjacent quantity => not a cost row
            continue
        raw, _ = read_varint(data, tag + 1)
        if raw is None:
            continue
        quantity = unzig(raw)
        if quantity <= 0:
            continue
        out.append({"item": match.group(0).decode("ascii"), "quantity": quantity})
    return out


def _node_spans(data: bytes) -> list[tuple[str, int, int]]:
    """`[(node_key, start, end), …]` - every cost-bearing node definition, in file order.

    Candidate tokens are scanned in order; a token opens a node when a cost row appears
    before the next candidate. The first definition of a key wins, so the prerequisite
    mentions of an already-defined node don't reopen it."""
    cost_starts = [m.start() for m in _COST_RE.finditer(data)]
    if not cost_starts:
        return []
    cost_set = cost_starts

    candidates: list[tuple[int, str]] = []
    for match in _TOKEN_RE.finditer(data):
        start = match.start()
        # A token preceded by `/` is a path segment (`…/crystal`), and one preceded by an
        # identifier byte is the tail of a longer token - neither is a node key. Without
        # this every material basename inside a cost path is read as its own node, which
        # both invents nodes and steals the real node's cost rows.
        if start > 0 and (data[start - 1] == 0x2F or _IDENT_BYTE(data[start - 1])):
            continue
        token = match.group(0).decode("ascii")
        if token.endswith("_upgrade_tree") or token.startswith("item"):
            continue
        candidates.append((start, token))
    if not candidates:
        return []

    opened: dict[str, int] = {}
    for index, (offset, token) in enumerate(candidates):
        if token in opened:
            continue
        nxt = candidates[index + 1][0] if index + 1 < len(candidates) else len(data)
        # A cost reference starting inside (offset, nxt) makes this token a definition.
        if any(offset < c < nxt for c in cost_set):
            opened[token] = offset

    ordered = sorted(opened.items(), key=lambda kv: kv[1])
    spans: list[tuple[str, int, int]] = []
    for index, (token, offset) in enumerate(ordered):
        end = ordered[index + 1][1] if index + 1 < len(ordered) else len(data)
        spans.append((token, offset, end))
    return spans


def parse_upgrade_costs(data: bytes, path: str) -> dict:
    """`{system_kind, system_key, nodes: [{node_key, rank, costs, requires, offset}, …]}`.

    `nodes` is empty (and `system_kind` blank) when the file has no cost-bearing node -
    then it is a bonus-only tree and `geode.parse_upgrade_tree` is the reader for it.

    `requires` lists other node keys named inside a node's own span. It is EVIDENCE of a
    prerequisite edge, not a complete graph: the file states dependencies only where it
    needs to, so an empty list means "none stated here", never "no prerequisites".
    """
    spans = _node_spans(data)
    if not spans:
        return {"system_kind": "", "system_key": system_key_from_path(path), "nodes": []}

    keys = [key for key, _s, _e in spans]
    key_set = set(keys)
    nodes: list[dict] = []
    for node_key, start, end in spans:
        costs = _costs(data, start, end)
        if not costs:
            continue
        rank_match = _RANK_RE.search(node_key)
        requires = sorted({
            other for other in key_set
            if other != node_key and other.encode() in data[start:end]
        })
        nodes.append({
            "node_key": node_key,
            "rank": int(rank_match.group(1)) if rank_match else None,
            "costs": costs,
            "requires": requires,
            "offset": start,
        })
    return {
        "system_kind": _system_kind(keys) if nodes else "",
        "system_key": system_key_from_path(path),
        "nodes": nodes,
    }


# --- per-node effects (prefabs/upgrade/upgrades/<key>.binfab) ----------------
#
# The sibling of the tree file: `trees/` states a system's structure and its material
# costs, `upgrades/` states what each node DOES. Both are needed to describe a node.
#
# The node keys and ability refs in this file are NOT wire-type-8 fields, so neither
# `harvest_strings` nor `_real_fields` returns them - only the section markers
# (`stats` / `effects` / `upgradablevalues`) and the `$…_name` keys come back that way.
# So the chunking is done on raw byte offsets, the same approach
# `geode.parse_upgrade_tree` already uses for the companion trees.

_ABILITY_REF_RE = re.compile(rb"abilities/[A-Za-z0-9_/.\-]+")
_NAME_KEY_RE = re.compile(rb"\$[A-Za-z0-9_]+_name")


def parse_upgrade_effects(data: bytes, system_key: str) -> dict[str, dict]:
    """`{node_key: {"name_key": str, "abilities": [ref, …]}}` for one upgrades file.

    Node keys are matched against the system key so a material or ability name that
    happens to end in two digits can't open a phantom node. A node with neither a name
    key nor an ability ref is omitted rather than stored empty.
    """
    key = re.escape(system_key.encode())
    pattern = re.compile(key + rb"(?:_level)?_\d{2}")
    hits = [(m.start(), m.group(0).decode("ascii")) for m in pattern.finditer(data)]
    if not hits:
        return {}

    # First occurrence of each key opens its chunk; the chunk ends where the next
    # DISTINCT key starts, so a key repeated as a prerequisite doesn't split it.
    starts: dict[str, int] = {}
    for offset, node_key in hits:
        starts.setdefault(node_key, offset)
    ordered = sorted(starts.items(), key=lambda kv: kv[1])

    out: dict[str, dict] = {}
    for index, (node_key, start) in enumerate(ordered):
        end = ordered[index + 1][1] if index + 1 < len(ordered) else len(data)
        chunk = data[start:end]
        abilities = []
        seen: set[str] = set()
        for match in _ABILITY_REF_RE.finditer(chunk):
            ref = match.group(0).decode("ascii").rstrip("./")
            if ref not in seen:
                seen.add(ref)
                abilities.append(ref)
        name_match = _NAME_KEY_RE.search(chunk)
        name_key = name_match.group(0).decode("ascii") if name_match else ""
        if name_key or abilities:
            out[node_key] = {"name_key": name_key, "abilities": abilities}
    return out


def module_name_key(system_key: str) -> str:
    """The `$…` locale key a geode module's display name lives under.

    `languages/en/ui_geode_modules.binfab` is already merged into the codex locale map,
    so the caller resolves this like every other name; an unresolved key falls back to
    the raw system key rather than an invented display name."""
    return f"$ui_geode_modules_{system_key}_name"
