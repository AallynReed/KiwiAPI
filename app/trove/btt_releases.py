"""BetterTroveTools releases relay (drives the desktop app's update checks).

Polls the GitHub releases API for the configured repo and stores releases in
Mongo (``BttRelease``). Two channels — ``release`` (``prerelease=False``) and
``beta`` (``prerelease=True``) — and three platforms detected by asset extension:

  * ``windows``  → ``.msi`` (preferred) | ``.exe``
  * ``linux``    → ``.AppImage`` | ``.deb`` | ``.rpm`` | ``.tar.gz``
  * ``android``  → ``.apk``

The "latest per platform" walks the channel's releases newest-first and returns
the first release that actually carries an asset for that platform. So if the
absolute latest release shipped no Windows build, ``/btt/latest`` still surfaces
the most recent release that did — exactly the "next candidate" rule the desktop
app needs to drive update prompts.
"""

import asyncio
import logging
import re
from datetime import datetime

import httpx

from app.core.config import settings
from app.core.utils import utcnow
from app.trove.models import BttRelease

logger = logging.getLogger("kiwi.trove.btt_releases")

# Channel name -> the GitHub `prerelease` flag value.
CHANNELS: dict[str, bool] = {"release": False, "beta": True}

# (platform, extensions-in-priority-order). The first match wins as the "primary"
# asset for that platform; secondary matches are still returned in the response.
PLATFORM_EXTENSIONS: dict[str, tuple[str, ...]] = {
    "windows": (".msi", ".exe"),
    "linux": (".appimage", ".deb", ".rpm", ".tar.gz"),
    "android": (".apk",),
}
PLATFORMS: tuple[str, ...] = tuple(PLATFORM_EXTENSIONS)

_GITHUB = "https://api.github.com"


# --- Platform detection + walk-back (pure) ----------------------------------

def _asset_priority(name: str, platform: str) -> int | None:
    """Index of the matching extension for `platform` (lower = higher priority),
    or None if the file isn't an artifact for that platform."""
    lowered = name.lower()
    for i, ext in enumerate(PLATFORM_EXTENSIONS[platform]):
        if lowered.endswith(ext):
            return i
    return None


def assets_for_platform(assets: list[dict], platform: str) -> list[dict]:
    """The subset of `assets` matching `platform`, sorted by extension priority."""
    matched = []
    for a in assets:
        p = _asset_priority(a.get("name", ""), platform)
        if p is not None:
            matched.append((p, a))
    matched.sort(key=lambda kv: (kv[0], kv[1].get("name", "")))
    return [a for _p, a in matched]


def walk_latest(releases, platform: str):
    """The first release in `releases` (assumed newest-first) carrying any asset
    for `platform`. Returns ``(release, matched_assets)`` or ``None``."""
    for r in releases:
        assets = r.assets if hasattr(r, "assets") else r["assets"]
        matched = assets_for_platform(assets, platform)
        if matched:
            return r, matched
    return None


# --- Version comparison (for the /check endpoint) ---------------------------
# Tolerant semver-ish parser: accepts "v1.2.3", "1.2.3", "1.2.3-beta.1". Strips
# the leading "v"; numeric parts compared as tuples (zero-padded to the same
# length so "1.0.0" == "1.0"); same numbers, *no* suffix is GREATER than a
# suffix (release > prerelease of the same version). Falls back to None if
# either side can't be parsed.

_VERSION_RE = re.compile(r"^v?(\d+(?:\.\d+)*)(.*)$")


def parse_version(value: str) -> tuple[tuple[int, ...], str] | None:
    """`'v1.2.3-beta.1'` -> `((1, 2, 3), '-beta.1')`, or None if unparseable."""
    if not value:
        return None
    m = _VERSION_RE.match(value.strip())
    if not m:
        return None
    try:
        nums = tuple(int(x) for x in m.group(1).split("."))
    except ValueError:
        return None
    return nums, m.group(2)


def compare_versions(a: str, b: str) -> int | None:
    """-1 if a<b, 0 if equal, 1 if a>b; None if either side isn't parseable."""
    pa, pb = parse_version(a), parse_version(b)
    if pa is None or pb is None:
        return None
    na, sa = pa
    nb, sb = pb
    # Pad to the same length so trailing zeros don't change ordering.
    width = max(len(na), len(nb))
    na = na + (0,) * (width - len(na))
    nb = nb + (0,) * (width - len(nb))
    if na != nb:
        return -1 if na < nb else 1
    # Same numeric core: a release ("") outranks a prerelease (suffix present).
    if sa == sb:
        return 0
    if not sa:
        return 1   # "1.0.0" > "1.0.0-beta"
    if not sb:
        return -1
    return -1 if sa < sb else 1


# --- GitHub fetch + normalization -------------------------------------------

def _parse_dt(value: str | None) -> datetime | None:
    if not value:
        return None
    try:
        return datetime.fromisoformat(value.replace("Z", "+00:00"))
    except ValueError:
        return None


def normalize_release(raw: dict) -> dict | None:
    """Pick the fields we keep from a GitHub releases payload entry. None on
    drafts or missing required fields (id / tag / url / published_at)."""
    if not isinstance(raw, dict) or raw.get("draft"):
        return None
    release_id, tag, html = raw.get("id"), raw.get("tag_name"), raw.get("html_url")
    pub = _parse_dt(raw.get("published_at"))
    if not (isinstance(release_id, int) and tag and html and pub):
        return None
    assets: list[dict] = []
    for a in raw.get("assets") or []:
        if not isinstance(a, dict) or not a.get("name") or not a.get("browser_download_url"):
            continue
        assets.append({
            "name": a["name"], "url": a["browser_download_url"],
            "size": int(a.get("size") or 0), "content_type": a.get("content_type"),
            "download_count": int(a.get("download_count") or 0),
        })
    return {
        "release_id": release_id, "tag_name": tag,
        "name": (raw.get("name") or "").strip(), "body": raw.get("body") or "",
        "html_url": html, "prerelease": bool(raw.get("prerelease")),
        "published_at": pub, "assets": assets,
    }


async def fetch_releases() -> list[dict]:
    """The newest 100 releases (raw GitHub payload) for the configured repo."""
    headers = {
        "Accept": "application/vnd.github+json",
        "X-GitHub-Api-Version": "2022-11-28",
        "User-Agent": "KiwiAPI/1.0",
    }
    if settings.btt_releases_token:
        headers["Authorization"] = f"Bearer {settings.btt_releases_token}"
    url = f"{_GITHUB}/repos/{settings.btt_releases_repo}/releases?per_page=100"
    async with httpx.AsyncClient(timeout=15, follow_redirects=True, headers=headers) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp.json()


async def refresh_releases() -> int:
    """Pull GitHub + upsert by `release_id`. Returns the count stored this cycle."""
    raw_list = await fetch_releases()
    stored = 0
    for raw in raw_list:
        norm = normalize_release(raw)
        if norm is None:
            continue
        existing = await BttRelease.find_one(BttRelease.release_id == norm["release_id"])
        now = utcnow()
        if existing is None:
            await BttRelease(**norm, fetched_at=now).insert()
        else:
            for key, value in norm.items():
                setattr(existing, key, value)
            existing.fetched_at = now
            await existing.save()
        stored += 1
    return stored


# --- Read helpers -----------------------------------------------------------

async def list_releases(
    channel: str | None, limit: int, offset: int
) -> tuple[list[BttRelease], int]:
    query: dict = {}
    if channel in CHANNELS:
        query["prerelease"] = CHANNELS[channel]
    total = await BttRelease.find(query).count()
    docs = await BttRelease.find(query).sort("-published_at").skip(offset).limit(limit).to_list()
    return docs, total


async def latest_per_platform(channel: str) -> dict:
    """{platform: (release_doc, matched_assets) | None} for the channel.

    Each platform walks back independently: a release with no Windows build
    doesn't suppress Windows updates, the platform just finds the previous
    release that did ship a Windows artifact."""
    docs = await BttRelease.find(
        BttRelease.prerelease == CHANNELS[channel]
    ).sort("-published_at").to_list()
    return {p: walk_latest(docs, p) for p in PLATFORMS}


# --- Background refresher ---------------------------------------------------

_task: asyncio.Task | None = None


async def _loop() -> None:
    while True:
        try:
            count = await refresh_releases()
            logger.info("BTT releases refreshed: %d release(s)", count)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("BTT releases refresh failed", exc_info=True)
        try:
            await asyncio.sleep(settings.btt_releases_refresh_seconds)
        except asyncio.CancelledError:
            raise


def start_btt_releases_refresher() -> None:
    global _task
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_btt_releases_refresher() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None
