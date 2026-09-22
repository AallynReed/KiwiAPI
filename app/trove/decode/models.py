"""One row per decoder run, for the dev panel's Game data tab."""
from __future__ import annotations

from datetime import datetime

from beanie import Document
from pymongo import ASCENDING, DESCENDING, IndexModel


class DecoderRun(Document):
    name: str                      # registry key, e.g. "class_levels"
    trigger: str                   # "patch" | "manual"
    ordinal: int                   # archive version it read (UpdateBranch.current_ordinal)
    version_tag: str | None = None
    code_version: str
    started_at: datetime
    finished_at: datetime | None = None
    ok: bool | None = None         # None while running
    entries: int | None = None     # entries written
    error: str | None = None
    traceback: str | None = None

    class Settings:
        name = "decoder_runs"
        indexes = [
            IndexModel([("name", ASCENDING), ("started_at", DESCENDING)]),
            IndexModel([("started_at", ASCENDING)], expireAfterSeconds=90 * 86400),
        ]
