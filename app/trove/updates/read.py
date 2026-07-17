"""Read side of the archive: branches, version history, directory browsing, file fetch.

The materialized `UpdateState` per branch *is* the current tree, so listings/fetches
are direct lookups (no change-log replay). Directory listings compute one level at a
time (ls-style) so a 100k-file tree is never dumped at once.
"""

from __future__ import annotations

import asyncio
from datetime import datetime

from app.core.config import settings
from app.core.pagination import paginate
from app.trove.updates.cas import ContentStore
from app.trove.updates.models import UpdateBranch, UpdateChange, UpdateState, UpdateVersion

# Cap for the in-browser file viewer: text and binary (hex) previews are only
# produced for files at/under this size, so a click never dumps a huge blob into
# the page. Images render through an <img> tag (the browser streams the bytes),
# so they aren't bound by this cap.
VIEW_MAX_BYTES = 1024 * 1024  # 1 MB

# Extensions the browser can render natively in an <img>. DDS is handled
# separately (decoded client-side to a canvas) since browsers can't render it.
IMAGE_EXTS = frozenset({"png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "apng"})


def directory_listing(entries: list[dict], prefix: str) -> list[dict]:
    """Immediate children of `prefix` from a flat (path, size, last_ordinal) list. Pure.

    A child is a file (no further '/') or a subdirectory (has more path below it),
    with file counts + total bytes rolled up and ``last_ordinal`` set to the newest
    version touched underneath it (so a directory sorts by its most-recent change).
    Directories first, then files, each alphabetical.
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
            name, {"name": name, "path": prefix + name, "is_dir": slash != -1,
                   "file_count": 0, "size": 0, "last_ordinal": 0}
        )
        if slash != -1:
            child["is_dir"] = True
            child["path"] = prefix + name + "/"
        child["file_count"] += 1
        child["size"] += e.get("size", 0)
        child["last_ordinal"] = max(child["last_ordinal"], e.get("last_ordinal", 0) or 0)
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
    return await paginate(
        UpdateVersion.find({"branch": branch, "status": "complete"}),
        sort="-ordinal", limit=limit, offset=offset,
    )


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
    """Viewer payload for one file, classified by ``kind`` so the client knows how
    to render it:

    * ``text``      - UTF-8 ``text`` for small, text-like files.
    * ``image``     - a browser-native image; render via ``<img>`` (no bytes here).
    * ``binary``    - small non-text file; the client fetches the raw bytes and
      renders a hex viewer.
    * ``too_large`` - non-image file above ``max_bytes``; download to inspect.
    * ``missing``   - blob absent from the store.

    ``reason`` mirrors ``kind`` for the non-text cases (kept for older clients).
    None when the path isn't in the latest tree. Sync blob I/O runs in a thread."""
    meta = await get_file_meta(branch, path)
    if meta is None:
        return None
    base = {
        "branch": branch, "path": path, "size": meta["size"],
        "content_sha256": meta["content_sha256"], "truncated": False, "text": None,
    }
    ext = path.rsplit(".", 1)[-1].lower() if "." in path.rsplit("/", 1)[-1] else ""
    # Images render straight through an <img> tag - no size cap, no blob read here.
    if ext in IMAGE_EXTS:
        return {**base, "viewable": False, "kind": "image", "reason": "image"}
    # DDS textures decode to a canvas client-side; .blueprint models open in the 3D
    # viewer. Both fetch their own bytes/payload, so no blob read or size cap here.
    if ext == "dds":
        return {**base, "viewable": False, "kind": "dds", "reason": "dds"}
    if ext == "blueprint":
        return {**base, "viewable": False, "kind": "blueprint", "reason": "blueprint"}
    if meta["size"] > max_bytes:
        return {**base, "viewable": False, "kind": "too_large", "reason": "too_large"}
    blob = ContentStore(settings.trove_update_store_dir).path_for(meta["content_sha256"])
    if not blob.is_file():
        return {**base, "viewable": False, "kind": "missing", "reason": "missing"}
    data = await asyncio.to_thread(blob.read_bytes)
    if b"\x00" in data:                       # NUL byte -> treat as binary
        return {**base, "viewable": False, "kind": "binary", "reason": "binary"}
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        return {**base, "viewable": False, "kind": "binary", "reason": "binary"}
    return {**base, "viewable": True, "kind": "text", "reason": None, "text": text}


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


async def _captured_at_by_ordinal(branch: str) -> dict[int, datetime]:
    """{ordinal → captured_at} for a branch, so a materialized ``last_ordinal`` can be
    rendered as a real timestamp without a per-row version join. The version set is
    tiny (one doc per build) relative to the file tree."""
    coll = UpdateVersion.get_pymongo_collection()
    docs = await coll.find({"branch": branch}, {"ordinal": 1, "captured_at": 1, "_id": 0}).to_list(length=None)
    return {d["ordinal"]: d.get("captured_at") for d in docs}


def _attach_modified(entries: list[dict], captured: dict[int, datetime]) -> list[dict]:
    """Add ``last_modified_at`` (from the version map) to each entry in place."""
    for e in entries:
        e["last_modified_at"] = captured.get(e.get("last_ordinal", 0) or 0)
    return entries


async def list_directory(branch: str, prefix: str) -> list[dict]:
    query: dict = {"branch": branch}
    if prefix:
        query["path"] = {"$gte": prefix, "$lt": prefix + "￿"}  # range scan on the (branch, path) index
    coll = UpdateState.get_pymongo_collection()
    entries = await coll.find(
        query, {"path": 1, "size": 1, "last_ordinal": 1, "_id": 0},
    ).to_list(length=None)
    listing = directory_listing(entries, prefix)
    return _attach_modified(listing, await _captured_at_by_ordinal(branch))


async def search_paths(branch: str, needle: str, limit: int = 200) -> tuple[list[dict], int]:
    """Full-tree substring path search (case-insensitive), capped at ``limit``,
    with the true match ``total`` so the UI can say "showing 200 of N". The
    needle is regex-escaped - it's a literal substring, never a user regex.
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
    cursor = coll.find(query, {"path": 1, "size": 1, "last_ordinal": 1, "_id": 0}).sort("path", 1).limit(limit)
    docs = await cursor.to_list(length=limit)
    entries = [
        {
            "path": d["path"],
            "name": d["path"].rsplit("/", 1)[-1],
            "size": d.get("size", 0),
            "is_dir": False,
            "last_ordinal": d.get("last_ordinal", 0),
        }
        for d in docs
    ]
    _attach_modified(entries, await _captured_at_by_ordinal(branch))
    return entries, total


async def get_file_meta(branch: str, path: str) -> dict | None:
    d = await UpdateState.find_one({"branch": branch, "path": path})
    if d is None:
        return None
    return {"path": d.path, "content_sha256": d.content_sha256, "size": d.size,
            "archive": d.archive, "archive_index": d.archive_index}


# A file's history is a range scan on the (branch, path, ordinal) change-log
# index; version metadata (tag, captured_at) is joined in a small parallel fetch.

async def file_history(branch: str, path: str) -> list[dict]:
    """Every change to ``path`` on ``branch``, newest version first, with each
    version's tag + ``captured_at`` joined in so a timeline renders without a
    round-trip per row.
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
    """Blob coordinates for ``path`` as of ``ordinal``: the most recent
    non-``removed`` change at or before it. ``None`` if the path didn't exist
    (or was already removed) then.
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
