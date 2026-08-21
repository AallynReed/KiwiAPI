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
STAT_SORTS = pg_store.STAT_SORTS
DEFAULT_SORT = pg_store.DEFAULT_SORT


async def query_entries(
    branch: str, *, codex_type: str | None = None, search: str | None = None,
    category: str | None = None, tradable: bool | None = None,
    stat: str | None = None, ability: str | None = None,
    sort: str = DEFAULT_SORT, limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    if not settings.postgres_enabled:
        return [], 0
    return await pg_store.query_entries(
        branch, codex_type=codex_type, search=search, category=category,
        tradable=tradable, stat=stat, ability=ability,
        sort=sort, limit=limit, offset=offset,
    )


async def type_counts(branch: str) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.type_counts(branch)


async def list_categories(branch: str, codex_type: str | None = None) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.list_categories(branch, codex_type)


async def stat_keys(branch: str, codex_type: str | None = None) -> list[dict]:
    """Stats granted by entries of a type, A-Z by display name (the filter's options)."""
    if not settings.postgres_enabled:
        return []
    return await pg_store.stat_keys(branch, codex_type)


async def ability_refs(branch: str, codex_type: str | None = None) -> list[dict]:
    """Displayed abilities referenced by entries of a type, most common first."""
    if not settings.postgres_enabled:
        return []
    return await pg_store.ability_refs(branch, codex_type)


async def get_entry(branch: str, codex_type: str, path: str) -> dict | None:
    if not settings.postgres_enabled:
        return None
    return await pg_store.get_entry(branch, codex_type, path)


async def all_recipes(branch: str) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.all_recipes(branch)


async def links_for(branch: str, path: str, *, direction: str = "out",
                    relation: str | None = None, limit: int = 200) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.links_for(branch, path, direction=direction,
                                    relation=relation, limit=limit)


async def entries_with_stat(branch: str, stat_key: str, *, codex_type: str | None = None,
                            limit: int = 50, offset: int = 0) -> tuple[list[dict], int]:
    if not settings.postgres_enabled:
        return [], 0
    return await pg_store.entries_with_stat(branch, stat_key, codex_type=codex_type,
                                            limit=limit, offset=offset)


async def requirements_for(branch: str, collection: str) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.requirements_for(branch, collection)


async def upgrade_systems(branch: str) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.upgrade_systems(branch)


async def upgrade_system(branch: str, system_key: str) -> list[dict]:
    if not settings.postgres_enabled:
        return []
    return await pg_store.upgrade_system(branch, system_key)


async def blueprints_for_names(branch: str, names: list[str]) -> dict[str, str]:
    """``lower(name) -> blueprint`` for names that map to exactly one blueprint
    (see ``pg_store.blueprints_for_names``). Empty when Postgres is disabled."""
    if not settings.postgres_enabled:
        return {}
    return await pg_store.blueprints_for_names(branch, names)
