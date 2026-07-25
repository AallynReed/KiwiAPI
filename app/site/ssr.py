"""Server-rendered first paint for the JS-driven site pages.

Most feature pages ship as an empty shell - a container plus a "Loading…"
placeholder - and paint themselves from the ``/site/*`` proxies after the JS
bundle runs. Browsers cope; crawlers largely don't. Googlebot renders JS on a
deferred second pass, so a page whose whole body arrives empty is indexed (and
scored) from HTML that contains no headlines, no item names, no rankings and -
worst for the catalog pages - no internal links to the detail pages underneath.

This module renders that above-the-fold content server-side. The HTML that
leaves the server already carries the real content and real ``<a href>``s; the
page JS then overwrites the same containers on load exactly as before, so live
behaviour, filtering and language switching are unchanged. The server-rendered
copy is English - the crawlable default - matching how ``classes_page`` and
``commands_page`` already work.

**Transport-agnostic by design.** Two containers render the same pages from two
different data paths: the API reads its own services in-process, the website
container fetches over HTTP. So every builder here takes a ``fetch`` callable
that returns the SAME JSON the ``/site/*`` proxies emit, and returns a plain
template model. Wiring lives in ``app/site/router.py`` (``_ssr_fetch``, a local
handler dispatch) and ``app/web/pages.py`` (``_ssr_fetch``, ``internal_get``).

**Everything fails soft.** A builder that can't get its data returns an empty
model and the template falls back to the JS-only placeholder it had before. A
slow or broken data plane must never take a page down - the ``fetch`` callables
swallow their own errors and return ``None``, and every builder tolerates it.

Fetches are memoised in-process (``_cache``) so a crawl burst doesn't re-run the
same query per hit, and a page's independent fetches are always issued
concurrently (``asyncio.gather``) so first paint costs one round-trip, not N.
"""
from __future__ import annotations

import asyncio
import logging
import re
import time
from collections.abc import Awaitable, Callable
from typing import Any
from urllib.parse import quote

logger = logging.getLogger("kiwi.site.ssr")

# ``fetch(path, params) -> parsed JSON | None``. Never raises - see module docs.
Fetch = Callable[[str, dict | None], Awaitable[Any]]

# How many rows each page pre-renders. Enough to give a crawler real content and
# real internal links, small enough that the HTML stays lean - the JS replaces
# these with the full, paginated, filterable set the moment it runs.
ROWS = 24


# ── fetch memo ─────────────────────────────────────────────────────────────
# Keyed by (path, sorted params). Short TTL: this is a first-paint cache, and
# the same payloads are already cached downstream (Redis / Cache-Control). The
# entry cap keeps per-slug keys (mod + modpack detail pages) from growing
# without bound; eviction is oldest-first, which is fine for a hit-rate cache.
_TTL = 60.0
_MAX_ENTRIES = 512
_cache: dict[str, tuple[float, Any]] = {}
_locks: dict[str, asyncio.Lock] = {}


def _key(path: str, params: dict | None) -> str:
    if not params:
        return path
    return path + "?" + "&".join(f"{k}={params[k]}" for k in sorted(params))


async def cached(fetch: Fetch, path: str, params: dict | None = None,
                 ttl: float = _TTL) -> Any:
    """Memoised ``fetch``. Single-flight per key so a burst of concurrent page
    renders issues one upstream call, not one per request."""
    key = _key(path, params)
    now = time.monotonic()
    hit = _cache.get(key)
    if hit is not None and now - hit[0] <= ttl:
        return hit[1]
    lock = _locks.setdefault(key, asyncio.Lock())
    async with lock:
        # Re-check: another waiter may have filled it while we queued.
        hit = _cache.get(key)
        now = time.monotonic()
        if hit is not None and now - hit[0] <= ttl:
            return hit[1]
        try:
            data = await fetch(path, params)
        except Exception:                       # defensive - fetch shouldn't raise
            logger.warning("ssr fetch %s failed", path, exc_info=True)
            data = None
        # A failed fetch keeps serving the last good payload if we have one, so a
        # brief data-plane blip doesn't blank the server-rendered content.
        if data is None and hit is not None:
            return hit[1]
        if len(_cache) >= _MAX_ENTRIES:
            for stale in list(_cache)[: _MAX_ENTRIES // 4]:
                _cache.pop(stale, None)
                _locks.pop(stale, None)
        _cache[key] = (time.monotonic(), data)
        return data


def _dict(data: Any) -> dict:
    """``data`` if it's a dict, else ``{}``. Every payload here arrives as
    ``Any`` (or ``None`` on a failed fetch), so narrowing once at the top of a
    builder keeps the rest of it free of isinstance noise."""
    return data if isinstance(data, dict) else {}


def _items(data: Any, key: str = "items") -> list[dict]:
    """The ``items`` list out of a proxy payload, or ``[]`` for anything else."""
    if not isinstance(data, dict):
        return []
    rows = data.get(key)
    return [r for r in rows if isinstance(r, dict)] if isinstance(rows, list) else []


# ── formatting helpers ─────────────────────────────────────────────────────
# Deliberately plain: these render the *crawlable* copy, so they emit absolute,
# locale-free text. The JS re-renders with the visitor's locale and live
# countdowns as soon as it runs.
def num(v: Any) -> str:
    """``12345`` -> ``"12,345"``; anything non-numeric -> em dash."""
    if isinstance(v, bool) or not isinstance(v, (int, float)):
        return "—"
    return f"{int(v):,}" if v == int(v) else f"{v:,.2f}".rstrip("0").rstrip(".")


def _unix(v: Any) -> int | None:
    """Unix seconds from an int or an ISO-8601 string; ``None`` if neither."""
    if isinstance(v, bool):
        return None
    if isinstance(v, (int, float)):
        return int(v)
    if isinstance(v, str) and v:
        try:
            from datetime import datetime
            return int(datetime.fromisoformat(v.replace("Z", "+00:00")).timestamp())
        except ValueError:
            return None
    return None


def date(v: Any) -> str:
    """An absolute UTC date - ``"12 Mar 2026"``. Absolute, not relative: the
    server-rendered copy can be cached or crawled hours later, and a stale
    "3 hours ago" is worse than a date that's simply true."""
    u = _unix(v)
    if u is None:
        return ""
    return time.strftime("%d %b %Y", time.gmtime(u))


def datetime_attr(v: Any) -> str:
    """ISO-8601 UTC for a ``<time datetime>`` attribute."""
    u = _unix(v)
    return time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime(u)) if u is not None else ""


def text(md: str | None, limit: int = 220) -> str:
    """Markdown/HTML -> a plain one-line excerpt for a card or meta description."""
    t = re.sub(r"<[^>]+>", " ", md or "")
    t = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", t)     # links/images -> their text
    t = re.sub(r"[#*`_>~|]", "", t)
    t = re.sub(r"\s+", " ", t).strip()
    return t[:limit].rstrip() + "…" if len(t) > limit else t


# ── page builders ──────────────────────────────────────────────────────────
async def home_view(fetch: Fetch, *, flags: dict[str, bool] | None = None) -> dict:
    """The homepage dashboard's first paint: today's bonuses and merchant
    rotations, the record highs, official news, community videos and the latest
    mods. Each block is independent - one failing source blanks only its own
    section, and ``home.js`` refills every one of them on load."""
    f = flags or {}
    want_lb = f.get("leaderboards_enabled", True)
    want_mods = f.get("mods_hub_enabled", True)

    rotations, news, videos, records, mods = await asyncio.gather(
        cached(fetch, "/site/rotations", ttl=30),
        cached(fetch, "/site/feeds/news", {"limit": 16}, ttl=300),
        cached(fetch, "/site/feeds/videos", {"platform": "youtube"}, ttl=300),
        cached(fetch, "/site/leaderboards/records", ttl=900) if want_lb else _none(),
        cached(fetch, "/site/mods/projects", {"sort": "recent", "limit": 40})
        if want_mods else _none(),
    )

    rot = _dict(rotations)
    out: dict[str, Any] = {
        "buffs": [b for b in (
            _buff(rot.get("daily_buff"), "Today's bonus"),
            _buff(rot.get("weekly_buff"), "This week's bonus"),
        ) if b],
        "chaos": _chaos(rot.get("chaos")),
        "merchants": [_merchant(m) for m in (rot.get("merchants") or [])
                      if isinstance(m, dict)],
        # Shop-offer posts are hidden by default on the client; mirror that here
        # so the server copy and the hydrated copy list the same headlines.
        "news": [_news(n) for n in _items(news)
                 if "Shop Offers" not in (n.get("categories") or [])][:12],
        "videos": [_video(v) for v in _items(videos)][:12],
        "records": _records(records),
        "mods": _mod_rail(_items(mods)),
    }
    return out


async def _none() -> None:
    """An awaitable ``None`` - lets ``gather`` keep its positional shape when a
    feature is toggled off instead of branching the unpack."""
    return None


def _buff(buff: Any, kicker: str) -> dict | None:
    if not isinstance(buff, dict) or not (buff.get("name") or "").strip():
        return None
    return {
        "kicker": kicker,
        "name": (str(buff.get("emoji") or "") + " ").lstrip() + str(buff["name"]),
        "list": [b for b in (buff.get("normal_buffs") or buff.get("buffs") or [])
                 if isinstance(b, str)][:4],
    }


def _chaos(chaos: Any) -> dict | None:
    if not isinstance(chaos, dict) or not chaos.get("ends_at"):
        return None
    item = chaos.get("item") if isinstance(chaos.get("item"), dict) else {}
    return {"name": (item or {}).get("name") or "Featured item",
            "ends_at": datetime_attr(chaos.get("ends_at"))}


def _merchant(m: dict) -> dict:
    return {
        "name": m.get("name") or "",
        "active": bool(m.get("active")),
        "badge": (m.get("state") or "Here") if m.get("active") else "Away",
        "biomes": [b if isinstance(b, str) else (b or {}).get("name") or ""
                   for b in (m.get("biomes") or [])][:3],
    }


def _news(n: dict) -> dict:
    return {"title": n.get("title") or "", "url": n.get("url") or "",
            "category": n.get("category") or "", "image": n.get("image") or "",
            "date": date(n.get("published_at")),
            "datetime": datetime_attr(n.get("published_at"))}


def _video(v: dict) -> dict:
    thumb = (v.get("thumbnail_url") or v.get("thumbnail") or "")
    return {"title": v.get("title") or "", "url": v.get("url") or "",
            "channel": v.get("channel") or "",
            "thumb": thumb.replace("{width}", "440").replace("{height}", "248"),
            "date": date(v.get("published_at"))}


def _records(data: Any) -> list[dict]:
    """Trove Mastery / Geode Mastery / Power Rank record highs - the same three
    cards ``home.js`` builds, as flat rows."""
    d = _dict(data)
    out = []
    for key, kicker, is_level in (
        ("trove_mastery", "Trove Mastery", True),
        ("geode_mastery", "Geode Mastery", True),
        ("power_rank", "Power Rank", False),
    ):
        r = d.get(key)
        if not isinstance(r, dict):
            continue
        out.append({
            "kicker": kicker,
            "value": ("Level " + num(r.get("level"))) if is_level else num(r.get("value")),
            "meta": (num(r.get("points")) + " pts") if is_level
                    else "Highest across all classes",
            "holder": r.get("player_name") or "",
        })
    return out


def _mod_rail(items: list[dict]) -> list[dict]:
    """Latest mods for the homepage rail, capped at 2 per author so one prolific
    author can't monopolise it (same rule as ``home.js``)."""
    per_author: dict[str, int] = {}
    picked = []
    for m in items:
        if len(picked) >= 8:
            break
        if not (m.get("handle") and m.get("slug")):
            continue
        key = (m.get("author") or m.get("owner_username") or m["handle"]).lower()
        if per_author.get(key, 0) >= 2:
            continue
        per_author[key] = per_author.get(key, 0) + 1
        picked.append({
            "title": m.get("title") or m["slug"],
            "author": m.get("author") or m.get("owner_username") or "",
            "url": f"/mods/{m['handle']}/{m['slug']}",
        })
    return picked


async def mods_view(fetch: Fetch) -> dict:
    """The Mods Hub catalog grid. The most valuable page on the site to
    server-render: without it the mod detail pages have no crawlable inbound
    links at all except the XML sitemap."""
    data = await cached(fetch, "/site/mods/projects",
                        {"sort": "recent", "limit": ROWS})
    rows = _items(data)
    return {
        "mods": [_mod_card(m) for m in rows if m.get("handle") and m.get("slug")],
        "total": (data or {}).get("total") if isinstance(data, dict) else None,
    }


def _mod_card(m: dict) -> dict:
    return {
        "title": m.get("title") or m["slug"],
        "url": f"/mods/{m['handle']}/{m['slug']}",
        "summary": text(m.get("summary"), 160),
        "author": m.get("author") or m.get("owner_username") or "",
        "tags": [t for t in (m.get("tags") or []) if isinstance(t, str)][:4],
        "downloads": num(m.get("download_count") or 0),
        "stars": num(m.get("star_count") or 0),
    }


async def modpacks_view(fetch: Fetch) -> dict:
    """The modpack catalog grid - same rationale as ``mods_view``."""
    data = await cached(fetch, "/site/modpacks/projects", {"limit": ROWS})
    rows = _items(data)
    return {
        "modpacks": [{
            "title": p.get("title") or p["slug"],
            "url": f"/modpacks/{p['handle']}/{p['slug']}",
            "summary": text(p.get("summary"), 160),
            "author": p.get("owner_username") or "",
            "downloads": num(p.get("download_count") or 0),
        } for p in rows if p.get("handle") and p.get("slug")],
        "total": (data or {}).get("total") if isinstance(data, dict) else None,
    }


async def updates_view(fetch: Fetch) -> dict:
    """The update archive's branch list + the most recent captured versions of
    the default (Live US) branch, with per-version change counts."""
    branches, versions = await asyncio.gather(
        cached(fetch, "/site/updates/branches"),
        cached(fetch, "/site/updates/live-us/versions", {"limit": ROWS}),
    )
    return {
        "branches": [{"branch": b.get("branch") or "",
                      "current": b.get("current_version") or ""}
                     for b in _items(branches) if b.get("branch")],
        "versions": [{
            "tag": v.get("version_tag") or "",
            "date": date(v.get("captured_at")),
            "datetime": datetime_attr(v.get("captured_at")),
            "added": num(v.get("files_added") or 0),
            "modified": num(v.get("files_modified") or 0),
            "removed": num(v.get("files_removed") or 0),
        } for v in _items(versions) if v.get("version_tag")],
    }


async def codexes_view(fetch: Fetch) -> dict:
    """The codex type tabs + a first page of entries, so the item names Trove
    players actually search for exist in the HTML."""
    types, entries = await asyncio.gather(
        cached(fetch, "/site/codexes/types"),
        cached(fetch, "/site/codexes/search", {"sort": "name", "limit": ROWS}),
    )
    return {
        "types": [{"type": t.get("type") or "", "count": num(t.get("count") or 0)}
                  for t in _items(types) if t.get("type")],
        "entries": [{
            "name": e.get("name") or "",
            "type": e.get("type") or "",
            "category": e.get("category") or "",
        } for e in _items(entries) if e.get("name")],
        "total": (entries or {}).get("total") if isinstance(entries, dict) else None,
    }


async def market_view(fetch: Fetch) -> dict:
    """Traded item names + the most recent listings. Item names are the whole
    reason anyone finds this page from a search engine."""
    items, listings = await asyncio.gather(
        cached(fetch, "/site/market/items"),
        cached(fetch, "/site/market/listings", {"limit": ROWS, "sort": "-last_seen"}),
    )
    names = (items or {}).get("items") if isinstance(items, dict) else None
    names = [n for n in (names or []) if isinstance(n, str)]
    return {
        # Every traded item name, as text. This is the page's whole search
        # surface: players arrive looking for one item by name.
        # NB "item_names", not "items": Jinja resolves ``ssr.items`` to dict.items
        # (attribute lookup wins over subscript), so a model key that collides
        # with a dict method silently renders the bound method instead.
        "item_names": names[:200],
        "item_count": num(len(names)) if names else None,
        "listings": [{
            "name": r.get("name") or "",
            "each": num(r.get("price_each")),
            "stack": num(r.get("stack")),
            "total": num(r.get("price")),
            "date": date(r.get("last_seen")),
        } for r in _items(listings) if r.get("name")],
    }


async def store_view(fetch: Fetch) -> dict:
    """The in-game store catalog: the category tabs plus the packs live now."""
    cats, products = await asyncio.gather(
        cached(fetch, "/site/store/categories"),
        cached(fetch, "/site/store/products", {"limit": ROWS, "active": "true"}),
    )

    def _label(raw: str) -> str:
        # Category labels arrive as raw localisation keys ("$StoreCategory_Foo");
        # store.js strips the prefix for display, so mirror it here.
        return re.sub(r"^\$?(StoreCategory_)?", "", raw or "")

    return {
        "categories": [{"label": _label(c.get("label") or c.get("name") or "")}
                       for c in _items(cats) if (c.get("label") or c.get("name"))],
        "products": [{
            "name": p.get("name") or p.get("code") or "",
            "price": p.get("price_string") or "",
            "kind": p.get("kind") or "",
        } for p in _items(products) if (p.get("name") or p.get("code"))],
        "total": (products or {}).get("total") if isinstance(products, dict) else None,
    }


async def calendar_view(fetch: Fetch) -> dict:
    """Today's rotations plus the ongoing / upcoming Trove events."""
    rotations, events = await asyncio.gather(
        cached(fetch, "/site/rotations", ttl=30),
        cached(fetch, "/site/calendar/events", ttl=120),
    )
    rot = _dict(rotations)
    ev = _dict(events)

    def _ev(e: dict) -> dict:
        return {"name": e.get("name") or "", "url": e.get("url") or "",
                "category": e.get("category") or "",
                "starts": date(e.get("starts_at")), "ends": date(e.get("ends_at"))}

    return {
        "buffs": [b for b in (
            _buff(rot.get("daily_buff"), "Today's bonus"),
            _buff(rot.get("weekly_buff"), "This week's bonus"),
        ) if b],
        "merchants": [_merchant(m) for m in (rot.get("merchants") or [])
                      if isinstance(m, dict)],
        "ongoing": [_ev(e) for e in _items(ev, "ongoing")][:20],
        "upcoming": [_ev(e) for e in _items(ev, "upcoming")][:20],
    }


async def streams_view(fetch: Fetch) -> dict:
    """Community videos, live streams and official news headlines."""
    youtube, twitch, news = await asyncio.gather(
        cached(fetch, "/site/feeds/videos", {"platform": "youtube"}, ttl=300),
        cached(fetch, "/site/feeds/videos", {"platform": "twitch"}, ttl=300),
        cached(fetch, "/site/feeds/news", {"limit": 16}, ttl=300),
    )
    return {
        "videos": [_video(v) for v in _items(youtube)][:12],
        "streams": [_video(v) for v in _items(twitch)][:12],
        "news": [_news(n) for n in _items(news)][:12],
    }


async def releases_view(fetch: Fetch) -> dict:
    """BetterTroveTools release history - version, channel and date per build."""
    data = await cached(fetch, "/site/btt/releases", {"limit": ROWS}, ttl=180)
    return {
        "releases": [{
            "tag": r.get("tag_name") or "",
            "name": r.get("name") or r.get("tag_name") or "",
            "channel": r.get("channel") or "",
            "url": r.get("html_url") or "",
            "date": date(r.get("published_at")),
            "datetime": datetime_attr(r.get("published_at")),
            "notes": text(r.get("body"), 240),
        } for r in _items(data) if r.get("tag_name")],
    }


async def status_view(fetch: Fetch) -> dict:
    """Current Live / PTS server state. Small, but it's the entire answer to the
    query this page exists for ("is Trove down") - it belongs in the HTML."""
    data = await cached(fetch, "/site/trove-status", ttl=30)
    d = _dict(data)
    overall = d.get("overall") or "unknown"
    envs = _dict(d.get("environments"))
    labels = (("eu", "Live EU"), ("us", "Live US"), ("pts", "PTS"))
    return {
        # The one-line answer to the query this page exists for.
        "headline": {"online": "All Trove servers operational",
                     "down": "Trove is down"}.get(overall, "Checking Trove status…"),
        "overall": overall,
        "environments": [{
            "label": label,
            "state": str(_dict(envs.get(key)).get("status") or "unknown").title(),
        } for key, label in labels if isinstance(envs.get(key), dict)],
        "checked": date(d.get("checked_at")),
    }


async def activity_view(fetch: Fetch) -> dict:
    """The live active-player estimate plus its 24h / 7d rollups."""
    data = await cached(fetch, "/site/leaderboards/activity", ttl=120)
    d = _dict(data)
    rows = [(label, d.get(key)) for label, key in (
        ("right now", "estimate"), ("in the last 24h", "estimate_24h"),
        ("in the last 7 days", "estimate_7d"),
    )]
    return {
        "estimate": num(d["estimate"]) if isinstance(d.get("estimate"), (int, float)) else "",
        "rollups": [{"label": label, "value": num(v)}
                    for label, v in rows if isinstance(v, (int, float))],
        "as_of": date(d.get("window_end")),
    }


async def class_activity_view(fetch: Fetch) -> dict:
    """Per-class player counts and share from the latest capture - a real,
    crawlable "which class is most played in Trove" table."""
    data = await cached(fetch, "/site/leaderboards/class-activity/current", ttl=120)
    rows = [c for c in _items(data, "classes")
            if c.get("name") and isinstance(c.get("active_players"), (int, float))]
    rows.sort(key=lambda c: c["active_players"], reverse=True)
    return {
        "classes": [{
            "name": c["name"],
            "players": num(c["active_players"]),
            "share": (f"{c['share'] * 100:.1f}%"
                      if isinstance(c.get("share"), (int, float)) else ""),
        } for c in rows][:40],
        "total": num((data or {}).get("total_active"))
                 if isinstance(data, dict) and data.get("total_active") else "",
    }


async def giveaways_view(fetch: Fetch) -> dict:
    """Open / upcoming giveaways with their prize names."""
    data = await cached(fetch, "/site/giveaways", ttl=30)
    return {"giveaways": [{
        "prize": g.get("prize_name") or "Giveaway",
        "title": g.get("title") or "",
        "description": text(g.get("description"), 200),
        "status": g.get("status") or "",
        "entries": num(g.get("entry_count")) if g.get("entry_count") is not None else "",
        "ends": date(g.get("ends_at")),
        "winner": g.get("winner_username") or "",
    } for g in _items(data)][:12]}


async def leaderboards_view(fetch: Fetch) -> dict:
    """The board list plus the top of the default board at the latest capture.

    Three dependent fetches (latest anchor -> boards -> entries), so this is the
    one page whose first paint can't be a single round-trip. All three are
    memoised and the anchor moves only once an hour, so in practice a crawl hit
    reads cache."""
    stamps = await cached(fetch, "/site/leaderboards/timestamps", {"limit": 1}, ttl=120)
    anchors = (stamps or {}).get("items") if isinstance(stamps, dict) else None
    anchor = next((a for a in (anchors or []) if isinstance(a, int)), None)
    if anchor is None:
        return {}
    boards = await cached(fetch, "/site/leaderboards/boards",
                          {"created_at": anchor}, ttl=120)
    rows = _items(boards)
    if not rows:
        return {}
    first = rows[0]
    uuid = first.get("uuid")
    entries = await cached(fetch, f"/site/leaderboards/{uuid}/entries",
                           {"created_at": anchor, "limit": 25}, ttl=120) if uuid else None
    return {
        "captured": date(anchor),
        "datetime": datetime_attr(anchor),
        "boards": [{"name": b.get("name") or "",
                    "entries": num(b.get("entries")) if b.get("entries") is not None else ""}
                   for b in rows if b.get("name")][:100],
        "board_name": first.get("name") or "",
        "entries": [{
            "rank": num(e.get("rank")),
            "player": e["player_name"],
            "url": "/player/" + quote(e["player_name"]),
            "score": num(e.get("score")),
        } for e in _items(entries) if e.get("player_name")],
    }


async def player_view(fetch: Fetch, name: str) -> dict:
    """A player's leaderboard standings - the substance of ``/player/<name>``,
    which otherwise arrives as a name in an ``<h1>`` and nothing else."""
    data = await cached(fetch, f"/site/leaderboards/players/{quote(name)}/profile", ttl=120)
    d = _dict(data)
    summary = _dict(d.get("summary"))
    boards = [b for b in _items(d, "boards") if b.get("board_name")]
    return {
        "canonical": d.get("player_name") or name,
        "verified": bool(d.get("verified")),
        "summary": [s for s in (
            {"label": "Best rank",
             "value": "#" + num(summary["best_rank"])} if summary.get("best_rank") else None,
            {"label": "Leaderboards", "value": num(summary.get("boards_appeared"))}
            if summary.get("boards_appeared") else None,
            {"label": "Top 10s", "value": num(summary.get("top10_count"))}
            if summary.get("top10_count") else None,
            {"label": "Last seen", "value": date(summary.get("latest_anchor"))}
            if summary.get("latest_anchor") else None,
        ) if s],
        "boards": [{
            "board": b["board_name"],
            "category": b.get("category") or "",
            "rank": ("#" + num(b["latest_rank"])) if b.get("latest_rank") else "",
            "best": ("#" + num(b["best_rank"])) if b.get("best_rank") else "",
            "score": num(b.get("latest_score")) if b.get("latest_score") is not None else "",
        } for b in boards][:60],
    }


def mod_project_view(project: Any) -> dict:
    """A single mod's page, from the payload the route already fetched for its
    Open Graph tags - so this costs no extra round-trip."""
    p = _dict(project)
    if not p:
        return {}
    return {
        "title": p.get("title") or p.get("slug") or "",
        "summary": text(p.get("summary"), 300),
        "description": text(p.get("description"), 1200),
        "author": p.get("author") or p.get("owner_username") or "",
        "owner": p.get("owner_username") or "",
        "tags": [t for t in (p.get("tags") or []) if isinstance(t, str)][:8],
        "downloads": num(p.get("download_count") or 0),
        "stars": num(p.get("star_count") or 0),
        "updated": date(p.get("updated_at")),
        "datetime": datetime_attr(p.get("updated_at")),
    }


def mod_profile_view(profile: Any) -> dict:
    """A modder's profile page, from the payload the route already fetched."""
    d = _dict(profile)
    if not d:
        return {}
    return {
        "name": d.get("display_name") or d.get("username") or "",
        "tagline": text(d.get("tagline"), 200),
        "readme": text(d.get("readme"), 900),
        "mods": [_mod_card(m) for m in _items(d, "mods")
                 if m.get("handle") and m.get("slug")][:ROWS],
    }


def modpack_project_view(pack: Any) -> dict:
    """A single modpack's page, from the payload the route already fetched."""
    p = _dict(pack)
    if not p:
        return {}
    # The bundled mods hang off each variant, and the same mod usually appears in
    # several. De-dupe by page URL so the list reads as "what's in this pack"
    # (and each mod contributes one crawlable link, not one per variant).
    seen: set[str] = set()
    mods = []
    for variant in _items(p, "variants"):
        for m in _items(variant, "mods"):
            title = m.get("title") or m.get("slug")
            url = (f"/mods/{m['handle']}/{m['slug']}"
                   if m.get("handle") and m.get("slug") else "")
            if not title or url in seen:
                continue
            seen.add(url)
            mods.append({"title": title, "url": url, "author": m.get("author") or ""})
    return {
        "title": p.get("title") or p.get("slug") or "",
        "summary": text(p.get("summary"), 300),
        "description": text(p.get("description"), 1200),
        "owner": p.get("owner_username") or "",
        "downloads": num(p.get("download_count") or 0),
        "updated": date(p.get("updated_at")),
        "variants": [v.get("label") or v.get("name") or ""
                     for v in _items(p, "variants")][:20],
        "mods": mods[:80],
    }
