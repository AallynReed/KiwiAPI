"""Community feeds: Twitch streams, YouTube and Bilibili videos.

These used to be relayed from the trovesaurus bot's HTTP server; they are now
fetched **at source**, porting the bot's own logic:

  - Twitch   - client-credentials app token → Helix ``/streams`` for Trove.
  - YouTube  - Data API v3 ``search``, then the bot's filters (excluded
               channels / title terms, skip live+upcoming, ≤N newest per
               channel, newest N overall).
  - Bilibili - HTML scrape of the search page (hotlink-protected thumbnails
               are served through the image proxy further down).

A background task pulls each feed into Mongo (one ``FeedCache`` doc per feed) on
a fixed cadence and the API serves from that cache, so request latency never
depends on the upstreams and a transient upstream failure leaves the last good
payload untouched (``refresh_all_feeds`` swallows per-feed errors).

Credentials come from the environment (``TWITCH_CLIENT_ID`` /
``TWITCH_CLIENT_SECRET`` / ``YT_API_KEY`` - the same names the bot used). The
per-feed filter knobs are runtime_config tunables (category ``community_feeds``)
so they tune from the admin panel without a redeploy.
"""

import logging
import re
from datetime import datetime, timedelta, timezone

import httpx

from app.admin import runtime_config
from app.core.config import settings
from app.core.database import upsert_by
from app.core.refresher import PeriodicRefresher
from app.core.utils import utcnow
from app.trove.models import FeedCache

logger = logging.getLogger("kiwi.trove.feeds")

_UA = "KiwiAPI/1.0 (+https://api.aallyn.net)"

_TWITCH_TOKEN_URL = "https://id.twitch.tv/oauth2/token"
_TWITCH_STREAMS_URL = "https://api.twitch.tv/helix/streams"
_YOUTUBE_SEARCH_URL = "https://www.googleapis.com/youtube/v3/search"
_YOUTUBE_VIDEOS_URL = "https://www.googleapis.com/youtube/v3/videos"
_BILIBILI_SEARCH_URL = "https://search.bilibili.com/all"


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


def _csv_set(raw: str) -> set[str]:
    """Lower-cased, whitespace-trimmed set from a comma-separated tunable.
    Mirrors the ``cheaters_excluded_board_uuids`` convention - runtime_config
    has no list type, so list-shaped knobs ride in a single string."""
    return {p.strip().lower() for p in (raw or "").split(",") if p.strip()}


def _has_term(text: str, term: str) -> bool:
    """Whole-word match of a lowercased ``term`` in already-lowercased ``text``
    - so "trove" matches "Trove's Summer" but not "introvert", and a multi-word
    term like "chaos chest" matches as a phrase."""
    return re.search(r"\b" + re.escape(term) + r"\b", text) is not None


_YT_DURATION_RE = re.compile(r"^PT(?:(\d+)H)?(?:(\d+)M)?(?:(\d+)S)?$")


def _iso8601_seconds(value: str | None) -> int | None:
    """Parse a YouTube ISO-8601 duration (``PT#H#M#S``) to whole seconds, or None
    when absent/unparseable. Used to drop Shorts from the community feed."""
    if not value:
        return None
    m = _YT_DURATION_RE.match(value)
    if not m:
        return None
    h, mi, se = (int(x) if x else 0 for x in m.groups())
    return h * 3600 + mi * 60 + se


# --- Twitch (client-credentials app token → Helix /streams) -----------------

_twitch_token: str | None = None  # cached app access token; re-minted on 401


async def _twitch_app_token(client: httpx.AsyncClient, *, force: bool = False) -> str:
    """Cached Twitch app access token. Minted on first use and whenever
    ``force`` is set (e.g. after a 401 - app tokens last ~60 days but expire,
    and the old bot only ever fetched one at startup, so it would silently
    serve stale-empty once that token died)."""
    global _twitch_token
    if _twitch_token and not force:
        return _twitch_token
    if not settings.twitch_client_id or not settings.twitch_client_secret:
        raise RuntimeError("TWITCH_CLIENT_ID / TWITCH_CLIENT_SECRET not configured")
    resp = await client.post(_TWITCH_TOKEN_URL, params={
        "client_id": settings.twitch_client_id,
        "client_secret": settings.twitch_client_secret,
        "grant_type": "client_credentials",
    })
    resp.raise_for_status()
    _twitch_token = resp.json()["access_token"]
    return _twitch_token


async def _fetch_twitch() -> list[dict]:
    if not settings.twitch_client_id or not settings.twitch_client_secret:
        raise RuntimeError("TWITCH_CLIENT_ID / TWITCH_CLIENT_SECRET not configured")
    async with httpx.AsyncClient(timeout=15, headers={"User-Agent": _UA}) as client:
        async def _get(tok: str) -> httpx.Response:
            return await client.get(_TWITCH_STREAMS_URL, params={
                "game_id": settings.twitch_game_id, "first": 100,
            }, headers={
                "Client-Id": settings.twitch_client_id,
                "Authorization": f"Bearer {tok}",
            })
        resp = await _get(await _twitch_app_token(client))
        if resp.status_code == 401:
            resp = await _get(await _twitch_app_token(client, force=True))
        resp.raise_for_status()
        return _normalize_twitch(resp.json())


# --- YouTube (Data API v3 search + the bot's filters) -----------------------

async def _fetch_youtube() -> list[dict]:
    """Broad relevance search, then OUR OWN relevance curation on the full video
    text - because the Data API's ``search.list`` ranking is far blunter than
    youtube.com's and lets ambiguous "Trove" noise (finance / treasure-hunting /
    the news app) through that no query/category tuning can fully suppress.

    Flow:
      1. ``search.list`` for the query - broad, NO category hard-filter (recall
         first; the filter is uploader-assigned and drops legit miscategorised
         Trove videos).
      2. ``videos.list`` (1 quota unit per 50 ids) to get the FULL description +
         tags + real uploader category that ``search.list`` snippets don't carry.
      3. Keep a video iff: it carries every ``require`` term (default "trove",
         whole-word), no ``exclude`` term, isn't live/upcoming, AND shows at
         least one Trove ``relevance`` signal term OR sits in the gaming
         category. Every term list is admin-tunable - this is the curation.
    """
    if not settings.yt_api_key:
        raise RuntimeError("YT_API_KEY not configured")

    query           = await runtime_config.get_setting("feeds_youtube_query")
    excluded_chans  = _csv_set(await runtime_config.get_setting("feeds_youtube_excluded_channels"))
    exclude_terms   = _csv_set(await runtime_config.get_setting("feeds_youtube_excluded_title_terms"))
    require_terms   = _csv_set(await runtime_config.get_setting("feeds_youtube_require_terms"))
    relevance_terms = _csv_set(await runtime_config.get_setting("feeds_youtube_relevance_terms"))
    gaming_cat      = (await runtime_config.get_setting("feeds_youtube_video_category_id") or "").strip()
    cutoff_days     = int(await runtime_config.get_setting("feeds_video_cutoff_days"))
    per_channel_max = int(await runtime_config.get_setting("feeds_per_channel_max"))
    max_items       = int(await runtime_config.get_setting("feeds_max_items"))

    published_after = (utcnow() - timedelta(days=cutoff_days)).isoformat().replace("+00:00", "Z")
    async with httpx.AsyncClient(timeout=15, headers={"User-Agent": _UA}) as client:
        # 1) Broad search - recall first; precision is done by us below.
        #    (search.list caps maxResults at 50; a higher value 400s.)
        resp = await client.get(_YOUTUBE_SEARCH_URL, params={
            "part": "snippet", "q": query, "type": "video", "order": "relevance",
            "publishedAfter": published_after, "maxResults": "50",
            "key": settings.yt_api_key,
        })
        resp.raise_for_status()
        snippets = {
            (it.get("id") or {}).get("videoId"): (it.get("snippet") or {})
            for it in resp.json().get("items", [])
        }
        snippets.pop(None, None)
        if not snippets:
            return []

        # 2) Enrich with the FULL description + tags + uploader category the
        #    search snippet lacks - 1 quota unit for up to 50 ids.
        vresp = await client.get(_YOUTUBE_VIDEOS_URL, params={
            "part": "snippet,contentDetails", "id": ",".join(snippets), "maxResults": "50",
            "key": settings.yt_api_key,
        })
        vresp.raise_for_status()
        details = {v["id"]: v for v in vresp.json().get("items", [])}

    # 3) Our relevance curation over title + description + tags.
    temp: list[dict] = []
    for vid, ssnip in snippets.items():
        full = details.get(vid, {})
        d = full.get("snippet") or {}
        channel = d.get("channelTitle") or ssnip.get("channelTitle", "")
        title = d.get("title") or ssnip.get("title", "")
        if channel.lower() in excluded_chans:
            continue
        if ssnip.get("liveBroadcastContent") in ("live", "upcoming"):
            continue
        blob = " ".join((
            title, d.get("description", ""), " ".join(d.get("tags") or []),
        )).lower()
        # Drop YouTube Shorts: ≤60s clips, or anything explicitly hashtagged
        # #shorts. Short-form vertical content doesn't belong in the community rail.
        dur = _iso8601_seconds((full.get("contentDetails") or {}).get("duration"))
        if (dur is not None and dur <= 60) or _has_term(blob, "shorts"):
            continue
        if any(_has_term(blob, t) for t in exclude_terms):
            continue
        if not all(_has_term(blob, t) for t in require_terms):
            continue
        in_gaming = bool(gaming_cat) and d.get("categoryId") == gaming_cat
        has_signal = any(_has_term(blob, t) for t in relevance_terms)
        if relevance_terms and not (has_signal or in_gaming):
            continue
        thumbs = d.get("thumbnails") or ssnip.get("thumbnails") or {}
        temp.append({
            "title": title,
            "video_id": vid,
            "url": f"https://www.youtube.com/watch?v={vid}",
            "channel": channel,
            "published_at": ssnip.get("publishedAt") or d.get("publishedAt"),
            "thumbnail_url": (thumbs.get("high") or {}).get("url"),
        })

    # ≤ per_channel_max newest per channel, then the newest max_items overall.
    by_channel: dict[str, list[dict]] = {}
    for v in temp:
        by_channel.setdefault(v["channel"], []).append(v)
    pool: list[dict] = []
    for vids in by_channel.values():
        vids.sort(key=lambda x: x["published_at"] or "", reverse=True)
        pool.extend(vids[:per_channel_max])
    pool.sort(key=lambda x: x["published_at"] or "", reverse=True)
    return _normalize_videos(pool[:max_items])


# --- Bilibili (HTML scrape of the search page) ------------------------------

def _parse_bilibili_date(date_str: str, now: datetime) -> datetime:
    """Port of the bot's date parser: absolute (MM-DD / YYYY-MM-DD) and the
    Chinese relative forms (昨天 / N小时前 / N分钟前). Falls back to ``now`` on
    anything unrecognised or malformed rather than dropping the card."""
    s = (date_str or "").strip()
    try:
        if "-" in s:
            parts = s.split("-")
            if len(parts) == 2:  # MM-DD (current year, or last year if future)
                d = datetime(now.year, int(parts[0]), int(parts[1]), tzinfo=timezone.utc)
                return d.replace(year=now.year - 1) if d > now else d
            if len(parts) == 3:  # YYYY-MM-DD
                return datetime(int(parts[0]), int(parts[1]), int(parts[2]), tzinfo=timezone.utc)
        elif "昨天" in s:        # yesterday
            return now - timedelta(days=1)
        elif "小时前" in s:       # N hours ago
            return now - timedelta(hours=int(s.replace("小时前", "").strip()))
        elif "分钟前" in s:       # N minutes ago
            return now - timedelta(minutes=int(s.replace("分钟前", "").strip()))
    except (ValueError, TypeError):
        return now
    return now


async def _fetch_bilibili() -> list[dict]:
    # Lazy import so a missing optional dep degrades to "bilibili feed fails,
    # logged" rather than taking down the whole module (and Twitch/YouTube
    # with it) at import time.
    try:
        from bs4 import BeautifulSoup
    except ImportError as exc:
        raise RuntimeError(
            "beautifulsoup4 is not installed (required for the Bilibili scraper)"
        ) from exc

    keyword         = await runtime_config.get_setting("feeds_bilibili_keyword")
    cutoff_days     = int(await runtime_config.get_setting("feeds_video_cutoff_days"))
    per_channel_max = int(await runtime_config.get_setting("feeds_per_channel_max"))
    max_items       = int(await runtime_config.get_setting("feeds_max_items"))

    async with httpx.AsyncClient(timeout=15, follow_redirects=True, headers={
        "User-Agent": _BILIBILI_UA,
        "Accept": "text/html,application/xhtml+xml,application/xml;q=0.9,image/avif,image/webp,*/*;q=0.8",
        "Accept-Language": "en-US,en;q=0.5",
    }) as client:
        resp = await client.get(_BILIBILI_SEARCH_URL, params={"keyword": keyword})
        resp.raise_for_status()
        html = resp.text

    soup = BeautifulSoup(html, "html.parser")
    now = utcnow()
    cutoff = now - timedelta(days=cutoff_days)
    channel_counts: dict[str, int] = {}
    videos: list[dict] = []

    for card in soup.select(".bili-video-card"):
        a_tag = card.select_one("a")
        if not a_tag:
            continue
        href = a_tag.get("href", "")
        if not href.startswith("//www.bilibili.com/video/"):
            continue
        bvid = href.split("/video/")[1].strip("/")

        title_elem = card.select_one("h3.bili-video-card__info--tit")
        title = title_elem.get("title", "") if title_elem else ""
        author_elem = card.select_one(".bili-video-card__info--author")
        author = author_elem.text.strip() if author_elem else ""
        date_elem = card.select_one(".bili-video-card__info--date")
        date_str = date_elem.text.replace("·", "").strip() if date_elem else ""

        vid_date = _parse_bilibili_date(date_str, now)
        if vid_date < cutoff:
            continue
        if channel_counts.get(author, 0) >= per_channel_max:
            continue
        channel_counts[author] = channel_counts.get(author, 0) + 1

        pic_elem = card.select_one("picture source")
        pic_url = pic_elem.get("srcset", "").split("@")[0] if pic_elem else ""
        if pic_url.startswith("//"):
            pic_url = f"https:{pic_url}"

        videos.append({
            "title": title,
            "video_id": bvid,
            "url": f"https:{href}",
            "channel": author,
            "published_at": vid_date.isoformat().replace("+00:00", "Z"),
            "thumbnail_url": pic_url,
        })
        if len(videos) >= max_items:
            break

    videos.sort(key=lambda x: x["published_at"], reverse=True)
    return _normalize_videos(videos)


# feed -> native fetcher (each returns a normalized list[dict])
_FEEDS = {
    "twitch": _fetch_twitch,
    "youtube": _fetch_youtube,
    "bilibili": _fetch_bilibili,
}


async def refresh_feed(feed: str) -> int:
    """Fetch one feed at source and upsert its FeedCache doc. Raises on fetch
    failure (missing creds, upstream error) so ``refresh_all_feeds`` can log it
    and leave the previously cached payload in place."""
    items = await _FEEDS[feed]()
    # fetched_at is written on BOTH insert and update, so it rides in `fields`.
    await upsert_by(FeedCache, "feed", feed, {"items": items, "fetched_at": utcnow()})
    return len(items)


async def refresh_all_feeds() -> None:
    for feed in _FEEDS:
        try:
            count = await refresh_feed(feed)
            logger.info("Refreshed feed %s: %d item(s)", feed, count)
        except Exception:
            logger.warning("Feed '%s' refresh failed", feed, exc_info=True)


async def get_feed(feed: str) -> tuple[list[dict], datetime | None]:
    doc = await FeedCache.find_one(FeedCache.feed == feed)
    return (doc.items, doc.fetched_at) if doc else ([], None)


# --- Bilibili thumbnail proxy ----------------------------------------------
# hdslb.com (Bilibili's CDN) blocks hotlinking unless the request carries a
# bilibili.com Referer, which an <img> tag can't set. Clients point <img src> at
# the API and we refetch with the Referer, streaming the bytes back.

_BILIBILI_REFERER = "https://www.bilibili.com/"
_BILIBILI_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/120.0 Safari/537.36"
)


def _hdslb_target(url: str) -> httpx.URL:
    """Validate a caller-supplied URL as an https hdslb.com image and return the
    request URL rebuilt from the *validated* scheme/host, so nothing downstream
    fetches an unchecked host. Raises ValueError otherwise."""
    parsed = httpx.URL(url)
    host = (parsed.host or "").lower()
    if parsed.scheme != "https" or not (host == "hdslb.com" or host.endswith(".hdslb.com")):
        raise ValueError("only https hdslb.com images may be proxied")
    # Rebuild from the guarded host + literal https scheme; path/query preserved.
    return parsed.copy_with(scheme="https", host=host)


def _is_hdslb(url: str) -> bool:
    try:
        _hdslb_target(url)
    except ValueError:
        return False
    return True


async def fetch_bilibili_image(url: str) -> tuple[bytes, str]:
    """Proxy a Bilibili (hdslb.com) thumbnail, injecting the Referer its hotlink
    protection requires. Returns (bytes, content_type).

    Raises ValueError if the URL isn't an https hdslb.com image (guards against
    using the proxy as an open SSRF relay). Redirects are NOT followed, so a
    hdslb redirect can't bounce the fetch to an internal host. httpx errors
    propagate to the caller.
    """
    target = _hdslb_target(url)
    async with httpx.AsyncClient(
        timeout=10,
        follow_redirects=False,
        headers={"Referer": _BILIBILI_REFERER, "User-Agent": _BILIBILI_UA},
    ) as client:
        resp = await client.get(target)
        resp.raise_for_status()
        content_type = resp.headers.get("content-type", "image/jpeg")
        if not content_type.startswith("image/"):
            content_type = "image/jpeg"
        return resp.content, content_type


# --- Background refresher ---------------------------------------------------

_refresher = PeriodicRefresher(
    refresh_all_feeds,
    name="Feed refresh iteration",
    delay=lambda: settings.trove_feeds_refresh_seconds,
)


def start_feeds_refresher() -> None:
    _refresher.start()


async def stop_feeds_refresher() -> None:
    await _refresher.stop()
