"""Web-tier feature-flag gating (no DB).

The website container can't read ``runtime_config`` in-process the way the API
does, so it fetches the resolved flag map from the API's
``GET /site/feature-flags`` (memoised ~5s) and uses it to (a) 404 a disabled
feature's page - mirroring the API's ``_resolve_feature_flags`` - and (b) drive
the navbar's ``{% if <feature>_enabled %}`` conditionals via a Jinja context
processor.

Fails OPEN (all-enabled) when the API is briefly unreachable: a transient blip
must never blank the navbar or 404 every gated page. The API independently gates
its own ``/site/*`` proxies, so a stale-open web flag can't actually expose a
disabled feature's data - the proxy still 404s.
"""
import time

from fastapi import HTTPException, Request

from app.core.internal_api import internal_get
from app.site.feature_map import SITE_FEATURE_FLAGS, feature_blocks

_TTL = 5.0
# Calc switches the /leaderboards page needs on top of the master page toggles;
# the API returns them in the same payload. Stashed on request.state for the page.
_EXTRA_FLAGS = ("cheater_detection_enabled", "alt_clusters_enabled", "renames_enabled")
_ALL_ATTRS = tuple(SITE_FEATURE_FLAGS) + _EXTRA_FLAGS

# Fail-open default: every feature enabled.
_DEFAULT = dict.fromkeys(_ALL_ATTRS, True)

_cache: dict = {"at": -1e9, "flags": None}


async def _fetch() -> dict:
    """Resolved flag map from the API, memoised ~5s. All-True on any failure."""
    now = time.monotonic()
    cached = _cache["flags"]
    if cached is not None and now - float(_cache["at"]) <= _TTL:
        return cached
    data = await internal_get("/site/feature-flags")
    flags = {**_DEFAULT, **data} if isinstance(data, dict) else dict(_DEFAULT)
    _cache["flags"] = flags
    _cache["at"] = now
    return flags


async def resolve(request: Request) -> None:
    """Per-request page gate (mirrors the API's ``_resolve_feature_flags``): stash
    every flag on ``request.state`` for the template context + the leaderboards
    page, and 404 the pages of any disabled feature."""
    flags = await _fetch()
    for attr, value in flags.items():
        setattr(request.state, attr, value)
    if feature_blocks(request.url.path, flags):
        raise HTTPException(status_code=404)


def context(request: Request) -> dict:
    """Inject the feature flags into every template (navbar + dashboard read them);
    default to enabled if a flag wasn't resolved."""
    return {attr: getattr(request.state, attr, True) for attr in SITE_FEATURE_FLAGS}
