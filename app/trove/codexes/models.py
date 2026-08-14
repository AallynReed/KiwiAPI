"""Row shape for the Postgres ``codex_entry`` table (see ``pg_schema``).

Codexes moved off Mongo/Beanie to Postgres (``pg_store``); this module is the
single source of truth for the column order shared by the INSERT and the row
builder, so the indexer and the store can't drift apart.
"""

from __future__ import annotations

from datetime import datetime

# Column order for `codex_entry` - must match the INSERT in `pg_store` and the
# CREATE TABLE in `pg_schema`. `data` is the JSONB bonus blob.
COLUMNS: tuple[str, ...] = (
    "branch", "path", "codex_type", "content_sha256", "name", "category",
    "description", "tradable", "mastery", "mastery_geode", "power_rank",
    "name_key", "desc_key", "blueprint", "data", "indexed_at",
)


def to_row(entry: dict, branch: str, content_sha256: str, indexed_at: datetime) -> tuple:
    """An extracted entry dict -> a positional row tuple in ``COLUMNS`` order."""
    return (
        branch, entry["path"], entry["codex_type"], content_sha256,
        entry.get("name", ""), entry.get("category", ""), entry.get("description", ""),
        entry.get("tradable"), entry.get("mastery"), entry.get("mastery_geode"),
        entry.get("power_rank"), entry.get("name_key"), entry.get("desc_key"),
        entry.get("blueprint"), entry.get("data") or {}, indexed_at,
    )


# --- child tables -----------------------------------------------------------
#
# The scalar bonus rows the extractor leaves in `entry["data"]` are ALSO written to
# their own tables so they can be queried directly. `data` stays the display payload;
# these are the queryable projection of the same decode, never a second decode.

STAT_COLUMNS: tuple[str, ...] = (
    "branch", "path", "ord", "stat_key", "stat_id", "stat_name", "operation",
    "amount", "value", "is_percent", "slot_key", "label", "level",
)

ABILITY_COLUMNS: tuple[str, ...] = (
    "branch", "path", "ord", "ref", "hidden", "loc_key", "name", "description",
)

LINK_COLUMNS: tuple[str, ...] = (
    "branch", "src_path", "rel", "dst_path", "ord", "qty", "data",
)

REQUIREMENT_COLUMNS: tuple[str, ...] = (
    "branch", "collection", "rank", "rank_name", "badge_id", "completion_kind",
    "requirement_key", "label", "amount", "difficulty", "status", "context",
)

UPGRADE_COLUMNS: tuple[str, ...] = (
    "branch", "system_kind", "system_key", "node_key", "rank", "source_path",
    "costs", "requires", "effects",
)


def _f(value) -> float | None:
    """A finite float, or None. Postgres rejects NaN/Infinity in DOUBLE PRECISION
    columns, and a decoded float can legitimately be either."""
    try:
        number = float(value)
    except (TypeError, ValueError):
        return None
    return number if number == number and abs(number) != float("inf") else None


def stat_rows(entry: dict, branch: str) -> list[tuple]:
    """`data.stats` -> `codex_stat` row tuples, numbered in source order."""
    out: list[tuple] = []
    for ord_, stat in enumerate((entry.get("data") or {}).get("stats") or []):
        key = stat.get("stat")
        if not key:
            continue
        operation = stat.get("operation")
        out.append((
            branch, entry["path"], ord_, key, stat.get("stat_id"),
            stat.get("stat_name") or "",
            operation if isinstance(operation, str) else str(operation or ""),
            _f(stat.get("amount")), _f(stat.get("value")),
            bool(stat.get("is_percent")), stat.get("slot"),
            str(stat.get("label") or ""), int(stat.get("level") or 0),
        ))
    return out


def ability_rows(entry: dict, branch: str) -> list[tuple]:
    """`data.abilities` -> `codex_ability` row tuples."""
    out: list[tuple] = []
    for ord_, ability in enumerate((entry.get("data") or {}).get("abilities") or []):
        ref = ability.get("ref")
        if not ref:
            continue
        out.append((
            branch, entry["path"], ord_, ref, bool(ability.get("hidden")),
            ability.get("key"), ability.get("name") or "",
            ability.get("description") or "",
        ))
    return out


def requirement_rows(rows: list[dict], branch: str) -> list[tuple]:
    """Decoded badge rows -> `codex_requirement` tuples.

    A row with no resolved collection is dropped: the table is keyed on the badge it
    belongs to, and a requirement that can't name its badge has nothing to attach to."""
    out: list[tuple] = []
    seen: set[tuple[str, int]] = set()
    for row in rows:
        collection = row.get("collection") or ""
        rank = int(row.get("rank") or 0)
        if not collection or (collection, rank) in seen:
            continue
        seen.add((collection, rank))
        amount = row.get("amount")
        out.append((
            branch, collection, rank, row.get("rank_name") or "",
            row.get("badge_id") or "", row.get("completion_kind") or "",
            row.get("requirement_key") or "", row.get("label") or "",
            int(amount) if isinstance(amount, (int, float)) else None,
            int(row.get("difficulty") or 0), row.get("status") or "",
            row.get("context") or {},
        ))
    return out


def upgrade_rows(parsed: dict, branch: str, source_path: str,
                 effects: dict[str, dict] | None = None) -> list[tuple]:
    """A `upgrades.parse_upgrade_costs` result -> `codex_upgrade` tuples.

    ``effects`` is the node_key -> effect map from the sibling `upgrades/` file; a node
    with no entry stores `{}` rather than a placeholder, so "we know it grants nothing"
    and "the file didn't say" stay distinguishable from the row alone."""
    effects = effects or {}
    return [
        (branch, parsed["system_kind"], parsed["system_key"], node["node_key"],
         node.get("rank"), source_path, node.get("costs") or [],
         node.get("requires") or [], effects.get(node["node_key"]) or {})
        for node in parsed.get("nodes") or []
    ]
