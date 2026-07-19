"""Website changelog relay (drives the public ``/changelog`` transparency page).

Relays the commit history of the website's own source repo
(``settings.site_source_repo`` = ``AallynReed/KiwiAPI``) from GitHub, grouped by
tag using the same builder as the BetterTroveTools app changelog
(``btt_releases.build_changelog_groups``).

Unlike the BTT relay (a Mongo-backed background refresher), this is an on-demand
read cached in **Redis** with a short TTL. Redis is shared across all API workers,
so the ~2 GitHub calls per TTL window are made once for the whole fleet - staying
comfortably under GitHub's 60/hr unauthenticated limit even without a token
(``btt_releases_token`` lifts it to 5000/hr when set).
"""
import json
import logging

import httpx

from app.core.config import settings
from app.core.redis import get_redis
from app.trove.btt_releases import build_changelog_groups

logger = logging.getLogger("kiwi.site.changelog")

_GITHUB = "https://api.github.com"
_CACHE_KEY = "site:changelog"
_TTL_SECONDS = 900   # 15 min - the page is not time-critical


def repo_url() -> str:
    return f"https://github.com/{settings.site_source_repo}"


def _gh_headers() -> dict[str, str]:
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "KiwiAPI/1.0",
    }
    if settings.btt_releases_token:
        headers["Authorization"] = f"Bearer {settings.btt_releases_token}"
    return headers


async def _gh_get(path: str):
    url = f"{_GITHUB}/repos/{settings.site_source_repo}/{path}"
    async with httpx.AsyncClient(timeout=15, follow_redirects=True, headers=_gh_headers()) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp.json()


async def _build() -> dict:
    """Fetch tags + commits from GitHub and build the grouped changelog."""
    try:
        tags = await _gh_get("tags?per_page=100")
    except Exception:
        logger.warning("site changelog: tags fetch failed", exc_info=True)
        tags = []
    rate_limited = False
    try:
        commits: object = await _gh_get("commits?per_page=100")
    except Exception:
        logger.warning("site changelog: commits fetch failed", exc_info=True)
        commits = []
    # GitHub returns the rate-limit body as a dict on the commits call.
    if isinstance(commits, dict):
        rate_limited = "rate limit" in str(commits.get("message", "")).lower()
        commits = []
    groups = build_changelog_groups(
        tags if isinstance(tags, list) else [],
        commits if isinstance(commits, list) else [],
    )
    return {
        "repo": settings.site_source_repo,
        "repo_url": repo_url(),
        "groups": groups,
        "rate_limited": rate_limited,
    }


async def get_changelog() -> dict:
    """The grouped website changelog, served from a shared Redis cache (built on
    miss). Never raises - a GitHub/Redis hiccup degrades to an empty payload the
    page renders as 'no changelog yet'."""
    redis = get_redis()
    if redis is not None:
        try:
            cached = await redis.get(_CACHE_KEY)
            if cached:
                return json.loads(cached)
        except Exception:
            logger.warning("site changelog: Redis read failed", exc_info=True)

    data = await _build()
    # Don't cache an empty rate-limited miss - retry next request so a transient
    # 403 doesn't pin an empty page for the whole TTL.
    if redis is not None and not (data["rate_limited"] and not data["groups"]):
        try:
            await redis.set(_CACHE_KEY, json.dumps(data), ex=_TTL_SECONDS)
        except Exception:
            logger.warning("site changelog: Redis write failed", exc_info=True)
    return data
