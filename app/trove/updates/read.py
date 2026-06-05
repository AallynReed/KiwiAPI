"""Read side of the archive: branches, version history, directory browsing, file fetch.

Latest-version only for now — the materialized `UpdateState` per branch *is* the
current tree, so listings/fetches are direct lookups (no change-log replay).
Directory listings compute one level at a time (ls-style) so a 100k-file tree is
never dumped at once.
"""

from __future__ import annotations

from app.trove.updates.models import UpdateBranch, UpdateState, UpdateVersion


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


async def list_directory(branch: str, prefix: str) -> list[dict]:
    query: dict = {"branch": branch}
    if prefix:
        query["path"] = {"$gte": prefix, "$lt": prefix + "￿"}  # range scan on the (branch, path) index
    coll = UpdateState.get_pymongo_collection()
    entries = await coll.find(query, {"path": 1, "size": 1, "_id": 0}).to_list(length=None)
    return directory_listing(entries, prefix)


async def get_file_meta(branch: str, path: str) -> dict | None:
    d = await UpdateState.find_one({"branch": branch, "path": path})
    if d is None:
        return None
    return {"path": d.path, "content_sha256": d.content_sha256, "size": d.size,
            "archive": d.archive, "archive_index": d.archive_index}
