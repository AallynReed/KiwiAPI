"""Relayed community feeds: Twitch streams, YouTube and Bilibili videos.

The trovesaurus bot already fetches these (Twitch OAuth, the YouTube Data API,
and a Bilibili scraper) and exposes the results. We relay + cache those rather
than re-implementing the credentials/scraping here: a background task pulls each
feed into Mongo (one ``FeedCache`` doc per feed) and the API serves from it.
"""

import asyncio
import logging
from datetime import datetime

import httpx

from app.core.config import settings
from app.core.utils import utcnow
from app.trove.models import FeedCache

logger = logging.getLogger("kiwi.trove.relays")


def _normalize_twitch(raw) -> list[dict]:
    streams = raw if isinstance(raw, list) else (raw or {}).get("data", [])
    out = []
    for s in streams:
        if not isinstance(s, dict):
            continue
        login = s.get("user_login") or s.get("user_name") or ""
        thumb = (s.get("thumbnail_url") or "").replace("{width}", "440").replace("{height}", "248")
        out.append({
            "channel": s.get("user_name") or login,
            "login": login,
            "url": f"https://twitch.tv/{login}" if login else "https://twitch.tv",
            "title": s.get("title") or "",
            "viewers": int(s.get("viewer_count") or 0),
            "game": s.get("game_name"),
            "started_at": s.get("started_at"),
            "thumbnail": thumb or None,
        })
    return out


def _normalize_videos(raw) -> list[dict]:
    videos = raw if isinstance(raw, list) else []
    out = []
    for v in videos:
        if not isinstance(v, dict) or not v.get("url"):
            continue
        out.append({
            "title": v.get("title") or "",
            "url": v.get("url") or "",
            "channel": v.get("channel") or "",
            "video_id": v.get("video_id"),
            "published_at": v.get("published_at"),
            "thumbnail_url": v.get("thumbnail_url"),
        })
    return out


# feed -> (upstream path on the trovesaurus bot, normalizer)
_FEEDS = {
    "twitch": ("/twitch_streams", _normalize_twitch),
    "youtube": ("/youtube_videos", _normalize_videos),
    "bilibili": ("/bilibili_videos", _normalize_videos),
}


async def refresh_feed(feed: str) -> int:
    path, normalize = _FEEDS[feed]
    async with httpx.AsyncClient(timeout=15, headers={"User-Agent": "KiwiAPI/1.0"}) as client:
        resp = await client.get(settings.trovesaurus_base_url + path)
        resp.raise_for_status()
        items = normalize(resp.json())

    existing = await FeedCache.find_one(FeedCache.feed == feed)
    if existing is None:
        await FeedCache(feed=feed, items=items, fetched_at=utcnow()).insert()
    else:
        existing.items = items
        existing.fetched_at = utcnow()
        await existing.save()
    return len(items)


async def refresh_all_feeds() -> None:
    for feed in _FEEDS:
        try:
            count = await refresh_feed(feed)
            logger.info("Relayed feed %s: %d item(s)", feed, count)
        except Exception:
            logger.warning("Feed relay '%s' failed", feed, exc_info=True)


async def get_feed(feed: str) -> tuple[list[dict], datetime | None]:
    doc = await FeedCache.find_one(FeedCache.feed == feed)
    return (doc.items, doc.fetched_at) if doc else ([], None)


# --- Background refresher ---------------------------------------------------

_task: asyncio.Task | None = None


async def _loop() -> None:
    while True:
        try:
            await refresh_all_feeds()
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("Feed relay iteration failed", exc_info=True)
        try:
            await asyncio.sleep(settings.trove_feeds_refresh_seconds)
        except asyncio.CancelledError:
            raise


def start_feeds_refresher() -> None:
    global _task
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_feeds_refresher() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None
