"""Trove news relay: fetch the official RSS feed, upsert into Mongo, serve from DB.

A background task periodically pulls ``trovegame.com/feed`` and stores the parsed
items in the ``trove_news`` collection so the API can serve them without each
client hitting the upstream feed. The parser is split from the fetch so it can be
unit-tested with a fixed XML string (no network).
"""

import asyncio
import logging
import re
from datetime import timezone
from email.utils import parsedate_to_datetime
from html import unescape
from xml.etree import ElementTree as ET

import httpx

from app.core.config import settings
from app.core.utils import utcnow
from app.trove.models import TroveNews

logger = logging.getLogger("kiwi.trove.news")
UTC = timezone.utc

_RSS_NS = {
    "content": "http://purl.org/rss/1.0/modules/content/",
    "dc": "http://purl.org/dc/elements/1.1/",
    "media": "http://search.yahoo.com/mrss/",
}
_MAX_ITEMS = 30


def _strip_html(value: str) -> str:
    if not value:
        return ""
    stripped = re.sub(r"<br\s*/?>", " ", value, flags=re.IGNORECASE)
    stripped = re.sub(r"<[^>]+>", " ", stripped)
    return " ".join(unescape(stripped).split()).strip()


def _truncate(value: str, limit: int = 220) -> str:
    if len(value) <= limit:
        return value
    return value[:limit].rsplit(" ", 1)[0].strip() + "..."


def parse_feed(xml_text: str) -> list[dict]:
    """Parse the Trove RSS feed XML into normalized news dicts. Pure / no network."""
    root = ET.fromstring(xml_text)
    items: list[dict] = []
    for item in root.findall("./channel/item"):
        link = (item.findtext("link") or "").strip()
        if not link:
            continue
        title = unescape((item.findtext("title") or "").strip())
        author = (item.findtext("dc:creator", "", _RSS_NS) or "").strip()
        description = item.findtext("description") or ""
        categories = [
            unescape((c.text or "").strip())
            for c in item.findall("category")
            if (c.text or "").strip()
        ]

        published_at = None
        try:
            published_at = parsedate_to_datetime((item.findtext("pubDate") or "").strip())
            published_at = published_at.astimezone(UTC)
        except (TypeError, ValueError):
            published_at = None

        image = None
        for tag in ("media:content", "media:thumbnail"):
            el = item.find(tag, _RSS_NS)
            if el is not None and el.attrib.get("url"):
                image = el.attrib["url"]
                break

        items.append({
            "url": link,
            "title": title,
            "author": author or "Team Trove",
            "summary": _truncate(_strip_html(unescape(description))),
            "category": categories[0] if categories else "News",
            "categories": categories,
            "image": image,
            "published_at": published_at,
        })
        if len(items) >= _MAX_ITEMS:
            break
    return items


async def refresh_news() -> int:
    """Fetch the feed and upsert items into Mongo (by url). Returns items processed."""
    async with httpx.AsyncClient(timeout=15, headers={"User-Agent": "KiwiAPI/1.0"}) as client:
        resp = await client.get(settings.trove_news_feed_url)
        resp.raise_for_status()
        items = parse_feed(resp.text)

    for it in items:
        existing = await TroveNews.find_one(TroveNews.url == it["url"])
        if existing is None:
            await TroveNews(**it).insert()
            continue
        existing.title = it["title"]
        existing.author = it["author"]
        existing.summary = it["summary"]
        existing.category = it["category"]
        existing.categories = it["categories"]
        existing.image = it["image"]
        existing.published_at = it["published_at"]
        existing.updated_at = utcnow()
        await existing.save()

    await _prune()
    return len(items)


async def _prune() -> None:
    """Keep only the newest `trove_news_keep` articles."""
    keep = settings.trove_news_keep
    newest = await TroveNews.find().sort("-published_at").limit(keep).to_list()
    keep_ids = {n.id for n in newest}
    if keep_ids:
        await TroveNews.find({"_id": {"$nin": list(keep_ids)}}).delete()


# --- Background refresher ---------------------------------------------------

_task: asyncio.Task | None = None


async def _loop() -> None:
    while True:
        try:
            count = await refresh_news()
            logger.info("Trove news refreshed: %d item(s)", count)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("Trove news refresh failed", exc_info=True)
        try:
            await asyncio.sleep(settings.trove_news_refresh_seconds)
        except asyncio.CancelledError:
            raise


def start_news_refresher() -> None:
    global _task
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_news_refresher() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None
