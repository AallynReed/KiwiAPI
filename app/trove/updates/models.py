"""Mongo models for the game-file version archive.

- UpdateBranch        - one per timeline (live-us / pts-us): where it's at.
- UpdateVersion       - one per detected build; `ordinal` orders the history.
- UpdateChange        - append-only log of logical-file changes (source of truth).
- UpdateState         - materialized current logical tree per branch (+ TFI baseline).
- UpdateManifestEntry - last-seen opaque manifest sha1 per top-level file (the sidecar).

Blob bytes live in the filesystem CAS, not here - these only hold metadata/hashes.
"""

from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class UpdateBranch(Document):
    branch: str  # "live-us" | "pts-us" - unique
    content_path: str = ""
    current_version: str | None = None
    current_ordinal: int = 0
    last_probe_at: datetime | None = None
    status: str = "idle"  # idle | syncing | error
    last_error: str | None = None

    class Settings:
        name = "update_branches"
        indexes = [IndexModel([("branch", ASCENDING)], unique=True)]


class UpdateVersion(Document):
    branch: str
    ordinal: int                      # 1, 2, 3 … stable ordering within a branch
    version_tag: str
    status: str = "in_progress"       # in_progress | complete
    captured_at: datetime = Field(default_factory=utcnow)
    completed_at: datetime | None = None
    content_path: str = ""
    motd: str = ""
    files_added: int = 0
    files_modified: int = 0
    files_removed: int = 0
    bytes_added: int = 0               # only newly-stored blob bytes (post-dedup)

    class Settings:
        name = "update_versions"
        indexes = [
            IndexModel([("branch", ASCENDING), ("ordinal", ASCENDING)], unique=True),
            IndexModel([("branch", ASCENDING), ("version_tag", ASCENDING)]),
            IndexModel([("branch", ASCENDING), ("status", ASCENDING)]),
        ]


class UpdateChange(Document):
    branch: str
    ordinal: int
    path: str                         # logical path
    type: str                         # added | modified | removed
    content_sha256: str | None = None  # None for removed
    fnv_hash: int | None = None
    size: int = 0

    class Settings:
        name = "update_changes"
        indexes = [
            # Unique per (branch, version, path) so the bulk upsert is idempotent on
            # resume; also serves version-listing (branch[, ordinal]) prefix queries.
            IndexModel([("branch", ASCENDING), ("ordinal", ASCENDING), ("path", ASCENDING)], unique=True),
            # File-history lookups: latest change to a path at/below a version.
            IndexModel([("branch", ASCENDING), ("path", ASCENDING), ("ordinal", ASCENDING)]),
            IndexModel([("content_sha256", ASCENDING)]),
        ]


class UpdateState(Document):
    branch: str
    path: str                         # logical path - unique per branch
    content_sha256: str
    fnv_hash: int | None = None       # None for loose (non-archive) files
    size: int = 0
    archive: str | None = None        # the TFI directory for archive-sourced files, else None
    archive_index: int | None = None
    last_ordinal: int = 0             # version this file was last added/modified in (0 = not
                                      #   yet backfilled; drives the "last modified" tree sort)

    class Settings:
        name = "update_state"
        indexes = [
            IndexModel([("branch", ASCENDING), ("path", ASCENDING)], unique=True),
            IndexModel([("branch", ASCENDING), ("archive", ASCENDING)]),
        ]


class UpdateManifestEntry(Document):
    branch: str
    path: str                         # top-level manifest path - unique per branch
    sha1: str                         # opaque manifest hash (change key)
    size: int

    class Settings:
        name = "update_manifest"
        indexes = [IndexModel([("branch", ASCENDING), ("path", ASCENDING)], unique=True)]
