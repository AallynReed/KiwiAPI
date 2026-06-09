"""Read side of the codexes: counts, filtered/sorted/paged search, lookup.

Serves straight from the materialized ``CodexEntry`` collection - no archive or
CAS access on the hot path. ``query_entries`` is the one filter engine behind both
the per-type listing and the cross-type search: optional type, name/description
substring, exact category, tradability, with a whitelisted sort and paging.
"""

from __future__ import annotations

import re

from app.trove.codexes.models import CodexEntry

# Whitelisted sort keys -> Beanie sort fields (so user input can't sort on
# arbitrary fields). Secondary `name` keeps results stable within a group.
SORTS: dict[str, tuple[str, ...]] = {
    "name": ("name",),
    "-name": ("-name",),
    "category": ("category", "name"),
    "-category": ("-category", "name"),
    "mastery": ("mastery", "name"),
    "-mastery": ("-mastery", "name"),
    "indexed_at": ("indexed_at", "name"),
    "-indexed_at": ("-indexed_at", "name"),
}
DEFAULT_SORT = "name"


def _filter(
    branch: str, *, codex_type: str | None, search: str | None,
    category: str | None, tradable: bool | None,
) -> dict:
    query: dict = {"branch": branch}
    if codex_type:
        query["codex_type"] = codex_type
    if category:
        query["category"] = category
    if tradable is not None:
        query["tradable"] = tradable
    if search:
        # Case-insensitive substring on name OR description; escaped so user input
        # can't inject regex metacharacters.
        rx = {"$regex": re.escape(search), "$options": "i"}
        query["$or"] = [{"name": rx}, {"description": rx}]
    return query


async def query_entries(
    branch: str, *, codex_type: str | None = None, search: str | None = None,
    category: str | None = None, tradable: bool | None = None,
    sort: str = DEFAULT_SORT, limit: int = 50, offset: int = 0,
) -> tuple[list[CodexEntry], int]:
    """Filtered, sorted, paged entries + the total match count."""
    query = _filter(branch, codex_type=codex_type, search=search,
                    category=category, tradable=tradable)
    order = SORTS.get(sort, SORTS[DEFAULT_SORT])
    total = await CodexEntry.find(query).count()
    docs = await CodexEntry.find(query).sort(*order).skip(offset).limit(limit).to_list()
    return docs, total


async def type_counts(branch: str) -> list[dict]:
    """Per-type entry counts for a branch, ordered by type."""
    cursor = await CodexEntry.get_pymongo_collection().aggregate([
        {"$match": {"branch": branch}},
        {"$group": {"_id": "$codex_type", "count": {"$sum": 1}}},
        {"$sort": {"_id": 1}},
    ])
    rows = await cursor.to_list(length=None)
    return [{"type": r["_id"], "count": r["count"]} for r in rows]


async def list_categories(branch: str, codex_type: str | None = None) -> list[dict]:
    """Distinct non-empty categories (+ counts) for filter dropdowns, A→Z."""
    match: dict = {"branch": branch, "category": {"$nin": [None, ""]}}
    if codex_type:
        match["codex_type"] = codex_type
    cursor = await CodexEntry.get_pymongo_collection().aggregate([
        {"$match": match},
        {"$group": {"_id": "$category", "count": {"$sum": 1}}},
        {"$sort": {"_id": 1}},
    ])
    rows = await cursor.to_list(length=None)
    return [{"category": r["_id"], "count": r["count"]} for r in rows]


async def get_entry(branch: str, codex_type: str, path: str) -> CodexEntry | None:
    return await CodexEntry.find_one(
        {"branch": branch, "codex_type": codex_type, "path": path}
    )
