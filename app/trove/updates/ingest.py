"""The sync pipeline: probe a branch, download only what changed, dedup into the CAS.

`sync_branch` takes its collaborators (CDN client, blob store, persistence repo) as
parameters so the whole flow is unit-testable offline with fakes. Per probe:

  pointer → manifest → diff vs sidecar (opaque sha1)
    ├─ loose file changed → download → CAS → record
    └─ archive dir touched → download index.tfi → TFI-diff (FNV) → download only the
       changed archiveN.tfa → extract only the changed slices → CAS → record

Downloads + extraction run with bounded concurrency (memory stays ≈ N archives in
flight, since each archive's slices are stored and freed before the next); the repo
batches its writes. Storage grows by genuinely-new content only; the CAS is shared.
"""

from __future__ import annotations

import asyncio
import logging
from collections import defaultdict

from app.trove.updates import archive
from app.trove.updates.cas import ContentStore
from app.trove.updates.diff import (
    classify_manifest_diff,
    diff_logical,
    join_logical,
    parent_dir,
)

logger = logging.getLogger("kiwi.trove.updates")


async def _gather_bounded(sem, fn, items) -> None:
    """Run `fn(item)` for each item, at most `sem` of them in flight at once."""
    async def _wrap(item) -> None:
        async with sem:
            await fn(item)
    await asyncio.gather(*[_wrap(it) for it in items])


async def sync_branch(branch: str, pointer_file: str, cdn, store: ContentStore,
                      repo, *, download_concurrency: int = 6) -> dict:
    """Run one probe/sync for a branch (its bootstrap pointer is `pointer_file`)."""
    pointer = await cdn.fetch_pointer(pointer_file)
    content_path = pointer["content_path"]
    version, entries = await cdn.fetch_manifest(content_path, pointer["version"])

    old_manifest = await repo.get_manifest_sidecar(branch)
    plan = classify_manifest_diff(old_manifest, entries)
    if not plan["any"]:
        await repo.touch_probe(branch, content_path, version)
        return {"branch": branch, "version": version, "changed": False,
                "added": 0, "modified": 0, "removed": 0, "bytes_added": 0}

    ordinal, _resumed = await repo.begin_version(branch, version, pointer)
    sem = asyncio.Semaphore(download_concurrency)
    counts = {"added": 0, "modified": 0, "removed": 0, "bytes_added": 0}

    async def _store(data: bytes, change_type: str, path: str, fnv: int | None,
                     archive_dir: str | None, archive_index: int | None) -> None:
        sha, created = await asyncio.to_thread(store.put, data)
        await repo.record_change(branch, ordinal, path, change_type, sha, fnv, len(data))
        await repo.upsert_state(branch, path, sha, fnv, len(data), archive_dir, archive_index, ordinal)
        counts[change_type] += 1
        if created:
            counts["bytes_added"] += len(data)
        # Periodic heartbeat so a long first sync shows progress in the logs.
        total = counts["added"] + counts["modified"]
        if total % 5000 == 0:
            logger.info("updates[%s]: %d files, %d MiB stored so far…",
                        branch, total, counts["bytes_added"] // (1024 * 1024))

    # ---- loose files (path == logical path; opaque sha1 detects change) ----
    async def _do_loose(path: str) -> None:
        e = plan["new_map"][path]
        data = await cdn.download_file(content_path, path, e["sha1"], expected_size=e["size"])
        change_type = "modified" if await repo.state_get(branch, path) else "added"
        await _store(data, change_type, path, None, None, None)
        await repo.set_manifest_entry(branch, path, e["sha1"], e["size"])

    await _gather_bounded(sem, _do_loose, plan["changed_loose"])

    for path in plan["removed_loose"]:
        if await repo.state_get(branch, path):
            await repo.record_change(branch, ordinal, path, "removed", None, None, 0)
            await repo.remove_state(branch, path)
            counts["removed"] += 1
        await repo.remove_manifest_entry(branch, path)

    # ---- archive directories (one at a time; archives within a dir in parallel) ----
    for directory, work in plan["dir_work"].items():
        await _sync_directory(branch, directory, work, plan, old_manifest,
                              cdn, store, repo, content_path, sem, counts, ordinal, _store)

    await repo.finish_version(branch, ordinal, version, pointer, counts)
    return {"branch": branch, "version": version, "changed": True, "ordinal": ordinal, **counts}


async def _sync_directory(branch, directory, work, plan, old_manifest, cdn, store, repo,
                          content_path, sem, counts, ordinal, _store) -> None:
    tfi_path = work["tfi_path"]

    # Whole directory gone from the manifest → every logical file under it is removed.
    if tfi_path is None:
        for path in await repo.get_archive_state(branch, directory):
            await repo.record_change(branch, ordinal, path, "removed", None, None, 0)
            await repo.remove_state(branch, path)
            counts["removed"] += 1
        for p in [p for p in old_manifest if parent_dir(p) == directory]:
            await repo.remove_manifest_entry(branch, p)
        return

    tfi_e = plan["new_map"][tfi_path]
    tfi_bytes = await cdn.download_file(content_path, tfi_path, tfi_e["sha1"], expected_size=tfi_e["size"])
    tfi_entries = archive.parse_tfi(tfi_bytes)
    new_full = {join_logical(directory, e.name): e for e in tfi_entries}

    old_dir = await repo.get_archive_state(branch, directory)
    old_fnv = {p: st.get("fnv_hash") for p, st in old_dir.items()}
    new_fnv = {p: e.fnv_hash for p, e in new_full.items()}
    ld = diff_logical(old_fnv, new_fnv)

    # Group changed logical files by the archive that holds them; only those archives
    # need downloading (an archive whose manifest changed but holds nothing new - e.g.
    # only a removal - is just a sidecar update, no download).
    changed_by_archive: dict[int, list[str]] = defaultdict(list)
    for path in ld["added"] + ld["modified"]:
        changed_by_archive[new_full[path].archive_index].append(path)
    archives = sorted(set(work["changed_archives"]) | set(changed_by_archive))

    async def _process_archive(ai: int) -> None:
        tfa_path = join_logical(directory, f"archive{ai}.tfa")
        tfa_e = plan["new_map"].get(tfa_path)
        names = changed_by_archive.get(ai, [])
        if names and tfa_e is not None:
            tfa_bytes = await cdn.download_file(content_path, tfa_path, tfa_e["sha1"], expected_size=tfa_e["size"])
            content = await asyncio.to_thread(archive.decompress_tfa, tfa_bytes)
            for path in names:
                e = new_full[path]
                change_type = "added" if path not in old_fnv else "modified"
                await _store(content[e.offset:e.offset + e.size], change_type, path,
                             e.fnv_hash, directory, e.archive_index)
        if tfa_e is not None:
            await repo.set_manifest_entry(branch, tfa_path, tfa_e["sha1"], tfa_e["size"])

    await _gather_bounded(sem, _process_archive, archives)

    for path in ld["removed"]:
        await repo.record_change(branch, ordinal, path, "removed", None, None, 0)
        await repo.remove_state(branch, path)
        counts["removed"] += 1
    await repo.set_manifest_entry(branch, tfi_path, tfi_e["sha1"], tfi_e["size"])
