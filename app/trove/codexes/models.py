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
