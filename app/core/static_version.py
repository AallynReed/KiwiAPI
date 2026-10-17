"""Content versions for the site's ``/static`` files.

Rendered pages link each existing static file as ``/static/<file>?v=<hash>``. A
request carrying the file's current hash is cached for a year: a changed file
gets a new URL, so a deploy (or an edit on the server) is still picked up at once.
Hashes are cached per file and recomputed when its mtime or size changes.
"""
from __future__ import annotations

import hashlib
import re
from pathlib import Path

from app.core.config import settings

_ROOT = (Path(settings.site_root) / "static").resolve()
# Fonts are left out: CSS asks for them unversioned, and a versioned preload of the
# same file would be a second download.
_LINK = re.compile(r'(\s(?:src|href)=")(/static/[^"?#]+?(?<!\.woff2)(?<!\.woff))(")')
_cache: dict[str, tuple[int, int, str]] = {}

FOREVER = "public, max-age=31536000, immutable"
_FONT_TTL = "public, max-age=604800"   # fonts are reached from CSS url()s, unversioned


def version(url_path: str) -> str | None:
    """Short content hash of the file behind a ``/static/...`` path, or None."""
    rel = url_path.removeprefix("/static/")
    try:
        f = (_ROOT / rel).resolve()
        if not f.is_relative_to(_ROOT):
            return None
        st = f.stat()
        hit = _cache.get(rel)
        if hit and hit[:2] == (st.st_mtime_ns, st.st_size):
            return hit[2]
        digest = hashlib.sha256(f.read_bytes()).hexdigest()[:10]
    except (OSError, ValueError):
        return None
    _cache[rel] = (st.st_mtime_ns, st.st_size, digest)
    return digest


def link(html: str) -> str:
    """Add ``?v=<hash>`` to every src/href that points at an existing static file."""
    def add(m: re.Match[str]) -> str:
        v = version(m.group(2))
        return f"{m.group(1)}{m.group(2)}?v={v}{m.group(3)}" if v else m.group(0)
    return _LINK.sub(add, html)


def cache_control(url_path: str, v: str | None) -> str:
    """Cache-Control for a static file request. Static bytes are the same for every
    visitor, so this applies with or without a session cookie."""
    if v and v == version(url_path):
        return FOREVER
    if url_path.endswith(".woff2"):
        return _FONT_TTL
    return "no-cache"
