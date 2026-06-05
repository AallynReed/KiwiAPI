"""Read side of the codexes: type counts, paginated/searchable entry lists, lookup.

Serves straight from the materialized ``CodexEntry`` collection — no archive or
CAS access on the hot path. Listings are paged and name-searchable; a single
entry is addressed by its stable source prefab path.
"""

from __future__ import annotations

import re

from app.trove.codexes.models import CodexEntry


async def type_counts(branch: str) -> list[dict]:
    """Per-type entry counts for a branch, ordered by type."""
    cursor = await CodexEntry.get_pymongo_collection().aggregate([
        {"$match": {"branch": branch}},
        {"$group": {"_id": "$codex_type", "count": {"$sum": 1}}},
        {"$sort": {"_id": 1}},
    ])
    rows = await cursor.to_list(length=None)
    return [{"type": r["_id"], "count": r["count"]} for r in rows]


async def list_entries(
    branch: str, codex_type: str, *, search: str | None, limit: int, offset: int
) -> tuple[list[CodexEntry], int]:
    query: dict = {"branch": branch, "codex_type": codex_type}
    if search:
        # Case-insensitive substring on the resolved name. Escaped so user input
        # can't inject regex metacharacters.
        query["name"] = {"$regex": re.escape(search), "$options": "i"}
    total = await CodexEntry.find(query).count()
    docs = await CodexEntry.find(query).sort("name").skip(offset).limit(limit).to_list()
    return docs, total


async def get_entry(branch: str, codex_type: str, path: str) -> CodexEntry | None:
    return await CodexEntry.find_one(
        {"branch": branch, "codex_type": codex_type, "path": path}
    )
