"""Trove news relay: fetch the official RSS feed, upsert into Mongo, serve from DB.

A background task periodically pulls ``trovegame.com/feed`` and stores the parsed
items in the ``trove_news`` collection so the API can serve them without each
client hitting the upstream feed. The parser is split from the fetch so it can be
unit-tested with a fixed XML string (no network).
"""

import logging
import re
from datetime import timezone
from email.utils import parsedate_to_datetime
from html import unescape
from xml.etree import ElementTree as ET

from app.core.config import settings
from app.core.http import fetch_text
from app.core.pagination import paginate
from app.core.refresher import PeriodicRefresher
from app.core.utils import utcnow
from app.trove.models import TroveNews

logger = logging.getLogger("kiwi.trove.news")
UTC = timezone.utc

# XML namespace prefixes used by the Trove RSS feed. The feed mixes plain RSS tags
# (<title>, <link>) with extension tags that live under a namespace URI:
#   dc:creator                      -> Dublin Core: the author
#   media:content / media:thumbnail -> Media RSS: the article image
# ElementTree needs this prefix->URI map to resolve those namespaced tags, e.g.
# item.find("media:content", _RSS_NS).
_RSS_NS = {
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
    # fetch_text follows redirects: trovegame.com/feed 301s to /feed/, and without
    # the follow we'd parse the redirect page instead of the RSS and relay nothing.
    items = parse_feed(await fetch_text(settings.trove_news_feed_url))

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

    return len(items)


# --- Read helpers -----------------------------------------------------------
# Nothing is pruned - `trove_news` is the durable archive. The live feed serves
# the newest few; the history endpoint pages through everything.

async def latest_news(limit: int) -> list[TroveNews]:
    """The newest ``limit`` articles (the small, live view), de-duplicated by
    title. The official feed lists the same post once per category (sometimes
    under different URLs), so a raw query shows the same article several times;
    we keep the newest copy and merge the others' categories into it (so the
    Shop-Offers filter still sees every category the post was filed under)."""
    docs = await TroveNews.find().sort("-published_at").limit(max(limit * 3, limit)).to_list()
    out: list[TroveNews] = []
    seen: dict[str, TroveNews] = {}
    for d in docs:
        key = (d.title or "").strip().lower() or d.url
        kept = seen.get(key)
        if kept is not None:
            merged = list(kept.categories or [])
            for c in (d.categories or []):
                if c not in merged:
                    merged.append(c)
            kept.categories = merged
            continue
        seen[key] = d
        out.append(d)
    return out[:limit]


async def news_history(limit: int, offset: int) -> tuple[list[TroveNews], int]:
    """A page of the full archive (newest first) + the total article count."""
    return await paginate(TroveNews.find(), sort="-published_at", limit=limit, offset=offset)


# --- Background refresher ---------------------------------------------------

async def _refresh_news_cycle() -> int:
    """Refresh the news feed, then fan it out to the live event channel."""
    count = await refresh_news()
    # Push to the live event channel (SSE + the bot's news announcement).
    # Dedup makes this a no-op unless the newest article changed.
    from app.events import bus
    await bus.publish_type("trove_news")
    return count


_refresher = PeriodicRefresher(
    _refresh_news_cycle,
    name="Trove news refresh",
    delay=lambda: settings.trove_news_refresh_seconds,
    log_result=lambda count: f"{count} item(s)",
)


def start_news_refresher() -> None:
    _refresher.start()


async def stop_news_refresher() -> None:
    await _refresher.stop()
