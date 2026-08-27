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
    last_ordinal: int = 0             # newest version touched under this entry (0 = unknown)
    last_modified_at: datetime | None = None  # captured_at of that version, if resolvable


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
    removed: bool = False             # true when the file is gone from the tree and
                                      #   this is its last version before removal
    removed_ordinal: int | None = None  # the version that removed it


class FileView(BaseModel):
    """In-browser preview of one file. ``text`` is the UTF-8 content when the file
    is small + text-like; otherwise ``viewable`` is false and ``kind`` tells the
    client how to render it ("text" / "image" / "binary" / "too_large" / "missing").
    ``reason`` mirrors ``kind`` for the non-text cases (kept for older clients)."""
    branch: str
    path: str
    size: int
    content_sha256: str
    viewable: bool
    kind: str | None = None
    reason: str | None = None
    truncated: bool = False
    text: str | None = None
    removed: bool = False             # this is the last version before removal
    removed_ordinal: int | None = None


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
    ordinal: int
    version_tag: str
    captured_at: datetime | None
    content_sha256: str | None = None
    size: int


class DiffHunkLine(BaseModel):
    kind: str                         # equal | add | remove
    left: int | None = None           # 1-based line number in the "from" side
    right: int | None = None          # 1-based line number in the "to" side
    text: str


class DiffHunk(BaseModel):
    left_start: int                   # 1-based starting line in "from"
    right_start: int                  # 1-based starting line in "to"
    lines: list[DiffHunkLine]


class FileCompareResponse(BaseModel):
    """``from`` is a Python keyword, so the attribute is ``from_`` exposed under
    the JSON key ``from`` via alias; ``populate_by_name`` keeps the ctor accepting
    the attribute name too."""
    branch: str
    path: str
    from_: FileVersionInfo = Field(alias="from")
    to: FileVersionInfo
    identical: bool                   # shas match → no diff payload
    is_text: bool                     # both sides decoded as text
    reason: str | None = None         # why we didn't diff (e.g. "binary", "too large")
    hunks: list[DiffHunk] = []        # populated only when is_text=True and not identical

    model_config = ConfigDict(populate_by_name=True)
