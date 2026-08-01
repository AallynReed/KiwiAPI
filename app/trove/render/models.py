"""Mongo models for the render package.

Only the blueprint payload cache lives here - the rasterizer itself is stateless
and the PNG cache is Redis-only (``service.py``). See ``bp_cache.py`` for how the
index rows below pair with the content-addressed payload blobs.
"""

from datetime import datetime

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class BlueprintCacheEntry(Document):
    """One decoded ``.blueprint`` payload, remembered so it's never decoded twice.

    ``key`` is built from things the caller knows *without* doing the expensive
    work (the container's content hash + the path inside it), so a lookup costs
    one indexed find. The payload itself is not stored here - ``blob_sha`` points
    at the gzipped JSON body in the content-addressed store, which dedupes the
    same model across every release that ships it.

    A row with ``err_status`` set is a remembered *failure* (empty placeholder,
    over the voxel cap, undecodable): the answer is deterministic, so a
    pathological file is rejected instantly instead of re-decoded on every hit.
    """

    key: str
    blob_sha: str | None = None                 # CAS sha of the gzipped JSON body
    byte_len: int = 0                           # compressed size, for admin/pruning
    voxel_count: int = 0                        # voxels in the model (`count` shadows Document.count)
    size: list[int] | None = None               # bounding box [x, y, z]

    err_status: int | None = None               # 413 / 422 for a remembered failure
    err_code: str | None = None
    err_msg: str | None = None

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "blueprint_cache"
        indexes = [
            IndexModel([("key", ASCENDING)], unique=True),
            IndexModel([("created_at", ASCENDING)]),   # prune oldest-first
        ]
