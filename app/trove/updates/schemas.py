"""Response models for the `updates:read` (archived game files) endpoints."""

from datetime import datetime

from pydantic import BaseModel, ConfigDict, Field


class BranchInfo(BaseModel):
    branch: str                       # "live-us" | "pts"
    current_version: str | None
    current_ordinal: int
    last_probe_at: datetime | None
    status: str                       # idle | syncing | error
    file_count: int                   # logical files in the latest tree


class BranchList(BaseModel):
    items: list[BranchInfo]
    count: int


class VersionInfo(BaseModel):
    branch: str
    ordinal: int
    version_tag: str
    captured_at: datetime
    completed_at: datetime | None
    files_added: int
    files_modified: int
    files_removed: int
    bytes_added: int


class VersionList(BaseModel):
    items: list[VersionInfo]
    count: int                        # returned this page
    total: int                        # all complete versions for the branch


class ChangeEntry(BaseModel):
    path: str                         # logical path
    type: str                         # added | modified | removed
    content_sha256: str | None = None  # null for removed
    size: int                         # 0 for removed


class ChangeList(BaseModel):
    branch: str
    ordinal: int
    version_tag: str
    entries: list[ChangeEntry]
    count: int                        # returned this page
    total: int                        # all changes for this version (after the type filter)
    files_added: int                  # version-level totals (unfiltered)
    files_modified: int
    files_removed: int


class TreeEntry(BaseModel):
    name: str                         # immediate child name
    path: str                         # full path (dirs end with "/")
    is_dir: bool
    file_count: int                   # files under it (1 for a file)
    size: int                         # total bytes under it


class TreeListing(BaseModel):
    branch: str
    prefix: str
    entries: list[TreeEntry]
    count: int


class FileMeta(BaseModel):
    branch: str
    path: str
    content_sha256: str
    size: int
    archive: str | None = None        # source TFI directory (null for loose files)
    archive_index: int | None = None


# ── File history + compare ────────────────────────────────────────────────
# History is "every UpdateChange touching this (branch, path)" with the
# version's tag + capture time joined in. Compare resolves two versions of
# the SAME path to two blob shas, then either runs a unified-diff (text) or
# reports the size/sha mismatch (binary).

class FileHistoryEntry(BaseModel):
    ordinal: int
    version_tag: str
    captured_at: datetime | None
    type: str                         # added | modified | removed
    content_sha256: str | None = None
    size: int


class FileHistoryList(BaseModel):
    branch: str
    path: str
    items: list[FileHistoryEntry]
    count: int


class FileVersionInfo(BaseModel):
    """One side of a compare result - identifies which version and blob."""
    ordinal: int
    version_tag: str
    captured_at: datetime | None
    content_sha256: str | None = None
    size: int


class DiffHunkLine(BaseModel):
    """One line inside a unified-diff hunk."""
    kind: str                         # equal | add | remove
    left: int | None = None           # 1-based line number in the "from" side
    right: int | None = None          # 1-based line number in the "to" side
    text: str


class DiffHunk(BaseModel):
    """One contiguous block of changed lines plus its surrounding context."""
    left_start: int                   # 1-based starting line in "from"
    right_start: int                  # 1-based starting line in "to"
    lines: list[DiffHunkLine]


class FileCompareResponse(BaseModel):
    """Note: the "from" version is exposed under that JSON key via alias -
    Python's ``from`` is a reserved word, so the attribute is named
    ``from_`` while ``populate_by_name`` lets the constructor still accept
    that.
    """
    branch: str
    path: str
    from_: FileVersionInfo = Field(alias="from")
    to: FileVersionInfo
    identical: bool                   # shas match → no diff payload
    is_text: bool                     # both sides decoded as text
    reason: str | None = None         # why we didn't diff (e.g. "binary", "too large")
    hunks: list[DiffHunk] = []        # populated only when is_text=True and not identical

    model_config = ConfigDict(populate_by_name=True)
