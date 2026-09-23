"""Pictures and 3D previews for the wiki's ally and mount pages.

Both come from what the codexes already resolved: each codex entry carries the
blueprint that draws it (the `_ui` whole-model for multi-part creatures), and the
API renders that as a PNG (``/site/codexes/render``). A mount or dragon is also a
rigged creature, so its render and its preview are the assembled animal
(``prefab=``); an ally is a single model, previewed from its blueprint
(``game=``). The wiki holds no database, so the blueprints are read over the
internal API once and cached.
"""
from __future__ import annotations

import time
from urllib.parse import quote, urlencode

from app.core.config import settings
from app.core.internal_api import internal_get

# Codex types that hold our ally/mount entries, and which are rigged creatures.
CODEX_TYPES = {"ally": ("ally",), "mount": ("mount", "dragon")}
RIGGED = frozenset({"mount", "dragon"})
_PAGE = 200
_TTL = 3600

_cache: dict[str, tuple[float, dict[str, tuple[str, str]]]] = {}


def codex_path(entry: dict) -> str:
    """``collections/pet/x`` -> the codex's ``prefabs/collections/pet/x.binfab``."""
    return f"prefabs/{entry['prefab']}.binfab"


async def blueprints(kind: str) -> dict[str, tuple[str, str]]:
    """``codex path -> (blueprint, codex type)`` for every entry of ``kind``."""
    hit = _cache.get(kind)
    if hit and time.monotonic() - hit[0] < _TTL:
        return hit[1]
    out: dict[str, tuple[str, str]] = {}
    for ctype in CODEX_TYPES[kind]:
        offset = 0
        while True:
            page = await internal_get("/site/codexes/search", {"type": ctype, "limit": _PAGE, "offset": offset},
                                      timeout=5.0) or {}
            items = page.get("items") or []
            for row in items:
                if row.get("path") and row.get("blueprint"):
                    out[row["path"]] = (row["blueprint"], ctype)
            offset += len(items)
            if not items or offset >= (page.get("total") or 0):
                break
    if out or not hit:
        _cache[kind] = (time.monotonic(), out)
    return out if out else (hit[1] if hit else {})


def thumb_url(entry: dict, found: tuple[str, str] | None, dim: int) -> str:
    if not found:
        return ""
    blueprint, ctype = found
    params = {"blueprint": blueprint, "dim": dim}
    if ctype in RIGGED:
        params["prefab"] = codex_path(entry)
    return f"{settings.api_url.rstrip('/')}/site/codexes/render?{urlencode(params)}"


def preview_url(entry: dict, found: tuple[str, str] | None) -> str:
    if not found:
        return ""
    blueprint, ctype = found
    base = f"{settings.app_url.rstrip('/')}/embed/viewer"
    if ctype in RIGGED:
        return f"{base}?prefab={quote(codex_path(entry))}"
    return f"{base}?game={quote(blueprint)}"
