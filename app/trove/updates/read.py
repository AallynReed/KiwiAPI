"""Read side of the archive: branches, version history, directory browsing, file fetch.

Latest-version only for now - the materialized `UpdateState` per branch *is* the
current tree, so listings/fetches are direct lookups (no change-log replay).
Directory listings compute one level at a time (ls-style) so a 100k-file tree is
never dumped at once.
"""

from __future__ import annotations

import asyncio

from app.core.config import settings
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateBranch, UpdateChange, UpdateState, UpdateVersion

# Cap for the in-browser file viewer: only files at/under this size are read and
# returned as text, so a click never dumps a huge blob into the page.
VIEW_MAX_BYTES = 512 * 1024


def directory_listing(entries: list[dict], prefix: str) -> list[dict]:
    """Immediate children of `prefix` from a flat (path, size) list. Pure.

    A child is a file (no further '/') or a subdirectory (has more path below it),
    with file counts and total bytes rolled up. Directories first, then files,
    each alphabetical.
    """
    children: dict[str, dict] = {}
    plen = len(prefix)
    for e in entries:
        path = e["path"]
        if not path.startswith(prefix):
            continue
        rest = path[plen:]
        if not rest:
            continue
        slash = rest.find("/")
        name = rest if slash == -1 else rest[:slash]
        child = children.setdefault(
            name, {"name": name, "path": prefix + name, "is_dir": slash != -1, "file_count": 0, "size": 0}
        )
        if slash != -1:
            child["is_dir"] = True
            child["path"] = prefix + name + "/"
        child["file_count"] += 1
        child["size"] += e.get("size", 0)
    return sorted(children.values(), key=lambda c: (not c["is_dir"], c["name"]))


async def list_branches() -> list[dict]:
    out = []
    for d in await UpdateBranch.find().sort("branch").to_list():
        file_count = await UpdateState.find({"branch": d.branch}).count()
        out.append({
            "branch": d.branch, "current_version": d.current_version,
            "current_ordinal": d.current_ordinal, "last_probe_at": d.last_probe_at,
            "status": d.status, "file_count": file_count,
        })
    return out


async def list_versions(branch: str, limit: int, offset: int) -> tuple[list[UpdateVersion], int]:
    q = {"branch": branch, "status": "complete"}
    total = await UpdateVersion.find(q).count()
    docs = await UpdateVersion.find(q).sort("-ordinal").skip(offset).limit(limit).to_list()
    return docs, total


async def latest_version(branch: str) -> UpdateVersion | None:
    """The newest completed version for a branch (highest ordinal), or None.

    Used by the live-event ``game_update`` source, the Discord announcement, and
    the homepage "latest patch" banner."""
    docs = await (
        UpdateVersion.find({"branch": branch, "status": "complete"})
        .sort("-ordinal").limit(1).to_list()
    )
    return docs[0] if docs else None


async def read_file_text(branch: str, path: str, max_bytes: int = VIEW_MAX_BYTES) -> dict | None:
    """Viewer payload for one file: UTF-8 ``text`` when it's small + text-like, else
    ``viewable: False`` with a ``reason`` ("too_large" / "binary" / "missing"). None
    when the path isn't in the latest tree. Sync blob I/O is off-loaded to a thread."""
    meta = await get_file_meta(branch, path)
    if meta is None:
        return None
    base = {
        "branch": branch, "path": path, "size": meta["size"],
        "content_sha256": meta["content_sha256"], "truncated": False, "text": None,
    }
    if meta["size"] > max_bytes:
        return {**base, "viewable": False, "reason": "too_large"}
    blob = ContentStore(settings.trove_update_store_dir).path_for(meta["content_sha256"])
    if not blob.is_file():
        return {**base, "viewable": False, "reason": "missing"}
    data = await asyncio.to_thread(blob.read_bytes)
    if b"\x00" in data:                       # NUL byte -> treat as binary
        return {**base, "viewable": False, "reason": "binary"}
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return {**base, "viewable": False, "reason": "binary"}
    return {**base, "viewable": True, "reason": None, "text": text}


async def resolve_version(
    branch: str, ordinal: int | None = None, version_tag: str | None = None,
) -> UpdateVersion | None:
    """Pin a version for a branch: by tag, by ordinal, or (both omitted) the latest complete one."""
    if version_tag:
        return await UpdateVersion.find_one({"branch": branch, "version_tag": version_tag})
    if ordinal is not None:
        return await UpdateVersion.find_one({"branch": branch, "ordinal": ordinal})
    latest = await UpdateVersion.find({"branch": branch, "status": "complete"}).sort("-ordinal").limit(1).to_list()
    return latest[0] if latest else None


async def list_changes(
    branch: str, ordinal: int, type_filter: str | None, limit: int, offset: int,
) -> tuple[list[UpdateChange], int]:
    """The change-log entries for one version, sorted by type then path. Optionally filtered by type."""
    q: dict = {"branch": branch, "ordinal": ordinal}
    if type_filter:
        q["type"] = type_filter
    total = await UpdateChange.find(q).count()
    docs = await UpdateChange.find(q).sort("+type", "+path").skip(offset).limit(limit).to_list()
    return docs, total


async def list_directory(branch: str, prefix: str) -> list[dict]:
    query: dict = {"branch": branch}
    if prefix:
        query["path"] = {"$gte": prefix, "$lt": prefix + "￿"}  # range scan on the (branch, path) index
    coll = UpdateState.get_pymongo_collection()
    entries = await coll.find(query, {"path": 1, "size": 1, "_id": 0}).to_list(length=None)
    return directory_listing(entries, prefix)


async def search_paths(branch: str, needle: str, limit: int = 200) -> tuple[list[dict], int]:
    """Full-tree file search: every path on ``branch`` containing ``needle``
    (case-insensitive substring), NOT just the current directory level.

    Returns ``(entries, total)`` where each entry is a full-path file row
    ``{"path", "name", "size", "is_dir": False}`` and ``total`` is the true
    match count (so the UI can say "showing 200 of N"). Results are capped at
    ``limit`` and sorted by path. The needle is escaped so it's a literal
    substring, never a user-supplied regex.
    """
    import re

    needle = needle.strip()
    if not needle:
        return [], 0
    coll = UpdateState.get_pymongo_collection()
    query: dict = {
        "branch": branch,
        "path": {"$regex": re.escape(needle), "$options": "i"},
    }
    total = await coll.count_documents(query)
    cursor = coll.find(query, {"path": 1, "size": 1, "_id": 0}).sort("path", 1).limit(limit)
    docs = await cursor.to_list(length=limit)
    entries = [
        {
            "path": d["path"],
            "name": d["path"].rsplit("/", 1)[-1],
            "size": d.get("size", 0),
            "is_dir": False,
        }
        for d in docs
    ]
    return entries, total


async def get_file_meta(branch: str, path: str) -> dict | None:
    d = await UpdateState.find_one({"branch": branch, "path": path})
    if d is None:
        return None
    return {"path": d.path, "content_sha256": d.content_sha256, "size": d.size,
            "archive": d.archive, "archive_index": d.archive_index}


# --- File history + per-version resolution ---------------------------------
# The change-log holds every (branch, path, ordinal) triple, so a file's
# history is a prefix range scan on the (branch, path, ordinal) index; the
# version metadata (tag, captured_at) is joined client-side from a small
# parallel fetch of the relevant UpdateVersion rows.

async def file_history(branch: str, path: str) -> list[dict]:
    """Every change to ``path`` on ``branch``, newest version first.

    Each row carries the change type ("added" | "modified" | "removed"),
    the resulting ``content_sha256`` + ``size`` (None / 0 for removed), the
    version ordinal + tag, and the version's ``captured_at`` timestamp so
    a chart / timeline can render without an extra round-trip per row.
    """
    docs = await UpdateChange.find({"branch": branch, "path": path}).sort("-ordinal").to_list()
    if not docs:
        return []
    ordinals = sorted({d.ordinal for d in docs})
    versions = await UpdateVersion.find(
        {"branch": branch, "ordinal": {"$in": ordinals}},
    ).to_list()
    by_ord = {v.ordinal: v for v in versions}
    out: list[dict] = []
    for d in docs:
        v = by_ord.get(d.ordinal)
        out.append({
            "ordinal": d.ordinal,
            "version_tag": v.version_tag if v else "",
            "captured_at": v.captured_at if v else None,
            "type": d.type,
            "content_sha256": d.content_sha256,
            "size": d.size,
        })
    return out


async def resolve_file_at_version(
    branch: str, path: str, ordinal: int,
) -> dict | None:
    """Resolve a file's blob coordinates AT a historical version.

    Walks back through the change-log to find the most recent
    non-``removed`` change for ``path`` at or before ``ordinal``. Returns
    ``None`` if the path never existed at that point or had been removed
    before it.
    """
    docs = await UpdateChange.find(
        {"branch": branch, "path": path, "ordinal": {"$lte": ordinal}},
    ).sort("-ordinal").limit(1).to_list()
    if not docs:
        return None
    last = docs[0]
    if last.type == "removed":
        return None
    return {
        "ordinal": last.ordinal,
        "content_sha256": last.content_sha256,
        "size": last.size,
    }
