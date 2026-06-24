"""Resolve a blueprint logical name to its raw ``.blueprint`` bytes.

Primary source: the updates content store (the mirrored game archive) -- parse
``blueprints/index.tfi`` to find the entry, then extract it from its
``archiveN.tfa``. Dev fallback: a local game install (``settings.trove_local_game_dir``)
read straight off disk. The parsed TFI index is cached per content hash since it's
shared by every render until the next game update.
"""
from __future__ import annotations

import asyncio
import logging
from pathlib import Path

from app.core.config import settings
from app.trove.updates.archive import (
    ArchiveError,
    TfiEntry,
    extract_archive,
    parse_tfi,
)
from app.trove.updates.cas import ContentStore
from app.trove.updates.read import get_file_meta

logger = logging.getLogger("kiwi.render")

BLUEPRINTS_DIR = "blueprints"
# (sha-or-path) -> {"entries": [...], "by_name": {...}, "by_base": {...}}
_tfi_cache: dict[str, dict] = {}


def _index(entries: list[TfiEntry]) -> dict:
    by_name: dict[str, TfiEntry] = {}
    by_base: dict[str, TfiEntry] = {}
    for e in entries:
        by_name[e.name.lower()] = e
        by_base.setdefault(e.name.rsplit("/", 1)[-1].lower(), e)
    return {"entries": entries, "by_name": by_name, "by_base": by_base}


def _normalize(name: str) -> list[str]:
    """Candidate TFI names for a requested blueprint reference."""
    n = name.replace("\\", "/").strip().lower()
    if n.startswith("blueprints/"):
        n = n[len("blueprints/"):]
    cands = {n}
    if not n.endswith(".blueprint"):
        cands.add(n + ".blueprint")
    return list(cands)


def _lookup(idx: dict, name: str) -> TfiEntry | None:
    for cand in _normalize(name):
        e = idx["by_name"].get(cand)
        if e:
            return e
    # fall back to basename match (codex refs sometimes carry a different subdir)
    for cand in _normalize(name):
        e = idx["by_base"].get(cand.rsplit("/", 1)[-1])
        if e:
            return e
    return None


# --------------------------------------------------------------------------- #
# updates content store (production)
# --------------------------------------------------------------------------- #
async def _from_store(name: str, branch: str) -> bytes | None:
    clean_name = name.replace("\\", "/").strip()
    if clean_name.lower().startswith("blueprints/"):
        clean_name = clean_name[len("blueprints/"):]
    path = f"blueprints/{clean_name}"

    from app.trove.updates.models import UpdateState
    import re

    d = await UpdateState.find_one({"branch": branch, "path": path})
    if d is None:
        d = await UpdateState.find_one({"branch": branch, "path": path.lower()})
    if d is None:
        escaped = re.escape(path)
        d = await UpdateState.find_one({"branch": branch, "path": {"$regex": f"^{escaped}$", "$options": "i"}})

    if d is None:
        return None

    store = ContentStore(settings.trove_update_store_dir)
    return await asyncio.to_thread(store.get, d.content_sha256)


# --------------------------------------------------------------------------- #
# local game install (dev fallback)
# --------------------------------------------------------------------------- #
def _from_local_sync(name: str) -> bytes | None:
    root = settings.trove_local_game_dir
    if not root:
        return None
    bp = Path(root) / BLUEPRINTS_DIR

    clean_name = name.replace("\\", "/").strip()
    if clean_name.lower().startswith("blueprints/"):
        clean_name = clean_name[len("blueprints/"):]

    parts = [p for p in clean_name.split("/") if p]
    if not parts:
        return None

    tfi_path = None
    archive_dir_path = bp
    relative_name = clean_name

    for i in range(len(parts) - 1, -1, -1):
        sub_dir = "/".join(parts[:i])
        candidate_dir = bp / sub_dir if sub_dir else bp
        cand_tfi = candidate_dir / "index.tfi"
        if cand_tfi.is_file():
            tfi_path = cand_tfi
            archive_dir_path = candidate_dir
            relative_name = "/".join(parts[i:])
            break

    if not tfi_path:
        return None

    key = f"local:{tfi_path}:{tfi_path.stat().st_mtime_ns}"
    idx = _tfi_cache.get(key)
    if idx is None:
        idx = _index(parse_tfi(tfi_path.read_bytes()))
        _tfi_cache[key] = idx
    entry = _lookup(idx, relative_name)
    if entry is None:
        return None
    # a loose override (blueprints/override/<name>) wins, matching the game tool
    override = bp / "override" / entry.name.rsplit("/", 1)[-1]
    if override.is_file():
        return override.read_bytes()
    tfa_path = archive_dir_path / f"archive{entry.archive_index}.tfa"
    if not tfa_path.is_file():
        return None
    return extract_archive(tfa_path.read_bytes(), idx["entries"], entry.archive_index).get(entry.name)


async def get_blueprint_bytes(name: str, branch: str | None = None) -> bytes | None:
    """Raw ``.blueprint`` bytes for a logical name, or None if not found."""
    branch = branch or settings.trove_render_branch
    try:
        data = await _from_store(name, branch)
        if data:
            return data
    except Exception as e:  # noqa: BLE001 - store is best-effort; fall back to local
        logger.warning("render: store lookup failed for %r: %s", name, e)
    try:
        return await asyncio.to_thread(_from_local_sync, name)
    except (ArchiveError, OSError) as e:
        logger.warning("render: local lookup failed for %r: %s", name, e)
        return None
