"""Read + cache JSON data files under ``site/static`` for server-side rendering.

Several showcase pages render from a static JSON file that the browser also
fetches (commands, stat tables, star chart, …). To render that content
server-side we need the same data in the request handler - this loads it once
and caches it, re-reading only when the file's mtime changes so a redeploy that
ships new data is picked up without a process restart.
"""
import json
from pathlib import Path
from threading import Lock
from typing import Any

from app.core.config import settings

_STATIC_ROOT = Path(settings.site_root) / "static"
_CACHE: dict[str, tuple[float, Any]] = {}
_LOCK = Lock()


def load_static_json(relpath: str) -> Any:
    """Load + parse ``site/static/<relpath>``, cached by file mtime.

    Raises the usual ``FileNotFoundError`` / ``json.JSONDecodeError`` if the file
    is missing or malformed - callers server-rendering from it should let that
    surface rather than paper over a broken data file.
    """
    path = _STATIC_ROOT / relpath
    mtime = path.stat().st_mtime
    cached = _CACHE.get(relpath)
    if cached is not None and cached[0] == mtime:
        return cached[1]
    with _LOCK:
        cached = _CACHE.get(relpath)
        if cached is not None and cached[0] == mtime:
            return cached[1]
        with path.open(encoding="utf-8") as fh:
            data = json.load(fh)
        _CACHE[relpath] = (mtime, data)
    return data
