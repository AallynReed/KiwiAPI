"""Content-addressed blob store on the filesystem.

Blobs are keyed by SHA-256 of their bytes and stored raw (no compression — game
content is mostly already compressed, and reads stay fast) at
``<root>/objects/<sha[:2]>/<sha>``. Writes are atomic (temp + fsync + rename) and
idempotent, so identical content — including across the Live and PTS timelines —
collapses to one copy. Sync I/O; call via ``asyncio.to_thread`` from the pipeline.
"""

from __future__ import annotations

import hashlib
import os
import tempfile
from pathlib import Path


class ContentStore:
    def __init__(self, root: str | os.PathLike):
        self.root = Path(root)
        self.objects = self.root / "objects"

    def path_for(self, sha: str) -> Path:
        return self.objects / sha[:2] / sha

    def has(self, sha: str) -> bool:
        return self.path_for(sha).is_file()

    def put(self, data: bytes) -> tuple[str, bool]:
        """Store bytes, return (sha256_hex, created). `created` is False if already present."""
        sha = hashlib.sha256(data).hexdigest()
        dest = self.path_for(sha)
        if dest.is_file():
            return sha, False
        dest.parent.mkdir(parents=True, exist_ok=True)
        fd, tmp = tempfile.mkstemp(dir=dest.parent, suffix=".part")
        try:
            with os.fdopen(fd, "wb") as f:
                f.write(data)
                f.flush()
                os.fsync(f.fileno())
            os.replace(tmp, dest)  # atomic
        except BaseException:
            try:
                os.unlink(tmp)
            except OSError:
                pass
            raise
        return sha, True

    def get(self, sha: str) -> bytes | None:
        p = self.path_for(sha)
        return p.read_bytes() if p.is_file() else None
