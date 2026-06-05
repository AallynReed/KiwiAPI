"""Pure diff/classification helpers for the ingestion pipeline (no I/O).

Two levels:
  - manifest level: which top-level files changed (loose vs archive), keyed by the
    opaque manifest sha1; produces the per-directory work list.
  - logical level: within a directory, which extracted logical files changed,
    keyed by the TFI's per-file FNV hash.
"""

from __future__ import annotations

import re

_ARCHIVE_RE = re.compile(r"archive(\d+)\.tfa$")


def is_tfa(path: str) -> bool:
    return path.endswith(".tfa")


def is_tfi(path: str) -> bool:
    return path.endswith(".tfi")


def parent_dir(path: str) -> str:
    return path.rsplit("/", 1)[0] if "/" in path else ""


def archive_index_of(path: str) -> int | None:
    m = _ARCHIVE_RE.search(path)
    return int(m.group(1)) if m else None


def classify_manifest_diff(old: dict[str, dict], new_entries: list[dict]) -> dict:
    """Compare the new manifest to the stored sidecar.

    old:  {path: {"sha1","size"}}        (last-seen manifest)
    new:  [{"path","sha1","size"}, …]    (current manifest)
    Returns changed/removed loose files plus per-directory archive work.
    """
    new_map = {e["path"]: e for e in new_entries}
    changed = [p for p, e in new_map.items() if old.get(p, {}).get("sha1") != e["sha1"]]
    removed = [p for p in old if p not in new_map]
    touched = set(changed) | set(removed)

    changed_loose = sorted(p for p in changed if not is_tfa(p) and not is_tfi(p))
    removed_loose = sorted(p for p in removed if not is_tfa(p) and not is_tfi(p))

    arch_dirs = {parent_dir(p) for p in touched if is_tfa(p) or is_tfi(p)}
    dir_work: dict[str, dict] = {}
    for d in sorted(arch_dirs):
        tfi_path = next((p for p in new_map if is_tfi(p) and parent_dir(p) == d), None)
        changed_archives = sorted(
            ai for p in changed
            if is_tfa(p) and parent_dir(p) == d and (ai := archive_index_of(p)) is not None
        )
        dir_work[d] = {"tfi_path": tfi_path, "changed_archives": changed_archives}
    return {
        "changed_loose": changed_loose,
        "removed_loose": removed_loose,
        "dir_work": dir_work,
        "new_map": new_map,
        "any": bool(changed_loose or removed_loose or dir_work),
    }


def diff_logical(old_fnv: dict[str, int], new_fnv: dict[str, int]) -> dict:
    """Compare TFI FNV hashes (logical_path → fnv) to find added/modified/removed."""
    added = sorted(p for p in new_fnv if p not in old_fnv)
    modified = sorted(p for p in new_fnv if p in old_fnv and old_fnv[p] != new_fnv[p])
    removed = sorted(p for p in old_fnv if p not in new_fnv)
    return {"added": added, "modified": modified, "removed": removed}


def join_logical(directory: str, name: str) -> str:
    """Full logical path from a TFI's directory + an entry name (both posix)."""
    return f"{directory}/{name}" if directory else name
