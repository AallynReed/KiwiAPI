"""Read side of the codexes: counts, filtered/sorted/paged search, lookup.

Thin layer over ``pg_store`` (the Postgres ``codex_entry`` table) - it owns the
SQL; this module is the router-facing surface and the place the
``postgres_enabled`` guard lives, so the API degrades to empty results instead of
raising when Postgres isn't configured (local dev without a DSN).
"""

from __future__ import annotations

from app.core.config import settings
from app.trove.codexes import pg_store

# Re-export the sort whitelist so the router validates against the same keys the
# store knows how to translate to SQL.
SORTS = pg_store.SORTS
DEFAULT_SORT = pg_store.DEFAULT_SORT


async def query_entries(
    branch: str, *, codex_type: str | None = None, search: str | None = None,
    category: str | None = None, tradable: bool | None = None,
    sort: str = DEFAULT_SORT, limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """Filtered, sorted, paged entries + the total match count."""
    if not settings.postgres_enabled:
        return [], 0
    return await pg_store.query_entries(
        branch, codex_type=codex_type, search=search, category=category,
        tradable=tradable, sort=sort, limit=limit, offset=offset,
    )


async def type_counts(branch: str) -> list[dict]:
    """Per-type entry counts for a branch, ordered by type."""
    if not settings.postgres_enabled:
        return []
    return await pg_store.type_counts(branch)


async def list_categories(branch: str, codex_type: str | None = None) -> list[dict]:
    """Distinct non-empty categories (+ counts) for filter dropdowns, A->Z."""
    if not settings.postgres_enabled:
        return []
    return await pg_store.list_categories(branch, codex_type)


async def get_entry(branch: str, codex_type: str, path: str) -> dict | None:
    """A single entry by its source prefab path (None if absent)."""
    if not settings.postgres_enabled:
        return None
    return await pg_store.get_entry(branch, codex_type, path)


async def all_recipes(branch: str) -> list[dict]:
    """Every recipe entry (path/name/category/data) - the crafting calculator's
    dependency-tree source. Empty when Postgres isn't configured (dev)."""
    if not settings.postgres_enabled:
        return []
    return await pg_store.all_recipes(branch)
