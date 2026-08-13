"""Routes for the BetterTroveTools showcase site (`trove.aallyn.net`).

HTML page routes plus a JSON surface under ``/site/*`` that mirrors the read-side
of ``/v1/*`` but tokenless + same-origin, so the page-side JS isn't throttled by
the public API's per-token caps. The data is already public, so the bypass costs
nothing. Every ``/site/<feature>/*`` proxy is feature-gated in ``_feature_blocks``.
"""

import asyncio
import json
import logging
import re
import time
from pathlib import Path
from urllib.parse import unquote

from fastapi import APIRouter, Depends, File, Form, HTTPException, Query, Request, UploadFile
from fastapi.encoders import jsonable_encoder
from fastapi.responses import HTMLResponse, JSONResponse, RedirectResponse, Response
from fastapi.templating import Jinja2Templates

from app.admin import runtime_config
from app.core import features as feature_flags
from app.core.config import settings
from app.core.utils import iso
from app.site import classes_page, commands_page, ssr
from app.site.feature_map import SITE_FEATURE_FLAGS as _SITE_FEATURE_FLAGS
from app.site.feature_map import SITEMAP_PAGES as _SITEMAP_PAGES
from app.site.feature_map import feature_blocks as _feature_blocks
from app.site.feature_map import robots_body as _robots_body
from app.site_auth.dependencies import get_current_site_user, get_optional_site_user
from app.site_auth.models import SiteUser
from app.trove import btt_releases as trove_btt
from app.trove import calendar as trove_calendar
from app.trove import captures as trove_captures
from app.trove import chaos as trove_chaos
from app.trove import feeds as trove_feeds
from app.trove import luxion as trove_luxion
from app.trove import news as trove_news
from app.trove import rotations as trove_rotations
from app.trove import server_time as trove_server_time
from app.trove import stats as trove_stats
from app.trove import status as trove_status
from app.trove.codexes import crafting as codexes_crafting
from app.trove.codexes import read as codexes_read
from app.trove.codexes.types import ALL_TYPES as CODEX_TYPES
from app.trove.gems import builds as gem_builds
from app.trove.gems import evaluator as gem_evaluator
from app.trove.gems.model import gem_lookups
from app.trove.gems.schemas import BuildConfigRequest, EvaluateRequest, SimpleEvaluateRequest
from app.trove.leaderboards import activity as leaderboards_activity
from app.trove.leaderboards import cache as leaderboards_cache
from app.trove.leaderboards import class_activity as leaderboards_class_activity
from app.trove.leaderboards import detection as leaderboards_detection
from app.trove.leaderboards import duplicates as leaderboards_duplicates
from app.trove.leaderboards import renames as leaderboards_renames
from app.trove.leaderboards import service as leaderboards_service
from app.trove.models import TroveEvent
from app.trove.modpacks import service as modpacks_service
from app.trove.mods_hub import creators as mods_hub_creators
from app.trove.mods_hub import service as mods_hub_service
from app.trove.mods_hub.schemas import CreatorScopeRequest
from app.trove.render import bp_cache
from app.trove.render.service import render_blueprint_cached, render_creature_cached
from app.trove.updates import compare as updates_compare
from app.trove.updates import read as updates_read
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES as UPDATE_BRANCHES

logger = logging.getLogger("kiwi.site.router")

# Filename extensions accepted as Trove screenshots for the hero slideshow.
# Anything else in the folder (READMEs, .DS_Store, etc.) is silently skipped.
_SCREENSHOT_EXTS = {".webp", ".png", ".jpg", ".jpeg", ".gif"}

async def _resolve_feature_flags(request: Request) -> None:
    """Per-site-request feature gate. Resolves the master toggles once and (a)
    stashes them on ``request.state`` for the template context processor below
    (so the navbar can hide a disabled feature's link), and (b) 404s the pages +
    ``/site/<feature>/*`` proxies of any disabled feature so it's hidden, not
    just unlinked. Cheap: the values are cached ~5s in runtime_config."""
    flags = {
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    }
    for attr, value in flags.items():
        setattr(request.state, attr, value)
    if _feature_blocks(request.url.path, flags):
        raise HTTPException(status_code=404)


def _flag_map(request: Request) -> dict[str, bool]:
    """The resolved feature flags as a plain dict - what the SSR builders need to
    skip fetching for a feature that's switched off."""
    return {attr: bool(getattr(request.state, attr, True))
            for attr in _SITE_FEATURE_FLAGS}


def _feature_context(request: Request) -> dict:
    """Inject the feature flags into EVERY template (the navbar + dashboard read
    them). Resolved by ``_resolve_feature_flags`` above; default to enabled."""
    return {
        attr: getattr(request.state, attr, True)
        for attr in _SITE_FEATURE_FLAGS
    }


_TEMPLATES = Jinja2Templates(
    directory=str(Path(settings.site_root) / "templates"),
    context_processors=[_feature_context],
)

router = APIRouter(
    tags=["site"], include_in_schema=False,
    dependencies=[Depends(_resolve_feature_flags)],
)


# ── server-rendered first paint ────────────────────────────────────────────
# The page routes below pre-render their above-the-fold content so crawlers get
# real HTML instead of an empty shell (see ``app/site/ssr.py``). The builders
# there are transport-agnostic: they ask for the SAME payloads the ``/site/*``
# proxies emit. In THIS container those proxies are local coroutines, so we
# dispatch straight to them - no HTTP, no self-request. (The website container
# fetches the same paths over the compose network instead; see
# ``app/web/pages.py``.)
#
# Only the paths the SSR builders actually ask for are routed; anything else
# returns None, which the builders treat as "no data" and the template falls
# back to its JS-only placeholder. The auth-gated leaderboard proxies are called
# anonymously (``user=None``) - the same view a crawler would get.
async def _ssr_fetch(path: str, params: dict | None = None) -> object | None:
    """Call this container's own ``/site/*`` handler and return its parsed JSON.

    Never raises: a handler that 404s / errors resolves to ``None`` so a single
    broken data source can't take a page render down with it."""
    p = params or {}
    try:
        resp = await _ssr_dispatch(path, p)
    except HTTPException:
        return None                       # e.g. an empty archive 404ing
    except Exception:
        logger.warning("ssr dispatch %s failed", path, exc_info=True)
        return None
    if resp is None:
        return None
    body = getattr(resp, "body", None)
    return json.loads(body) if body else None


_PLAYER_PREFIX = "/site/leaderboards/players/"
_PLAYER_SUFFIX = "/profile"


async def _ssr_dispatch(path: str, p: dict) -> JSONResponse | None:
    """Path -> local handler. Explicit rather than reflective: the SSR surface is
    a small fixed set, and an explicit table can't accidentally expose a proxy
    the builders never meant to call."""
    match path:
        case "/site/rotations":
            return await site_rotations()
        case "/site/feeds/news":
            return await site_feeds_news(limit=int(p.get("limit", 16)))
        case "/site/feeds/videos":
            return await site_feeds_videos(platform=str(p.get("platform", "youtube")))
        case "/site/calendar/events":
            return await site_calendar_events()
        case "/site/trove-status":
            return await site_trove_status()
        case "/site/giveaways":
            return await site_giveaways()
        case "/site/btt/releases":
            return await site_btt_releases(channel=None, limit=int(p.get("limit", 30)),
                                           offset=0)
        case "/site/mods/projects":
            return await site_mods_projects(
                q=None, tag=None, author=None, sort=str(p.get("sort", "recent")),
                limit=int(p.get("limit", 30)), offset=0)
        case "/site/modpacks/projects":
            return await site_modpacks_projects(
                q=None, tag=None, author=None, sort="recent",
                limit=int(p.get("limit", 30)), offset=0)
        case "/site/updates/branches":
            return await site_up_branches()
        case "/site/codexes/types":
            return await site_codex_types(branch=_DEFAULT_CODEX_BRANCH)
        case "/site/codexes/search":
            return await site_codex_search(
                branch=_DEFAULT_CODEX_BRANCH, q=None, type=None, category=None,
                tradable=None, sort=str(p.get("sort", "name")),
                limit=int(p.get("limit", 60)), offset=0)
        case "/site/market/items":
            return await site_market_items()
        case "/site/market/listings":
            return await site_market_listings(
                name=None, price_min=None, price_max=None, last_seen_after=None,
                hide_expired=True, sort=str(p.get("sort", "-last_seen")),
                limit=int(p.get("limit", 100)), offset=0)
        case "/site/store/categories":
            return await site_store_categories()
        case "/site/store/products":
            return await site_store_products(
                category=None, kind=None, currency=None, q=None, active=True,
                on_sale=False, limit=int(p.get("limit", 500)), offset=0)
        case "/site/leaderboards/records":
            return await site_lb_records()
        case "/site/leaderboards/activity":
            return await site_lb_activity()
        case "/site/leaderboards/class-activity/current":
            return await site_lb_class_activity_current()
        case "/site/leaderboards/timestamps":
            return await site_lb_timestamps(limit=int(p.get("limit", 60)))
        case "/site/leaderboards/boards":
            return await site_lb_boards(created_at=int(p["created_at"]), user=None)
    # Parameterised paths.
    if (m := re.fullmatch(r"/site/updates/([\w-]+)/versions", path)):
        return await site_up_versions(branch=m.group(1),
                                      limit=int(p.get("limit", 50)), offset=0)
    if (m := re.fullmatch(r"/site/leaderboards/(\d+)/entries", path)):
        return await site_lb_entries(uuid=int(m.group(1)),
                                     created_at=int(p["created_at"]),
                                     limit=int(p.get("limit", 100)), offset=0, user=None)
    # Sliced rather than matched: a player name is "anything", and `(.+)` before a
    # literal suffix is a backtracking pattern (CodeQL py/polynomial-redos).
    if path.startswith(_PLAYER_PREFIX) and path.endswith(_PLAYER_SUFFIX):
        name = path[len(_PLAYER_PREFIX):-len(_PLAYER_SUFFIX)]
        if name:
            return await site_lb_player_profile(player_name=unquote(name))
    return None


@router.get("/", response_class=HTMLResponse)
async def home(request: Request) -> HTMLResponse:
    """The content-hub homepage - a live front door (server status, leaderboard
    movers, latest mods, newest update, featured codex, reference). The app
    *showcase* lives at ``/app``."""
    return _TEMPLATES.TemplateResponse(request, "index.html", {
        "discord_install_url": settings.discord_install_link,
        "ssr": await ssr.home_view(_ssr_fetch, flags=_flag_map(request)),
    })


@router.get("/app", response_class=HTMLResponse)
async def app_showcase(request: Request) -> HTMLResponse:
    """The BetterTroveTools app showcase + downloads. Moved off ``/`` (now the
    content front door) but still linked from the navbar and the homepage CTA."""
    return _TEMPLATES.TemplateResponse(
        request, "app.html", {"discord_install_url": settings.discord_install_link},
    )


# --- Embeddable status badge ("backlink magnet") ---------------------------
# A shields.io-style SVG badge other Trove sites / Discords / READMEs embed via
# ``<a href="…/status"><img src="…/embed/status.svg"></a>``. Because the <a>
# lives on the HOST page's DOM (unlike an iframe), each embed is a real,
# followable backlink to trove.aallyn.net - plus referral traffic. The /status
# page hands out copy-paste HTML + Markdown snippets.
_BADGE_COLORS = {"online": "#3fb950", "down": "#f85149", "unknown": "#9aa4b2"}
_BADGE_VALUES = {"online": "online", "down": "offline", "unknown": "unknown"}


def _badge_svg(label: str, value: str, color: str) -> str:
    """Minimal flat status badge. Widths are approximated from text length
    (~6.5px/char + padding) - good enough for a two-segment badge without
    bundling a font-metrics table."""
    def w(s: str) -> int:
        return int(len(s) * 6.5) + 12
    lw, vw = w(label), w(value)
    total = lw + vw
    lx, vx = lw * 5, (lw + vw // 2) * 10  # text anchors at *10 (scaled coords)
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{total}" height="20" '
        f'role="img" aria-label="{label}: {value}">'
        f'<linearGradient id="s" x2="0" y2="100%">'
        f'<stop offset="0" stop-color="#bbb" stop-opacity=".1"/>'
        f'<stop offset="1" stop-opacity=".1"/></linearGradient>'
        f'<clipPath id="r"><rect width="{total}" height="20" rx="3" fill="#fff"/></clipPath>'
        f'<g clip-path="url(#r)">'
        f'<rect width="{lw}" height="20" fill="#2b3038"/>'
        f'<rect x="{lw}" width="{vw}" height="20" fill="{color}"/>'
        f'<rect width="{total}" height="20" fill="url(#s)"/></g>'
        f'<g fill="#fff" text-anchor="middle" '
        f'font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="11">'
        f'<text x="{lx}" y="15" transform="scale(.1)" textLength="{(lw - 12) * 10}">{label}</text>'
        f'<text x="{vx}" y="15" transform="scale(.1)" textLength="{(vw - 12) * 10}">{value}</text>'
        f'</g></svg>'
    )


@router.get("/embed/status.svg")
async def embed_status_badge() -> Response:
    """Live Trove server-status badge (SVG). Embed via a linked image so it
    doubles as a backlink. ``no-cache``-ish short TTL keeps it fresh without
    hammering the prober."""
    overall = (trove_status.get_status() or {}).get("overall", "unknown")
    if overall not in _BADGE_COLORS:
        overall = "unknown"
    svg = _badge_svg("trove", _BADGE_VALUES[overall], _BADGE_COLORS[overall])
    return Response(
        svg, media_type="image/svg+xml",
        headers={"Cache-Control": "public, max-age=60", "Access-Control-Allow-Origin": "*"},
    )


@router.get("/browse", response_class=HTMLResponse)
async def browse_index(request: Request) -> HTMLResponse:
    """Human-readable site index ("HTML sitemap"): real ``<a>`` links to every
    public modpack page. The catalog grids render client-side, so mod/modpack pages
    otherwise have no crawlable internal links - only the XML sitemap. Linked from
    the footer so it's reachable everywhere."""
    # Modpacks only: the full mod list (thousands of entries) was excessive for a
    # human index, and the XML sitemap already gives search engines every mod URL.
    packs: list[dict] = []
    if getattr(request.state, "mods_hub_enabled", True):
        packs = await _all_public_cards(
            modpacks_service.list_public, _SITEMAP_MAX_PER_SECTION, "browse-modpacks")
    packs.sort(key=lambda c: (c.get("title") or c.get("slug") or "").lower())
    return _TEMPLATES.TemplateResponse(request, "browse.html", {"modpacks": packs})


@router.get("/robots.txt", include_in_schema=False)
async def robots_txt(request: Request) -> Response:
    """Crawler directives, host-aware (see ``feature_map.robots_body``)."""
    return Response(
        _robots_body(request.url.hostname or ""), media_type="text/plain",
        headers={"Cache-Control": "public, max-age=86400"},
    )


# Bing Webmaster Tools site-ownership verification. Bing fetches this XML from
# the site ROOT (not /static), so it needs a real root route.
_BING_VERIFY = (
    '<?xml version="1.0"?>\n'
    '<users>\n'
    '\t<user>FC86658CF71BBCB1184266DE6480D237</user>\n'
    '</users>\n'
)


@router.get("/BingSiteAuth.xml")
async def bing_site_auth() -> Response:
    """Bing Webmaster Tools ownership-verification file, served at the site root
    so Bing's fetcher (and IndexNow) can confirm the domain."""
    return Response(
        _BING_VERIFY, media_type="application/xml",
        headers={"Cache-Control": "public, max-age=86400"},
    )


# In-process cache for the rendered sitemap. The dynamic mod/modpack sections
# enumerate the catalog (DB), and Cloudflare won't reliably edge-cache a
# generated .xml, so the body is memoised for a few minutes to keep crawler hits
# off the database. Flag/catalog changes reflect within the TTL.
_SITEMAP_TTL = 600.0
_SITEMAP_CACHE: dict[str, object] = {"body": None, "at": -1e9}
_SITEMAP_LOCK = asyncio.Lock()
# Per-section ceiling, well under the 50k-URL sitemap spec limit. If a section
# ever exceeds it we truncate and log rather than emit an oversized sitemap.
_SITEMAP_MAX_PER_SECTION = 25_000

# TEMP: individual mod detail pages (/mods/{handle}/{slug}) are excluded from the
# sitemap for now. The original reason - their above-the-fold content was
# JS-rendered, so as raw HTML they read thin - no longer applies: they now
# server-render title, author, description, tags and stats (see app/site/ssr.py
# ``mod_project_view``). What's left is purely a crawl-budget call: listing
# thousands of them while the core pages are still establishing indexing spends
# budget on the long tail. The hub pages (/mods, /mods/why) stay in via
# _SITEMAP_PAGES. Flip to True to re-list them once the core pages are indexed.
_SITEMAP_INCLUDE_MOD_PAGES = False


def _xml_loc(url: str) -> str:
    return url.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


async def _all_public_cards(list_fn, cap: int, label: str) -> list[dict]:
    """Page through a Mods-Hub ``list_public(limit, offset)`` and return every
    public card (carrying ``handle``/``slug``/``updated_at``). Truncates at
    ``cap`` with a warning - a silent cap would read as 'whole catalog indexed'
    when it isn't."""
    out: list[dict] = []
    offset, page = 0, 1000
    while True:
        rows, total = await list_fn(limit=page, offset=offset)
        out.extend(rows)
        offset += page
        if not rows or offset >= total or len(out) >= cap:
            break
    if len(out) > cap:
        logger.warning(
            "sitemap: %s catalog (%d) exceeds cap %d - truncating", label, len(out), cap)
        out = out[:cap]
    return out


async def _render_sitemap() -> str:
    """Build the sitemap XML: static feature pages (each gated by its master
    toggle) plus every public modpack page. Individual mod detail pages are
    currently excluded (see ``_SITEMAP_INCLUDE_MOD_PAGES``). The mod/modpack
    sections ride the Mods Hub master toggle, so they vanish wholesale when it's
    off. Approved strays are addressed as ``/mods/stray/<slug>`` - their card
    already carries ``handle='stray'``, so the generic URL shape is correct."""
    base = settings.app_url.rstrip("/")
    flags = {
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    }
    # (loc, lastmod-iso-or-None)
    entries: list[tuple[str, str | None]] = [
        (base + path, None)
        for path, attr in _SITEMAP_PAGES
        if attr is None or flags.get(attr, True)
    ]
    if flags.get("mods_hub_enabled", True):
        if _SITEMAP_INCLUDE_MOD_PAGES:
            mods = await _all_public_cards(
                mods_hub_service.list_public, _SITEMAP_MAX_PER_SECTION, "mods")
            entries += [
                (f"{base}/mods/{c['handle']}/{c['slug']}", c.get("updated_at"))
                for c in mods if c.get("handle") and c.get("slug")
            ]
        packs = await _all_public_cards(
            modpacks_service.list_public, _SITEMAP_MAX_PER_SECTION, "modpacks")
        entries += [
            (f"{base}/modpacks/{c['handle']}/{c['slug']}", c.get("updated_at"))
            for c in packs if c.get("handle") and c.get("slug")
        ]

    def _url(loc: str, lastmod: str | None) -> str:
        inner = f"<loc>{_xml_loc(loc)}</loc>"
        if lastmod:
            inner += f"<lastmod>{lastmod}</lastmod>"
        return f"  <url>{inner}</url>\n"

    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        + "".join(_url(loc, lm) for loc, lm in entries)
        + "</urlset>\n"
    )


@router.get("/sitemap.xml", include_in_schema=False)
async def sitemap_xml() -> Response:
    """XML sitemap of the public, indexable pages: the static feature pages plus
    every public modpack (individual mod pages are excluded for now - see
    ``_SITEMAP_INCLUDE_MOD_PAGES``). Cached in-process for a few minutes
    (``_SITEMAP_TTL``) so crawler hits don't re-enumerate the catalog each time."""
    now = time.monotonic()
    if _SITEMAP_CACHE["body"] is None or now - float(_SITEMAP_CACHE["at"]) > _SITEMAP_TTL:
        async with _SITEMAP_LOCK:
            now = time.monotonic()
            if _SITEMAP_CACHE["body"] is None or now - float(_SITEMAP_CACHE["at"]) > _SITEMAP_TTL:
                _SITEMAP_CACHE["body"] = await _render_sitemap()
                _SITEMAP_CACHE["at"] = now
    return Response(
        _SITEMAP_CACHE["body"], media_type="application/xml",
        headers={"Cache-Control": "public, max-age=3600"},
    )


@router.get("/documentation", response_class=HTMLResponse)
async def documentation(request: Request) -> HTMLResponse:
    """The user manual."""
    return _TEMPLATES.TemplateResponse(request, "docs.html", {})


@router.get("/swf-docs", response_class=HTMLResponse)
async def swf_docs(request: Request) -> HTMLResponse:
    """Hidden, unlisted reference: a markdown viewer with a grouped, searchable
    sidebar over the decompiled Trove Flash UI (``.swf``) docs. No nav/footer
    link and ``noindex``; the page shell loads its index + markdown from
    ``/static/swf-docs/*`` and renders client-side via the shared md renderer."""
    return _TEMPLATES.TemplateResponse(request, "swf-docs.html", {})


@router.get("/commands", response_class=HTMLResponse)
async def commands(request: Request) -> HTMLResponse:
    """In-game Trove slash-command reference. The command list is server-rendered
    from ``site/static/commands.json`` (English - the crawlable default) so the
    page is complete without JS; ``commands.js`` then hydrates and re-renders on
    language switch. See ``app/site/commands_page.py``."""
    return _TEMPLATES.TemplateResponse(
        request, "commands.html", {"cmd": commands_page.commands_view()},
    )


@router.get("/support", response_class=HTMLResponse)
async def support(request: Request) -> HTMLResponse:
    """'Support the project' page - landing for the red-heart navbar link (the
    floating widget is on every page). Renders the supporters credits list
    (managed via /admin/supporters)."""
    from app.supporters import service as supporters_service
    return _TEMPLATES.TemplateResponse(
        request, "support.html", {"supporters": await supporters_service.list_public()},
    )


@router.get("/status", response_class=HTMLResponse)
async def status_page(request: Request) -> HTMLResponse:
    """Dedicated Trove server-status page - live Live/PTS state plus a
    downtime-history timeline. Page shell + JS; data comes from
    ``/site/trove-status`` + ``/site/trove-status/history``."""
    return _TEMPLATES.TemplateResponse(
        request, "status.html", {"ssr": await ssr.status_view(_ssr_fetch)})


@router.get("/server-time", response_class=HTMLResponse)
async def server_time_page(request: Request) -> HTMLResponse:
    """Dedicated server-time page - a big live Trove server clock (UTC-11), the
    same instant across common player time zones, daily/weekly reset countdowns,
    and a Discord-timestamp maker. Page shell + JS; the clock anchors to
    ``/site/server-time`` (falling back to the local clock)."""
    return _TEMPLATES.TemplateResponse(request, "server-time.html", {})


@router.get("/calendar", response_class=HTMLResponse)
async def calendar_page(request: Request) -> HTMLResponse:
    """Live Trove calendar - every rotation and event on one board: the daily +
    weekly bonus, the Chaos Chest, the merchant/biome cycles (Corruxion, Fluxion,
    Wild Mana, Stampy, Shadow), and the ongoing/upcoming Trovesaurus events, each
    with a live countdown. Page shell + JS; data comes from ``/site/rotations``
    (shared with the homepage) + ``/site/calendar/events``."""
    return _TEMPLATES.TemplateResponse(
        request, "calendar.html", {"ssr": await ssr.calendar_view(_ssr_fetch)})


@router.get("/streams", response_class=HTMLResponse)
async def streams_page(request: Request) -> HTMLResponse:
    """Community hub - live Trove Twitch streams, recent YouTube videos, and the
    latest official news, all on one page. Page shell + JS; data comes from the
    shared ``/site/feeds/videos`` + ``/site/feeds/news`` proxies."""
    return _TEMPLATES.TemplateResponse(
        request, "streams.html", {"ssr": await ssr.streams_view(_ssr_fetch)})


@router.get("/releases", response_class=HTMLResponse)
async def releases_page(request: Request) -> HTMLResponse:
    """BetterTroveTools app releases + changelog. Latest build per platform
    (Windows/Linux/Android) with download links, the full release history, and the
    commit-grouped changelog. Page shell + JS; data comes from ``/site/btt/*``."""
    return _TEMPLATES.TemplateResponse(
        request, "releases.html", {"ssr": await ssr.releases_view(_ssr_fetch)})


@router.get("/classes", response_class=HTMLResponse)
async def classes(request: Request) -> HTMLResponse:
    """Trove class reference - a browsable codex of every class: base stats,
    weapons, damage type, its signature subclass (with the 1→30 level-scaling
    bonuses) and abilities. The picker + the first class's detail are
    server-rendered (English) so the page is complete without JS; classes.js
    fetches ``/site/stats/classes`` to power switching. See classes_page.py."""
    return _TEMPLATES.TemplateResponse(
        request, "classes.html", {"cls": classes_page.classes_view()},
    )


@router.get("/star-chart", response_class=HTMLResponse)
async def star_chart_page(request: Request) -> HTMLResponse:
    """Star Chart planner - an interactive radial builder for Trove's constellation
    star chart. Click through the nodes and the combined stats, abilities and
    rewards tally live; builds share by URL (``?b=<code>``) and by an ``SC:`` code
    that's byte-compatible with the desktop app. Fully client-rendered from the
    static ``/static/star_chart.json`` (no proxy, no /v1 API)."""
    return _TEMPLATES.TemplateResponse(request, "star-chart.html", {})


@router.get("/gems-guide", response_class=HTMLResponse)
async def gems_guide_page(request: Request) -> HTMLResponse:
    """How Gems Work - an interactive, animated explainer of Trove's gem system
    (tiers, elements incl. Cosmic/Light, Lesser vs Empowered, stat rolls,
    leveling/Power Rank and focusing). Fully client-rendered from the static
    ``/static/gems-guide.js`` - no proxy, no /v1 API."""
    return _TEMPLATES.TemplateResponse(request, "gems-guide.html", {})


@router.get("/dressing-room", response_class=HTMLResponse)
async def dressing_room_page(request: Request) -> HTMLResponse:
    """Dressing Room - build a Trove character out of the game's own parts: pick a
    class, a costume, a hat, a face and a weapon style and see the result assembled on
    that class's rig, with its animations. Client-rendered from the
    ``/site/dressing/*`` proxies; the whole outfit lives in the query string, so a look
    is shared by copying the URL and nothing is stored."""
    return _TEMPLATES.TemplateResponse(request, "dressing-room.html", {})


@router.get("/gem-simulator", response_class=HTMLResponse)
async def gem_simulator_page(request: Request) -> HTMLResponse:
    """Gem Simulator page. Fully client-rendered by the static
    ``/static/gem-engine.js`` (a JS port of the gem model), state in
    ``localStorage`` - no proxy, no /v1 API."""
    return _TEMPLATES.TemplateResponse(request, "gem-simulator.html", {})


@router.get("/gem-evaluator", response_class=HTMLResponse)
async def gem_evaluator_page(request: Request) -> HTMLResponse:
    """Gem Evaluator - type in a gem's tier / type / level and its three stats and
    get back the quality %, estimated Power Rank, a per-stat breakdown and the
    focus-material plan (Rough / Precise / Superior) to perfect it. Posts to the
    ``/site/gems/*`` proxies below."""
    return _TEMPLATES.TemplateResponse(request, "gem-evaluator.html", {})


@router.get("/gem-builds", response_class=HTMLResponse)
async def gem_builds_page(request: Request) -> HTMLResponse:
    """Gem Builds - pick a class / subclass / food / ally (plus optional star-chart
    code and buff toggles) and the optimizer ranks the top gem proc layouts by damage
    coefficient. Posts to the same-origin ``/site/gems/builds/*`` proxies."""
    return _TEMPLATES.TemplateResponse(request, "gem-builds.html", {})


@router.get("/calculators", response_class=HTMLResponse)
async def calculators_page(request: Request) -> HTMLResponse:
    """Calculators - Power Rank, Mastery, Magic Find and Light tabs. Client-rendered
    from static stat tables (``/static/assets/data/stats/*.json``); the Magic Find
    tab's optional star-chart preview uses the ``/site/gems/parse-star-chart`` proxy."""
    return _TEMPLATES.TemplateResponse(request, "calculators.html", {})


# /site/gems/* JSON proxies: mirror the read-side of /v1/gems/* (stateless compute).


@router.get("/site/gems/lookups", response_class=JSONResponse)
async def site_gem_lookups() -> JSONResponse:
    """Reference values (tiers, types, elements, stat types, augments) for the
    Gem Evaluator's dropdowns - same payload as ``/v1/gems/lookups``."""
    return JSONResponse(
        jsonable_encoder(gem_lookups()),
        headers={"Cache-Control": "public, max-age=3600"},
    )


@router.post("/site/gems/evaluate", response_class=JSONResponse)
async def site_gem_evaluate(req: EvaluateRequest) -> JSONResponse:
    """Score a typed-in gem (quality %, Power Rank, per-stat progress, focus plan).
    Same compute as ``/v1/gems/evaluate``."""
    try:
        out = gem_evaluator.evaluate_gem(
            req.tier, req.type, req.level,
            [s.model_dump() for s in req.stats], req.auto_guess_procs,
        )
    except gem_evaluator.GemEvaluatorError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    except (ValueError, KeyError, ZeroDivisionError) as e:
        raise HTTPException(status_code=400, detail=f"Could not evaluate gem: {e}") from e
    payload = {
        **out["result"],
        "available_extra_containers": out["available_extra_containers"],
        "guessed_distribution": out["guessed_distribution"],
    }
    return JSONResponse(jsonable_encoder(payload))


@router.post("/site/gems/evaluate-simple", response_class=JSONResponse)
async def site_gem_evaluate_simple(req: SimpleEvaluateRequest) -> JSONResponse:
    """Estimate a gem's quality from just its Power Rank. Same compute as
    ``/v1/gems/evaluate-simple``."""
    try:
        out = gem_evaluator.evaluate_gem_simple(req.tier, req.type, req.power_rank, req.level)
    except gem_evaluator.GemEvaluatorError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    except (ValueError, KeyError, ZeroDivisionError) as e:
        raise HTTPException(status_code=400, detail=f"Could not evaluate gem: {e}") from e
    return JSONResponse(jsonable_encoder(out))


@router.get("/site/gems/stat-range", response_class=JSONResponse)
async def site_gem_stat_range(
    tier: int, type: int, stat_type: int,
    level: int = Query(default=1, ge=1),
    extra_containers: int = Query(default=0, ge=0),
    element: int | None = None,
) -> JSONResponse:
    """Plausible (min, max) value a stat can roll at - for the evaluator's inline
    range hints. Same compute as ``/v1/gems/stat-range``."""
    try:
        return JSONResponse(
            jsonable_encoder(gem_evaluator.gem_stat_range(
                tier, type, stat_type, level, extra_containers, element)),
            headers={"Cache-Control": "public, max-age=3600"},
        )
    except (ValueError, KeyError) as e:
        raise HTTPException(status_code=400, detail=f"Invalid stat-range parameters: {e}") from e


@router.get("/site/gems/builds/options", response_class=JSONResponse)
async def site_gem_build_options() -> JSONResponse:
    """Valid field values for a build config (classes, allies, foods, flags) - same
    payload as ``/v1/gems/builds/options``. Static game data, cached hard."""
    return JSONResponse(
        jsonable_encoder(gem_builds.build_options()),
        headers={"Cache-Control": "public, max-age=3600"},
    )


@router.post("/site/gems/builds/calculate", response_class=JSONResponse)
async def site_gem_build_calculate(req: BuildConfigRequest) -> JSONResponse:
    """Top gem proc layouts for a build, ranked by damage coefficient. Same compute
    as ``/v1/gems/builds/calculate``; run off the event loop as it's a tight sync
    brute-force over the layout space."""
    try:
        results = await asyncio.to_thread(gem_builds.calculate_builds, req.model_dump())
    except gem_builds.BuildError as e:
        raise HTTPException(status_code=400, detail=str(e)) from e
    return JSONResponse(jsonable_encoder({"results": results, "count": len(results)}))


@router.get("/site/gems/parse-star-chart", response_class=JSONResponse)
async def site_gem_parse_star_chart(code: str = Query(default="", max_length=8192)) -> JSONResponse:
    """Decode a star-chart build code into aggregated passive stats for the Builds /
    Calculators live previews. Never errors: an unparseable code returns zero paths."""
    try:
        parsed = gem_builds.parse_star_chart(code)
    except Exception:  # noqa: BLE001 - preview only; bad codes must degrade, not 500
        parsed = {"stats": {}, "abilities": [], "paths_count": 0}
    return JSONResponse(jsonable_encoder(parsed))


@router.get("/site/stats/classes", response_class=JSONResponse)
async def site_stats_classes() -> JSONResponse:
    """Every Trove class as a full object for the /classes page - same data as
    ``/v1/stats/classes``. Static game data, cached hard."""
    return JSONResponse(
        jsonable_encoder(trove_stats.all_classes()),
        headers={"Cache-Control": "public, max-age=3600"},
    )


@router.get("/site/server-time", response_class=JSONResponse)
async def site_server_time() -> JSONResponse:
    """Authoritative Trove server time for the /server-time page - same payload as
    the public ``/v1/rotations/server-time``. Short cache; the page re-fetches each
    minute."""
    return JSONResponse(
        trove_server_time.server_time(),
        headers={"Cache-Control": "public, max-age=15"},
    )


def _merchant(mid: str, name: str, active: bool, starts_at, ends_at, **extra) -> dict:
    return {"id": mid, "name": name, "active": active,
            "starts_at": starts_at, "ends_at": ends_at, **extra}


def _biome_list(biomes: list[dict] | None) -> list[dict]:
    """Normalize biomes to ``{name, icon}`` so the dashboard can show each biome's
    icon (served from ``/static/assets/biomes/<icon>.png``) next to its name."""
    return [{"name": b.get("final_name") or b.get("name"), "icon": b.get("icon")}
            for b in (biomes or [])]


def _biomes(rot: dict | None) -> list[dict]:
    """The current biomes of a ``{current:{biomes:[...]}}`` rotation, as {name, icon}."""
    return _biome_list(((rot or {}).get("current") or {}).get("biomes"))


def _sched(rows: list[dict] | None, n: int = 6) -> list[dict]:
    """Normalize a merchant's upcoming windows (``schedule``/``upcoming``) into a
    compact list the dashboard modal can render: time window + optional state +
    optional biome names."""
    out: list[dict] = []
    for r in (rows or [])[:n]:
        entry = {"starts_at": r.get("starts_at"), "ends_at": r.get("ends_at")}
        if r.get("state"):
            entry["state"] = r["state"]
        if r.get("biomes"):
            entry["biomes"] = _biome_list(r["biomes"])
        out.append(entry)
    return out


def _daily_rotation(st: dict) -> list[dict]:
    """The full Mon→Sun daily-bonus rotation, each day flagged ``is_current`` and
    carrying ``next_at`` (unix) - when that day's window next begins. The current
    day's ``next_at`` is its window START (already active)."""
    db = trove_server_time.daily_buffs()
    week = db.get("week") or []
    cur = db.get("current") or {}
    cur_idx = next((i for i, x in enumerate(week) if x.get("name") == cur.get("name")), 0)
    reset = st.get("daily_reset_at")
    out = []
    for i, day in enumerate(week):
        d = (i - cur_idx) % 7
        out.append({
            "name": day.get("name"), "emoji": day.get("emoji"), "color": day.get("color"),
            "weekday": day.get("weekday"), "normal_buffs": day.get("normal_buffs"),
            "premium_buffs": day.get("premium_buffs"), "banner": day.get("banner"),
            "is_current": d == 0, "next_at": (reset + (d - 1) * 86400) if reset else None,
        })
    return out


def _weekly_rotation(st: dict) -> list[dict]:
    """The full 4-week weekly-bonus rotation, each week flagged ``is_current`` and
    carrying ``next_at`` (unix) - when that week next begins."""
    wb = trove_server_time.weekly_buffs()
    rotation = wb.get("rotation") or []
    cur = wb.get("current") or {}
    cur_idx = next((i for i, x in enumerate(rotation) if x.get("name") == cur.get("name")), 0)
    reset = st.get("weekly_reset_at")
    out = []
    for j, wk in enumerate(rotation):
        d = (j - cur_idx) % 4
        out.append({
            "name": wk.get("name"), "emoji": wk.get("emoji"), "color": wk.get("color"),
            "buffs": wk.get("buffs"), "banner": wk.get("banner"),
            "is_current": d == 0, "next_at": (reset + (d - 1) * 7 * 86400) if reset else None,
        })
    return out


@router.get("/site/rotations", response_class=JSONResponse)
async def site_rotations() -> JSONResponse:
    """"Today in Trove" payload for the homepage dashboard: server time + resets,
    today's daily + this week's weekly bonus, the Chaos Chest window, and the
    live merchant / biome rotations (Corruxion, Fluxion, Wild Mana, Stampy, the
    3-hour biome cycle). Reuses the /v1 rotations compute functions."""
    mana = trove_rotations.wild_mana()
    stampy = trove_rotations.stampy()
    d15 = trove_rotations.biome_rotation()
    corr = trove_server_time.corruxion()
    flux = trove_server_time.fluxion()
    # Luxion is CAPTURED (not computed): a fixed 7-day run whose start is dev-set.
    # Show it Here/Away at the event level - "Leaves in Nd" while the run is on;
    # no countdown when Away (the next appearance is unpredictable until seen
    # in-game). The daily 3-hour windows go in the schedule modal.
    lux = await trove_luxion.get_luxion()
    # While Luxion is here, expose the FULL weekly rotation (all 7 daily windows,
    # each labelled Day 1..7) so the dashboard/calendar modal shows the whole run
    # with the current window highlighted; nothing when it's away.
    lux_sched = _sched(lux.get("schedule"), n=7) if lux["active"] else []
    # The card also needs the daily 3-hour window: whether the merchant is open
    # right now (→ when it closes) or, between windows, when it next opens.
    _lw = lux.get("current_window") or lux.get("next_window")
    lux_window = (
        {"open": bool(lux.get("merchant_open")),
         "starts_at": _lw.get("starts_at"), "ends_at": _lw.get("ends_at")}
        if (lux["active"] and _lw) else None
    )
    stampy_cur = stampy.get("current")
    merchants = [
        _merchant("corruxion", "Corruxion", corr["active"], corr["starts_at"], corr["ends_at"],
                  schedule=_sched(corr.get("schedule"))),
        _merchant("fluxion", "Fluxion", flux["active"], flux["starts_at"], flux["ends_at"],
                  state=flux.get("state"), schedule=_sched(flux.get("schedule"))),
        _merchant("luxion", "Luxion", lux["active"],
                  lux["starts_at"] if lux["active"] else None,
                  lux["ends_at"] if lux["active"] else None,
                  state=("Open" if lux.get("merchant_open") else None),
                  schedule=lux_sched, window=lux_window),
        _merchant("wild_mana", "Wild Mana", True,
                  (mana.get("current") or {}).get("starts_at"),
                  (mana.get("current") or {}).get("ends_at"), biomes=_biomes(mana),
                  schedule=_sched(mana.get("upcoming"))),
        _merchant("d15", "Long Shade Rotation", True,
                  (d15.get("current") or {}).get("starts_at"),
                  (d15.get("current") or {}).get("ends_at"), biomes=_biomes(d15),
                  schedule=_sched(d15.get("upcoming"))),
    ]
    if stampy_cur:
        merchants.append(_merchant(
            "stampy", "Stampy", True, stampy_cur.get("starts_at"), stampy_cur.get("ends_at"),
            biomes=_biome_list(stampy_cur.get("biomes")),
            schedule=_sched(stampy.get("upcoming"))))
    # Chaos Chest: window + the current featured item (name, identifier and the
    # blueprint the card/modal draws its icon from, via /site/codexes/render).
    # Built into a clean dict so the datetime ``fetched_at`` the source carries
    # never reaches the JSON serializer.
    cc = await trove_chaos.get_chaos_chest()
    chaos = {
        "starts_at": cc.get("starts_at"), "ends_at": cc.get("ends_at"),
        "seconds_remaining": cc.get("seconds_remaining"), "item": cc.get("item"),
    }
    st = trove_server_time.server_time()
    return JSONResponse(
        {
            "server_time": st,
            "daily_buff": trove_server_time.daily_buffs().get("current") or None,
            "weekly_buff": trove_server_time.weekly_buffs().get("current") or None,
            "daily_rotation": _daily_rotation(st),
            "weekly_rotation": _weekly_rotation(st),
            "chaos": chaos,
            "merchants": merchants,
        },
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/feeds/news", response_class=JSONResponse)
async def site_feeds_news(limit: int = Query(default=16, ge=1, le=50)) -> JSONResponse:
    """Latest Trove news for the homepage dashboard - same data as
    ``/v1/feeds/news`` (relayed from trovegame.com)."""
    docs = await trove_news.latest_news(limit)
    items = [
        {"title": d.title, "url": d.url, "author": d.author, "summary": d.summary,
         "category": d.category, "categories": d.categories, "image": d.image,
         "published_at": d.published_at}
        for d in docs
    ]
    return JSONResponse(
        jsonable_encoder({"items": items}), headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/feeds/videos", response_class=JSONResponse)
async def site_feeds_videos(platform: str = Query(default="youtube")) -> JSONResponse:
    """Trove community videos/streams (YouTube or Twitch) for the dashboard -
    same source as ``/v1/feeds/{youtube,twitch}``."""
    platform = platform if platform in ("youtube", "twitch") else "youtube"
    items, fetched_at = await trove_feeds.get_feed(platform)
    return JSONResponse(
        jsonable_encoder({"platform": platform, "items": items, "fetched_at": fetched_at}),
        headers={"Cache-Control": "public, max-age=300"},
    )


def _calendar_event(ev: TroveEvent, now: int) -> dict:
    """A Trovesaurus event flattened for the /calendar board: status + countdown
    derived from now vs the start/end window (same rule as /v1/feeds/events)."""
    if now < ev.starts_at:
        status, seconds_until = "upcoming", ev.starts_at - now
    elif now < ev.ends_at:
        status, seconds_until = "ongoing", ev.ends_at - now
    else:
        status, seconds_until = "ended", 0
    return {
        "event_id": ev.event_id, "name": ev.name, "url": ev.url, "category": ev.category,
        "image": ev.image, "icon": ev.icon, "starts_at": ev.starts_at, "ends_at": ev.ends_at,
        "status": status, "seconds_until": seconds_until,
    }


@router.get("/site/calendar/events", response_class=JSONResponse)
async def site_calendar_events() -> JSONResponse:
    """Ongoing + upcoming Trovesaurus events for the /calendar page - same data as
    ``/v1/feeds/events`` + ``/v1/feeds/events/upcoming``. Ongoing end soonest first;
    upcoming start soonest first."""
    now = int(time.time())
    ongoing = await TroveEvent.find(
        {"starts_at": {"$lte": now}, "ends_at": {"$gt": now}}
    ).sort("ends_at").limit(100).to_list()
    upcoming = await TroveEvent.find(
        {"starts_at": {"$gt": now}}
    ).sort("starts_at").limit(100).to_list()
    return JSONResponse(
        jsonable_encoder({
            "ongoing": [_calendar_event(e, now) for e in ongoing],
            "upcoming": [_calendar_event(e, now) for e in upcoming],
            "now": now,
        }),
        headers={"Cache-Control": "public, max-age=120"},
    )


@router.get("/site/calendar/yearly", response_class=JSONResponse)
async def site_calendar_yearly() -> JSONResponse:
    """The full +/-365-day rotation timeline for the homepage yearly-calendar
    widget: weekly buffs, Corruxion/Fluxion, gardening windows, Wild Mana and
    Stampy, plus any recorded Luxion runs, as one flat, start-sorted list. Same
    compute as ``/v1`` rotations calendar - tokenless and long-cached (everything
    but Luxion is deterministic; Luxion shows only known past/current runs)."""
    luxion_runs = await trove_captures.list_luxion_starts()
    return JSONResponse(
        trove_calendar.yearly_calendar(luxion_runs=luxion_runs),
        headers={"Cache-Control": "public, max-age=300"},
    )


def _btt_release(d) -> dict:
    """A stored BttRelease flattened for the /releases page (channel derived from
    the GitHub prerelease flag; assets kept verbatim)."""
    return {
        "tag_name": d.tag_name, "name": d.name, "body": d.body, "html_url": d.html_url,
        "channel": "beta" if d.prerelease else "release", "prerelease": d.prerelease,
        "published_at": d.published_at, "assets": d.assets,
    }


@router.get("/site/btt/latest", response_class=JSONResponse)
async def site_btt_latest(channel: str = Query(default="release")) -> JSONResponse:
    """Latest BetterTroveTools build per platform (windows/linux/android) on a
    channel, for the /releases hero. Each platform walks back independently to the
    most recent release that ships an asset for it (same logic as /v1/btt/latest)."""
    channel = channel if channel in trove_btt.CHANNELS else "release"
    per = await trove_btt.latest_per_platform(channel)
    platforms: dict[str, dict | None] = {}
    for platform, found in per.items():
        if found is None:
            platforms[platform] = None
            continue
        release, matched = found
        platforms[platform] = {
            "platform": platform, "tag_name": release.tag_name,
            "published_at": release.published_at, "html_url": release.html_url,
            "assets": matched,
        }
    return JSONResponse(
        jsonable_encoder({"channel": channel, "platforms": platforms}),
        headers={"Cache-Control": "public, max-age=180"},
    )


@router.get("/site/btt/releases", response_class=JSONResponse)
async def site_btt_releases(
    channel: str | None = Query(default=None),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """BetterTroveTools release history (newest first) for the /releases list -
    same data as ``/v1/btt/releases``. Optional channel filter."""
    channel = channel if channel in trove_btt.CHANNELS else None
    docs, total = await trove_btt.list_releases(channel, limit, offset)
    return JSONResponse(
        jsonable_encoder({
            "channel": channel, "items": [_btt_release(d) for d in docs],
            "count": len(docs), "total": total,
        }),
        headers={"Cache-Control": "public, max-age=180"},
    )


@router.get("/site/btt/changelog", response_class=JSONResponse)
async def site_btt_changelog() -> JSONResponse:
    """The commit-grouped BetterTroveTools changelog for the /releases page - same
    data as ``/v1/btt/changelog``."""
    doc = await trove_btt.get_changelog()
    payload = {"groups": doc.groups if doc else [],
               "rate_limited": bool(doc.rate_limited) if doc else False,
               "fetched_at": doc.fetched_at if doc else None}
    return JSONResponse(
        jsonable_encoder(payload), headers={"Cache-Control": "public, max-age=180"})


@router.get("/terms", response_class=HTMLResponse)
async def terms(request: Request) -> HTMLResponse:
    """Terms of Service - reachable from the footer fine print (no navbar link)."""
    return _TEMPLATES.TemplateResponse(request, "terms.html", {})


@router.get("/privacy", response_class=HTMLResponse)
async def privacy(request: Request) -> HTMLResponse:
    """Privacy Policy - reachable from the footer fine print (no navbar link)."""
    return _TEMPLATES.TemplateResponse(request, "privacy.html", {})


@router.get("/accessibility", response_class=HTMLResponse)
async def accessibility(request: Request) -> HTMLResponse:
    """Accessibility statement - reachable from the footer fine print."""
    return _TEMPLATES.TemplateResponse(request, "accessibility.html", {})


@router.get("/status/og.png")
async def status_og_image(lang: str = "en") -> Response:
    """OG / Twitter card image: the live EU/US/PTS server-status card as a
    1200x630 PNG so a shared ``/status`` link previews the current state.
    Cached ~45s; falls back to the favicon if a render ever fails."""
    from app.site import og_image
    try:
        png = await og_image.render_status_og(lang)
    except Exception:  # noqa: BLE001 - never let a render error break the card
        logger.exception("status OG image render failed")
        return RedirectResponse("/static/assets/favicon.png", status_code=302)
    return Response(
        content=png, media_type="image/png",
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/board.png")
async def board_image(v: str | None = None, lang: str = "en") -> Response:
    """The live 'Trove Now' board as a 1200x630 PNG - the image the Discord bot's
    board feature embeds. Rendered at most once per minute and cached in Redis, so
    100 guilds (and every API worker) share a single render. ``v`` is the bot's
    per-minute cache-buster (so Discord refetches each minute); the render itself
    is keyed by the minute server-side. Falls back to the favicon on a render error."""
    from app.site import og_image
    try:
        png = await og_image.render_board_image(lang)
    except Exception:  # noqa: BLE001 - never let a render error break the board
        logger.exception("board image render failed")
        return RedirectResponse("/static/assets/favicon.png", status_code=302)
    return Response(
        content=png, media_type="image/png",
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/announce.png")
async def announcement_image(kind: str, v: str | None = None, lang: str = "en") -> Response:
    """A single announcement's banner PNG (used by the Discord bot's image
    announcements). Rendered once per minute and cached in Redis per (kind, minute),
    so 100 guilds share one render; ``v`` is the bot's countdown-scaled cache-buster.
    Falls back to the favicon on a render error."""
    from app.site import og_image
    try:
        png = await og_image.render_announcement_image(kind, lang)
    except Exception:  # noqa: BLE001 - never let a render error break the announcement
        logger.exception("announcement image render failed for %s", kind)
        return RedirectResponse("/static/assets/favicon.png", status_code=302)
    return Response(
        content=png, media_type="image/png",
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/login", response_class=HTMLResponse)
async def login(request: Request) -> HTMLResponse:
    """Public-facing sign-in page. Discord-only - the page just hosts the
    "Sign in with Discord" button and finishes the OAuth round-trip. Auth
    backend lives at /v1/site-auth/* - see app/site_auth/."""
    return _TEMPLATES.TemplateResponse(
        request, "login.html",
        {"discord_oauth_enabled": settings.discord_oauth_enabled},
    )


@router.get("/dashboard", response_class=HTMLResponse)
async def dashboard(request: Request) -> HTMLResponse:
    """Logged-in user dashboard. Client-side checks for a stored token
    and redirects to /login if absent - no server-side gate so the
    page can serve same-origin caches without varying on auth."""
    return _TEMPLATES.TemplateResponse(request, "dashboard.html", {})


@router.get("/market", response_class=HTMLResponse)
async def market(request: Request) -> HTMLResponse:
    """In-game marketplace browser (Beta). Reads the ``market_listings``
    collection via the /site/market/* proxies below."""
    return _TEMPLATES.TemplateResponse(
        request, "market.html", {"ssr": await ssr.market_view(_ssr_fetch)})


@router.get("/store", response_class=HTMLResponse)
async def store(request: Request) -> HTMLResponse:
    """Trove Store History (Beta): the in-game cash-shop catalog with per-pack
    availability timelines, price history and in-game art. Reads the store
    collections via the /site/store/* proxies below."""
    return _TEMPLATES.TemplateResponse(
        request, "store.html", {"ssr": await ssr.store_view(_ssr_fetch)})


@router.get("/codexes", response_class=HTMLResponse)
async def codexes(request: Request) -> HTMLResponse:
    """Codexes browser - parsed Trove game data (allies, mounts, dragons, mementos,
    recipes, items, fish, badges) with mastery / power rank / stat & ability bonuses.
    Reads ``/v1/codexes/*`` via the ``/site/codexes/*`` proxies below."""
    return _TEMPLATES.TemplateResponse(
        request, "codexes.html", {"ssr": await ssr.codexes_view(_ssr_fetch)})


@router.get("/codexes/crafting", response_class=HTMLResponse)
async def codexes_crafting_page(request: Request) -> HTMLResponse:
    """Recipe Cost Calculator - pick a craftable item and see its full crafting
    dependency tree with market prices rolled up from the leaves, plus a
    craft-vs-buy recommendation. Data comes from the codex recipe index joined to
    the market scope via ``/site/codexes/crafting`` below."""
    return _TEMPLATES.TemplateResponse(request, "codexes-crafting.html", {})


@router.get("/mods", response_class=HTMLResponse)
async def mods_hub(request: Request) -> HTMLResponse:
    """Mods Hub - browse + download shared Trove mods (public, no login). The
    grid + search are painted client-side from the ``/site/mods/*`` proxies
    below; creating/developing a mod needs a signed-in site user."""
    return _TEMPLATES.TemplateResponse(
        request, "mods.html", {"ssr": await ssr.mods_view(_ssr_fetch)})


# NOTE: must stay ABOVE the ``/mods/{handle}`` routes below - Starlette matches in
# definition order, so this static path has to win over the handle param. "why" is
# also a RESERVED_USERNAME so no modder's profile can ever shadow it.
@router.get("/mods/why", response_class=HTMLResponse)
async def mods_why(request: Request) -> HTMLResponse:
    """"Why Mods Hub" - a hidden explainer page (not in the nav) linked from the
    ``/mods`` hero. Sells what the hub offers players + modders. Static content."""
    return _TEMPLATES.TemplateResponse(request, "mods_why.html", {})


def _plain_excerpt(md: str | None, limit: int = 280) -> str:
    """Crude markdown/HTML → plain text for a meta description."""
    t = re.sub(r"<[^>]+>", " ", md or "")            # strip HTML tags
    t = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", t)  # links/images -> their text
    t = re.sub(r"[#*`_>~|]", "", t)                   # strip md markers
    t = re.sub(r"\s+", " ", t).strip()
    return t[:limit]


@router.get("/mods/{handle}/{slug}", response_class=HTMLResponse)
async def mods_project_page(request: Request, handle: str, slug: str) -> HTMLResponse:
    """A single mod's page: banner, previews, description, releases (with
    download) and the file/commit browser. The owner (when logged in) also
    gets the inline studio controls. Addressed as ``/mods/<owner_handle>/<slug>``;
    all data comes from ``/site/mods/*``.

    The page is client-rendered, but we fetch the mod here (anonymously) to emit
    real Open Graph / Twitter-card tags, so link unfurls (Discord, Twitter, …) show
    the actual mod - title, summary and banner. Drafts / unlisted-but-private /
    not-found fall back to generic tags so nothing private leaks into an embed; the
    page itself still renders (the client reveals owner-only content when logged in)."""
    base = settings.app_url.rstrip("/")
    page_url = f"{base}/mods/{handle}/{slug}"
    ctx: dict = {
        "slug": slug, "handle": handle, "og_page_url": page_url,
        "page_title": f"{slug} · Trove mod · Better Trove Tools",
        "og_title": f"{slug} · Trove mod",
        "og_desc": "A Trove mod shared on the Better Trove Tools Mods Hub.",
        "og_image": f"{base}/static/assets/favicon.png",
        "og_image_alt": "Better Trove Tools",
        "og_author": "",
        "twitter_card": "summary",
    }
    project = await mods_hub_service.get_project(handle, slug)
    if project is not None and mods_hub_service.can_view(project, None):
        desc = (project.summary or "").strip() or _plain_excerpt(project.description) \
            or f"A Trove mod by {project.owner_username}."
        img_sha = project.banner_sha or (project.preview_shas[0] if project.preview_shas else None)
        ctx.update({
            "page_title": f"{project.title} · Trove mod · Better Trove Tools",
            "og_title": f"{project.title} · Trove mod",
            "og_desc": desc[:300],
            "og_image": f"{base}/site/mods/image/{img_sha}" if img_sha else ctx["og_image"],
            "og_image_alt": project.title,
            "og_author": project.owner_username,
            "twitter_card": "summary_large_image" if img_sha else "summary",
        })
        # Same document, no extra query: the mod's title, description, tags and
        # stats become server-rendered body copy, not just meta tags.
        ctx["ssr"] = ssr.mod_project_view(
            await mods_hub_service.project_detail(project, None))
    return _TEMPLATES.TemplateResponse(request, "mods_project.html", ctx)


@router.get("/mods/{handle}", response_class=HTMLResponse)
async def mods_profile_page(request: Request, handle: str) -> HTMLResponse:
    """A modder's profile page (`/mods/<handle>`): avatar, banner, README, socials
    and their mods. Client-rendered from ``/site/mods/profile/<handle>``; this route
    fills per-modder Open Graph tags so a shared profile link unfurls properly.

    A profile only exists once the modder has ≥1 public mod, so this 404s otherwise
    (the front-facing 404 handler serves the themed HTML page)."""
    base = settings.app_url.rstrip("/")
    page_url = f"{base}/mods/{handle}"
    data = await mods_hub_service.profile_view(handle, None)
    if data is None:
        raise HTTPException(status_code=404, detail="No such modder.")
    ctx: dict = {
        "handle": handle, "og_page_url": page_url,
        "page_title": f"{handle} · Trove modder · Better Trove Tools",
        "og_title": f"{handle} · Trove modder",
        "og_desc": f"{handle}'s mods on the Better Trove Tools Mods Hub.",
        "og_image": f"{base}/static/assets/favicon.png",
        "og_image_alt": handle,
        "og_author": "",
        "twitter_card": "summary",
    }
    name = data["display_name"]
    desc = (data["tagline"] or "").strip() or _plain_excerpt(data["readme"]) \
        or f"{name}'s mods on the Better Trove Tools Mods Hub."
    img = data["banner_url"] or data["avatar_url"]
    ctx.update({
        "page_title": f"{name} · Trove modder · Better Trove Tools",
        "og_title": f"{name} · Trove modder",
        "og_desc": desc[:300],
        "og_image": img or ctx["og_image"],
        "og_image_alt": name,
        "og_author": name,
        "twitter_card": "summary_large_image" if data["banner_url"] else "summary",
        "ssr": ssr.mod_profile_view(data),
    })
    return _TEMPLATES.TemplateResponse(request, "mods_profile.html", ctx)


@router.get("/modpacks", response_class=HTMLResponse)
async def modpacks_hub(request: Request) -> HTMLResponse:
    """Modpacks - browse + download user-curated bundles of hub mods (public, no
    login). Grid painted client-side from ``/site/modpacks/*``; creating one needs
    a signed-in site user."""
    return _TEMPLATES.TemplateResponse(
        request, "modpacks.html", {"ssr": await ssr.modpacks_view(_ssr_fetch)})


@router.get("/modpacks/{handle}/{slug}", response_class=HTMLResponse)
async def modpack_project_page(request: Request, handle: str, slug: str) -> HTMLResponse:
    """A single modpack's page: banner, description, variants and the mods each
    bundles (with version per mod), plus download. Owner gets the inline editor.
    Client-rendered from ``/site/modpacks/*``; we fetch it here (anonymously) to
    emit real Open Graph / Twitter-card tags for link unfurls. Drafts / private /
    not-found fall back to generic tags so nothing private leaks into an embed."""
    base = settings.app_url.rstrip("/")
    page_url = f"{base}/modpacks/{handle}/{slug}"
    ctx: dict = {
        "slug": slug, "handle": handle, "og_page_url": page_url,
        "page_title": f"{slug} · Trove modpack · Better Trove Tools",
        "og_title": f"{slug} · Trove modpack",
        "og_desc": "A Trove modpack shared on Better Trove Tools.",
        "og_image": f"{base}/static/assets/favicon.png",
        "og_image_alt": "Better Trove Tools",
        "og_author": "",
        "twitter_card": "summary",
    }
    pack = await modpacks_service.get_pack(handle, slug)
    if pack is not None and modpacks_service.can_view(pack, None):
        desc = (pack.summary or "").strip() or _plain_excerpt(pack.description) \
            or f"A Trove modpack by {pack.owner_username}."
        img_sha = pack.banner_sha or (pack.preview_shas[0] if pack.preview_shas else None)
        ctx.update({
            "page_title": f"{pack.title} · Trove modpack · Better Trove Tools",
            "og_title": f"{pack.title} · Trove modpack",
            "og_desc": desc[:300],
            "og_image": f"{base}/site/mods/image/{img_sha}" if img_sha else ctx["og_image"],
            "og_image_alt": pack.title,
            "og_author": pack.owner_username,
            "twitter_card": "summary_large_image" if img_sha else "summary",
        })
        ctx["ssr"] = ssr.modpack_project_view(
            await modpacks_service.pack_detail(pack, None))
    return _TEMPLATES.TemplateResponse(request, "modpacks_project.html", ctx)


@router.get("/giveaways", response_class=HTMLResponse)
async def giveaways(request: Request) -> HTMLResponse:
    """Public giveaways page. Lists open / upcoming / past draws (data from
    the /site/giveaways proxy); entering needs a signed-in site user."""
    return _TEMPLATES.TemplateResponse(
        request, "giveaways.html", {"ssr": await ssr.giveaways_view(_ssr_fetch)})


@router.get("/clubs", response_class=HTMLResponse)
async def clubs_page_view(request: Request) -> HTMLResponse:
    """Public clubs directory - clubs marked public in the Discord dashboard,
    ordered by their rank on the in-game club leaderboard (board 1100)."""
    from app.site import clubs_page
    return _TEMPLATES.TemplateResponse(
        request, "clubs.html", {"clubs": await clubs_page.public_clubs_ordered()},
    )


# Period-keyed social cards: a shared `/activity?period=1y` link previews the
# 1Y graph. `period` MUST be a query param (or path) - URL #fragments never
# reach the server/scrapers, so they can't drive a per-period embed.
_OG_PERIODS = ("1d", "7d", "1m", "3m", "6m", "1y", "all")
_OG_PERIOD_LABEL = {
    "1d": "Last 24 hours", "7d": "Last 7 days", "1m": "Last 30 days",
    "3m": "Last 3 months", "6m": "Last 6 months", "1y": "Last 12 months",
    "all": "All time",
}


@router.get("/activity", response_class=HTMLResponse)
async def activity_page(request: Request, period: str | None = None) -> HTMLResponse:
    """Player Activity page - the live active-player pulse plus multi-period
    trend charts (1D … all-time). An optional ``?period=`` selects the graph
    AND drives the OG/Twitter card so each period previews its own chart."""
    p = (period or "").lower()
    p = p if p in _OG_PERIODS else ""        # "" = default (bare URL → 1d card)
    qs = f"?period={p}" if p else ""
    label = _OG_PERIOD_LABEL.get(p)
    title = f"Trove Player Activity · {label}" if label else "Trove Player Activity"
    desc = (f"Live active-player count over {label.lower()}, from the leaderboard "
            "captures." if label
            else "Live active-player estimate and trend charts (1D to all-time), "
                 "from the leaderboard captures.")
    return _TEMPLATES.TemplateResponse(request, "activity.html", {
        "og_title": title,
        "og_desc": desc,
        "og_image_url": f"https://trove.aallyn.net/activity/og.png{qs}",
        "og_page_url": f"https://trove.aallyn.net/activity{qs}",
        "ssr": await ssr.activity_view(_ssr_fetch),
    })


@router.get("/class-activity", response_class=HTMLResponse)
async def class_activity_page(request: Request) -> HTMLResponse:
    """Class Activity page - per-class active players over time (multi-line) plus
    a class player-share donut, derived from the Effort/Paragon leaderboards."""
    return _TEMPLATES.TemplateResponse(
        request, "class-activity.html",
        {"ssr": await ssr.class_activity_view(_ssr_fetch)})


@router.get("/player/{name}", response_class=HTMLResponse)
async def player_page(request: Request, name: str) -> HTMLResponse:
    """Public player profile - leaderboard appearances + a verified-claim badge.
    Shareable; the Discord bot's rank command can deep-link here. The page fetches
    /site/leaderboards/players/<name>/profile client-side."""
    title = f"{name} · Trove player profile"
    return _TEMPLATES.TemplateResponse(request, "player.html", {
        "player_name": name,
        "og_title": title,
        "og_desc": f"{name}'s Trove leaderboard ranks and recent appearances.",
        "og_page_url": f"https://trove.aallyn.net/player/{name}",
        "ssr": await ssr.player_view(_ssr_fetch, name),
    })


@router.get("/activity/og.png")
async def activity_og_image(period: str = "1d") -> Response:
    """OG / Twitter card image: the activity chart for ``period`` (default 1d)
    rendered to a 1200x630 PNG so a shared ``/activity?period=…`` link previews
    that graph. Cached in-process per period; falls back to the favicon if a
    render ever fails so the meta tag never 500s."""
    from app.site import og_image
    try:
        png = await og_image.render_activity_og(period)
    except Exception:  # noqa: BLE001 - never let a render error break the card
        logger.exception("activity OG image render failed")
        return RedirectResponse("/static/assets/favicon.png", status_code=302)
    return Response(
        content=png, media_type="image/png",
        headers={"Cache-Control": "public, max-age=600"},
    )


# --- /site/market/* JSON proxies for the /market page ---------------------

@router.get("/site/market/items", response_class=JSONResponse)
async def site_market_items() -> JSONResponse:
    """Item names with at least one stored listing, plus the admin-defined
    sidebar categories (ordered ``[{name, items}]``). Categories may reference
    names outside ``items`` (not currently trading / off the allow-list) - the
    client intersects, so membership survives allow-list churn server-side.
    ``untracked`` is the subset of ``items`` no longer on the scan allow-list
    (listings stored, updates stopped) - rendered as a system "Untracked"
    group with a warning instead of their category."""
    from app.trove.market import service as market_service
    items = await market_service.list_distinct_items()
    categories = await market_service.categories_public()
    untracked = await market_service.untracked_items(items)
    return JSONResponse(
        {"items": items, "count": len(items), "categories": categories,
         "untracked": untracked},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/market/item-images", response_class=JSONResponse)
async def site_market_item_images() -> JSONResponse:
    """``{name: blueprint}`` map for the market items that we can pin to a codex
    model, so the /market page can show a thumbnail per item via the existing
    ``/site/codexes/render`` endpoint. Market listings carry only a display name,
    so we reverse it through the codex (name -> blueprint); names we can't resolve
    unambiguously are simply omitted (no thumbnail rather than a wrong one). Best-
    effort: a codex/DB hiccup degrades to an empty map, never a broken item list."""
    from app.trove.market import service as market_service
    images: dict[str, str] = {}
    try:
        names = await market_service.list_distinct_items()
        if names:
            resolved = await codexes_read.blueprints_for_names(_DEFAULT_CODEX_BRANCH, names)
            # Re-key from lower(name) back onto the real market names the client uses.
            for name in names:
                bp = resolved.get(name.lower())
                if bp:
                    images[name] = bp
    except Exception:  # noqa: BLE001 - thumbnails are cosmetic; never break /market
        logger.exception("market: item-image resolve failed")
    return JSONResponse(
        {"images": images, "branch": _DEFAULT_CODEX_BRANCH, "count": len(images)},
        headers={"Cache-Control": "public, max-age=300"},
    )


# ── Store History proxies (same-origin, tokenless) ────────────────────────
# Back the /store page. All read from app.trove.store.service (Mongo store_*
# collections). The catalog changes at most daily, so caches are generous.

_STORE_TEXTURE_BRANCH = "live-us"


@router.get("/site/store/products", response_class=JSONResponse)
async def site_store_products(
    category: int | None = Query(default=None),
    kind: str | None = Query(default=None),
    currency: str | None = Query(default=None),
    q: str | None = Query(default=None),
    active: bool = Query(default=True),
    on_sale: bool = Query(default=False),
    limit: int = Query(default=500, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """Paginated store catalog for the Gallery tab."""
    from app.trove.store import service as store_service
    items, total, anchor = await store_service.list_products(
        category=category, kind=kind, currency=currency, q=q,
        active_only=active, on_sale=on_sale, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"items": items, "count": len(items), "total": total, "anchor": anchor},
        headers={"Cache-Control": "public, max-age=120"},
    )


@router.get("/site/store/categories", response_class=JSONResponse)
async def site_store_categories() -> JSONResponse:
    """Store tab list (label, icon, display-ordered codes) for the sidebar."""
    from app.trove.store import service as store_service
    items, anchor = await store_service.list_categories()
    return JSONResponse(
        {"items": items, "count": len(items), "anchor": anchor},
        headers={"Cache-Control": "public, max-age=120"},
    )


@router.get("/site/store/timeline", response_class=JSONResponse)
async def site_store_timeline(
    kind: str | None = Query(default=None),
    limit: int = Query(default=1000, ge=1, le=4000),
) -> JSONResponse:
    """Availability bands for every product - the History tab's data source."""
    from app.trove.store import service as store_service
    payload = await store_service.timeline(kind=kind, limit=limit)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/store/products/{code}", response_class=JSONResponse)
async def site_store_product(code: str) -> JSONResponse:
    """One product + price history + availability + records (detail drill-in)."""
    from app.trove.store import service as store_service
    payload = await store_service.get_product(code)
    if payload is None:
        raise HTTPException(status_code=404, detail="No such store product")
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/store/texture")
async def site_store_texture(
    path: str = Query(..., description="Game texture path, e.g. ui/store/foo.dds"),
) -> Response:
    """Raw bytes of an in-game texture from the updates CAS, resolved
    case-insensitively (store paths can drift from the manifest). Same-origin
    + tokenless so the /store page can `<img>`/decodeDDS them without a token,
    and independent of ``feature_updates_enabled`` (store owns this surface).
    404 when the branch has no such file (client falls back to a placeholder)."""
    from app.trove.store import service as store_service
    from app.trove.updates.cas import ContentStore

    sha = await store_service.resolve_texture_sha(path, _STORE_TEXTURE_BRANCH)
    if sha is None:
        raise HTTPException(status_code=404, detail="texture not found")
    blob = ContentStore(settings.trove_update_store_dir).path_for(sha)
    if not blob.is_file():
        raise HTTPException(status_code=404, detail="blob missing")
    data = await asyncio.to_thread(blob.read_bytes)
    # .dds decodes client-side; browser-native images pass straight through.
    ext = path.rsplit(".", 1)[-1].lower() if "." in path else ""
    media = {"png": "image/png", "jpg": "image/jpeg", "jpeg": "image/jpeg",
             "gif": "image/gif", "webp": "image/webp"}.get(ext, "application/octet-stream")
    return Response(content=data, media_type=media,
                    headers={"Cache-Control": "public, max-age=86400"})


_LB_ICON_NAME_RE = re.compile(r"^[a-z0-9_]+$")


@router.get("/site/leaderboards/board-icon/{name}")
async def site_lb_board_icon(name: str) -> Response:
    """A leaderboard board icon (``ui/leaderboard_icons/<name>.png``) served from
    the updates CAS we already mirror - so the game art isn't duplicated into the
    repo and stays current with the game. ``name`` is the icon stem the client's
    boardIconName() produces (e.g. ``icon_paragon_knight``); the strict charset
    is the path-traversal guard. Long-cached + tokenless so the leaderboards /
    player pages can <img> them; independent of feature_updates_enabled
    (leaderboards owns this surface). 404 → the <img> falls back to no icon."""
    from app.trove.updates import read as updates_read
    from app.trove.updates.cas import ContentStore

    if not _LB_ICON_NAME_RE.match(name):
        raise HTTPException(status_code=404, detail="bad icon name")
    meta = await updates_read.get_file_meta(
        _STORE_TEXTURE_BRANCH, f"ui/leaderboard_icons/{name}.png",
    )
    if not meta or not meta["content_sha256"]:
        raise HTTPException(status_code=404, detail="icon not found")
    blob = ContentStore(settings.trove_update_store_dir).path_for(meta["content_sha256"])
    if not blob.is_file():
        raise HTTPException(status_code=404, detail="blob missing")
    data = await asyncio.to_thread(blob.read_bytes)
    return Response(content=data, media_type="image/png",
                    headers={"Cache-Control": "public, max-age=604800"})


_BRDF_PATH = "textures/brdfmap.dds"
_brdf_png: tuple[str, bytes] | None = None      # (blob sha, converted PNG)


def _brdf_to_png(dds: bytes) -> bytes:
    """The BRDF atlas as a PNG the browser can upload as a texture. Uncompressed
    BGRA8 with mips; Pillow reads the top level, and the alpha channel is unused."""
    import io

    from PIL import Image

    img = Image.open(io.BytesIO(dds)).convert("RGB")
    out = io.BytesIO()
    img.save(out, format="PNG", optimize=True)
    return out.getvalue()


@router.get("/site/render/brdf-map.png")
async def site_render_brdf_map() -> Response:
    """Trove's specular BRDF lookup atlas, converted to PNG for the 3D viewers.

    A voxel's specular-map value (rough / metal / water / iridescent / waxy) is an
    index into this 4x2 atlas of highlight lobes, which the game's own shader
    samples by (N·H, L·H) - so the previews reproduce the highlight rather than
    inventing per-material shininess. Pulled from the updates CAS like the
    leaderboard icons, so the game art isn't duplicated into the repo, and cached
    in-process (it is one small file that only changes when the game's does). A
    404 here just means the viewers shade every solid as rough."""
    global _brdf_png
    from app.trove.updates import read as updates_read
    from app.trove.updates.cas import ContentStore

    meta = await updates_read.get_file_meta(_STORE_TEXTURE_BRANCH, _BRDF_PATH)
    sha = (meta or {}).get("content_sha256")
    if not sha:
        raise HTTPException(status_code=404, detail="brdf map not in the archive")
    headers = {"Cache-Control": "public, max-age=604800", "ETag": f'"{sha}"'}
    if _brdf_png and _brdf_png[0] == sha:
        return Response(content=_brdf_png[1], media_type="image/png", headers=headers)
    blob = ContentStore(settings.trove_update_store_dir).path_for(sha)
    if not blob.is_file():
        raise HTTPException(status_code=404, detail="blob missing")
    try:
        png = await asyncio.to_thread(_brdf_to_png, await asyncio.to_thread(blob.read_bytes))
    except Exception as exc:  # noqa: BLE001 - an undecodable texture is a 404, not a 500
        logger.warning("brdf map: could not convert %s: %s", sha, exc)
        raise HTTPException(status_code=404, detail="brdf map unreadable") from exc
    _brdf_png = (sha, png)
    return Response(content=png, media_type="image/png", headers=headers)


@router.get("/site/giveaways", response_class=JSONResponse)
async def site_giveaways() -> JSONResponse:
    """Public giveaway list for the /giveaways page (open, upcoming, recent).
    Short cache - entry counts move as people enter."""
    from app.giveaways import service as giveaways_service
    items = await giveaways_service.list_public()
    return JSONResponse(
        {"items": [i.model_dump(mode="json") for i in items]},
        headers={"Cache-Control": "public, max-age=15"},
    )


@router.get("/site/market/listings", response_class=JSONResponse)
async def site_market_listings(
    name: str | None = Query(default=None),
    price_min: float | None = Query(default=None, ge=0),
    price_max: float | None = Query(default=None, ge=0),
    last_seen_after: int | None = Query(default=None, ge=0),
    hide_expired: bool = Query(default=True),
    sort: str = Query(default="-last_seen"),
    limit: int = Query(default=100, ge=1, le=500),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    from app.trove.market import service as market_service
    if price_min is not None and price_max is not None and price_min > price_max:
        raise HTTPException(
            status_code=400,
            detail="price_min cannot be greater than price_max",
        )
    items, total = await market_service.list_listings(
        name=name, price_min=price_min, price_max=price_max,
        last_seen_after=last_seen_after, hide_expired=hide_expired,
        sort=sort, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/market/items/{name}/summary", response_class=JSONResponse)
async def site_market_item_summary(name: str) -> JSONResponse:
    from app.trove.market import service as market_service
    summary = await market_service.item_summary(name)
    if summary is None:
        raise HTTPException(
            status_code=404,
            detail=f"No active market listings for '{name}'",
        )
    return JSONResponse(summary, headers={"Cache-Control": "public, max-age=30"})


@router.get(
    "/site/market/items/{name}/history",
    response_class=JSONResponse,
)
async def site_market_item_history(
    name: str,
    days: int = Query(default=7, ge=1, le=30),
    include_expired: bool = Query(default=False),
    keep_outliers: bool = Query(
        default=False,
        description="Skip the log-space outlier filter and return the raw cloud.",
    ),
) -> JSONResponse:
    """Per-listing price-vs-time series for one item - drives the
    price-evolution chart on the /market page.

    Toggles:
      - ``include_expired`` - include posts that have aged out of the
        in-game live cycle (3h stale / 7d TTL).
      - ``keep_outliers`` - by default we run a log-space modified-Z
        filter (threshold 3.5) to drop extreme one-off listings that
        would otherwise stretch the y-axis through the roof. Pass
        ``true`` to see the raw cloud.
    """
    from app.trove.market import service as market_service
    payload = await market_service.price_history(
        name, days=days,
        include_expired=include_expired,
        keep_outliers=keep_outliers,
    )
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


# --- /site/market/analytics/* - the Market Analytics tab ------------------
# Aggregations over the same listing history the browser above reads. Cached a
# little longer than the live listing views since they move slowly.

@router.get("/site/market/analytics/timeline", response_class=JSONResponse)
async def site_market_analytics_timeline(
    name: str = Query(..., min_length=1),
    days: int = Query(default=14, ge=1, le=90),
    bucket_hours: int = Query(default=24, ge=1, le=168),
) -> JSONResponse:
    """Daily median/p25/p75 price band + supply volume for one item, plus the
    merchant-event bands overlapping the window (for the chart shading)."""
    from app.trove.market import service as market_service
    payload = await market_service.analytics_timeline(
        name, days=days, bucket_hours=bucket_hours)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/market/analytics/deals", response_class=JSONResponse)
async def site_market_analytics_deals(
    days: int = Query(default=7, ge=1, le=30),
    min_discount: float = Query(default=0.25, ge=0.05, le=0.95),
    limit: int = Query(default=50, ge=1, le=200),
) -> JSONResponse:
    """Underpriced active listings (a flip-finder): posts priced at least
    ``min_discount`` below their item's median-each, biggest discount first."""
    from app.trove.market import service as market_service
    deals = await market_service.analytics_deals(
        days=days, min_discount=min_discount, limit=limit)
    return JSONResponse(
        {"items": deals, "count": len(deals), "min_discount": min_discount, "days": days},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/market/analytics/movers", response_class=JSONResponse)
async def site_market_analytics_movers(
    days: int = Query(default=7, ge=1, le=30),
    limit: int = Query(default=40, ge=1, le=100),
) -> JSONResponse:
    """Biggest median-price movers over ``days``: risers and fallers vs the prior
    equal-length window."""
    from app.trove.market import service as market_service
    payload = await market_service.analytics_movers(days=days, limit=limit)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/market/analytics/overview", response_class=JSONResponse)
async def site_market_analytics_overview(
    days: int = Query(default=7, ge=1, le=30),
) -> JSONResponse:
    """Market-pulse header: live active-listing count / distinct items / total flux
    value, plus the window's biggest mover and most-traded item."""
    from app.trove.market import service as market_service
    payload = await market_service.analytics_overview(days=days)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/market/analytics/liquidity", response_class=JSONResponse)
async def site_market_analytics_liquidity(
    days: int = Query(default=14, ge=1, le=30),
    limit: int = Query(default=40, ge=1, le=100),
) -> JSONResponse:
    """Per-item sell-through / time-to-sell, estimated from how long listings live
    before leaving the market (an estimate - hourly capture, sale vs cancel
    indistinguishable)."""
    from app.trove.market import service as market_service
    payload = await market_service.analytics_liquidity(days=days, limit=limit)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/market/analytics/volume", response_class=JSONResponse)
async def site_market_analytics_volume(
    days: int = Query(default=14, ge=1, le=30),
    limit: int = Query(default=40, ge=1, le=100),
) -> JSONResponse:
    """Most-traded items by new-listing supply over ``days``."""
    from app.trove.market import service as market_service
    payload = await market_service.analytics_volume(days=days, limit=limit)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=120"})


# --- /site/codexes/* JSON proxies for the /codexes page --------------------
# The two "modes" are branches (live-us / pts); default to live-us.

_CODEX_BRANCHES = ("live-us", "pts")
_DEFAULT_CODEX_BRANCH = "live-us"


def _site_codex_branch(branch: str) -> str:
    if branch not in _CODEX_BRANCHES:
        raise HTTPException(
            status_code=404,
            detail=f"Unknown branch '{branch}' (known: {', '.join(_CODEX_BRANCHES)})",
        )
    return branch


def _site_codex_type(codex_type: str) -> None:
    if codex_type not in CODEX_TYPES:
        raise HTTPException(
            status_code=404,
            detail=f"Unknown codex type '{codex_type}' (known: {', '.join(CODEX_TYPES)})",
        )


def _codex_row(d: dict) -> dict:
    """JSON-safe codex entry row (the only non-serialisable field is the datetime)."""
    out = dict(d)
    ts = out.get("indexed_at")
    if hasattr(ts, "isoformat"):
        out["indexed_at"] = ts.isoformat()
    return out


@router.get("/site/codexes/types", response_class=JSONResponse)
async def site_codex_types(
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    _site_codex_branch(branch)
    rows = await codexes_read.type_counts(branch)
    return JSONResponse(
        {"branch": branch, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/codexes/categories", response_class=JSONResponse)
async def site_codex_categories(
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    type: str | None = Query(default=None),
) -> JSONResponse:
    _site_codex_branch(branch)
    if type is not None:
        _site_codex_type(type)
    rows = await codexes_read.list_categories(branch, type)
    return JSONResponse(
        {"branch": branch, "type": type, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/codexes/search", response_class=JSONResponse)
async def site_codex_search(
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    q: str | None = Query(default=None),
    type: str | None = Query(default=None),
    category: str | None = Query(default=None),
    tradable: bool | None = Query(default=None),
    sort: str = Query(default="name"),
    limit: int = Query(default=60, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """Cross-type / per-type search for the /codexes grid. Every filter is optional
    and ANDed; each result carries its own ``type``."""
    _site_codex_branch(branch)
    if type is not None:
        _site_codex_type(type)
    if sort not in codexes_read.SORTS:
        raise HTTPException(status_code=400, detail=f"Invalid sort '{sort}'")
    docs, total = await codexes_read.query_entries(
        branch, codex_type=type, search=q, category=category, tradable=tradable,
        sort=sort, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"branch": branch, "type": type, "query": q,
         "items": [_codex_row(d) for d in docs], "count": len(docs), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/codexes/render")
async def site_codex_render(
    blueprint: str = Query(..., min_length=1, description="Blueprint logical name from the codex row"),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    dim: int = Query(default=160, ge=32, le=512),
    prefab: str | None = Query(default=None, description="Codex entry path; renders the "
                               "whole assembled creature when its rig can supply every part"),
) -> Response:
    """Same-origin PNG render of a codex item's blueprint (the card thumbnail).
    Cached in Redis.

    The grid's ``<img onerror>`` hides on any non-200, so the status code exists for
    whoever is debugging a missing image - and there it has to separate the two very
    different causes, matching ``/v1/codexes/render``: **404** the branch has no such
    blueprint (a bad/stale name on the codex row), **422** the file is there but has
    nothing to draw (an empty placeholder - typically a component part rather than a
    whole model). Collapsing both into 404 is what previously made this only
    diagnosable by rendering the blueprint by hand inside the container."""
    from app.trove.render.voxel import BlueprintError

    _site_codex_branch(branch)
    # A creature is a set of parts on a skeleton; `blueprint` alone is one of those parts
    # (or the game's small `_ui` stand-in). When the caller names the prefab too, draw the
    # whole assembled creature - falling back to `blueprint` whenever every part can't be
    # supplied, since a half-assembled mount is worse than a single part.
    if prefab:
        assembled = await render_creature_cached(prefab, dim=dim, branch=branch)
        if assembled:
            return Response(content=assembled, media_type="image/png",
                            headers={"Cache-Control": "public, max-age=86400"})
    try:
        png = await render_blueprint_cached(blueprint, dim=dim, branch=branch)
    except BlueprintError as exc:
        logger.info("codex render: %r is not renderable: %s", blueprint, exc)
        raise HTTPException(status_code=422, detail=f"Blueprint not renderable: {exc}") from None
    # Anything else is a real fault, not a property of the blueprint - let it 500 so it
    # reaches the logs instead of hiding as a per-item "no image" (the grid degrades the
    # same either way: `onerror` drops the thumbnail on any non-200).
    if png is None:
        raise HTTPException(status_code=404,
                            detail=f"No blueprint '{blueprint}' on branch '{branch}'")
    return Response(content=png, media_type="image/png",
                    headers={"Cache-Control": "public, max-age=86400"})


@router.get("/site/dressing/classes", response_class=JSONResponse)
async def site_dressing_classes() -> JSONResponse:
    """Dressable classes for the /dressing-room picker - same payload as
    ``/v1/dressing/classes``."""
    from app.trove.dressing.router import list_classes

    return JSONResponse(jsonable_encoder(await list_classes()),
                        headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/dressing/palette", response_class=JSONResponse)
async def site_dressing_palette() -> JSONResponse:
    """The game's hair/eye colour swatches for the /dressing-room picker."""
    from app.trove.dressing.router import get_palette

    return JSONResponse(jsonable_encoder(await get_palette()),
                        headers={"Cache-Control": "public, max-age=3600"})


@router.get("/site/dressing/races", response_class=JSONResponse)
async def site_dressing_races() -> JSONResponse:
    """Character-creation races for the /dressing-room picker."""
    from app.trove.dressing.router import list_races

    return JSONResponse(jsonable_encoder(await list_races()),
                        headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/dressing/options", response_class=JSONResponse)
async def site_dressing_options(
    slot: str = Query(..., pattern="^(costume|hat|face|weapon|head|hair|eyes)$"),
    class_key: str | None = Query(default=None, alias="class"),
    race: str | None = Query(default=None),
    q: str | None = Query(default=None, max_length=80),
    offset: int = Query(default=0, ge=0),
    limit: int = Query(default=100, ge=1, le=500),
) -> JSONResponse:
    """One slot's options - same payload as ``/v1/dressing/options``."""
    from app.trove.dressing.router import list_options

    page = await list_options(slot=slot, class_key=class_key, race=race, q=q,
                              offset=offset, limit=limit)
    return JSONResponse(jsonable_encoder(page),
                        headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/dressing/model")
async def site_dressing_model(
    request: Request,
    class_key: str = Query(..., alias="class"),
    costume: str | None = Query(default=None),
    hat: str | None = Query(default=None),
    face: str | None = Query(default=None),
    weapon: str | None = Query(default=None),
    head: str | None = Query(default=None),
    hair: str | None = Query(default=None),
    eyes: str | None = Query(default=None),
    race: str | None = Query(default=None),
    weapon_family: str | None = Query(default=None),
    hair_color: str | None = Query(default=None, max_length=7),
    eye_color: str | None = Query(default=None, max_length=7),
    hair_scale: float | None = Query(default=None, ge=0.05, le=1.0),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
) -> Response:
    """The dressed character as the web-viewer model payload (same-origin mirror of
    ``/v1/dressing/model``), so ``model_viewer.js`` can draw it exactly as it draws an
    assembled mod."""
    from app.trove.dressing import service as dressing_service
    from app.trove.dressing.router import _with_issues, resolve_query

    outfit = await resolve_query(class_key, costume, hat, face, weapon, head, hair,
                                 weapon_family, eyes, race,
                                 hair_color, eye_color, hair_scale)
    built = await dressing_service.model(outfit, fmt)
    if built is None:
        raise HTTPException(status_code=404, detail="That outfit has nothing to draw.")
    return _with_issues(bp_cache.respond(request, built), outfit)


@router.get("/site/dressing/render")
async def site_dressing_render(
    blueprint: str | None = Query(default=None, min_length=1, max_length=200),
    prefab: str | None = Query(default=None, min_length=1, max_length=400),
    dim: int = Query(default=96, ge=32, le=256),
) -> Response:
    """Thumbnail for one option: a costume renders as its whole assembled creature
    (``prefab``), a style as its model blueprint. Same renderer and cache the codex
    grid uses - it lives here so the dressing room's own toggle governs it."""
    from app.trove.dressing.service import blueprint_path
    from app.trove.render.voxel import BlueprintError

    branch = _DEFAULT_CODEX_BRANCH
    if not blueprint:
        # A costume has no single model - it IS the assembled creature its prefab binds.
        if not prefab:
            raise HTTPException(status_code=404, detail="Nothing to render.")
        assembled = await render_creature_cached(prefab, dim=dim, branch=branch)
        if not assembled:
            raise HTTPException(status_code=404, detail="Nothing to assemble for that prefab.")
        return Response(content=assembled, media_type="image/png",
                        headers={"Cache-Control": "public, max-age=86400"})
    # The catalogue carries basenames; the archive keys on full logical paths, so a style
    # whose model lives in a dated folder renders only after this resolution. `prefab` is
    # the hint that picks between reused names, not a second thing to draw.
    resolved = await blueprint_path(blueprint.lower(), prefab or "", branch)
    try:
        png = await render_blueprint_cached(resolved or blueprint, dim=dim, branch=branch)
    except BlueprintError as exc:
        raise HTTPException(status_code=422, detail=f"Not renderable: {exc}") from None
    if png is None:
        raise HTTPException(status_code=404, detail=f"No blueprint '{blueprint}'")
    return Response(content=png, media_type="image/png",
                    headers={"Cache-Control": "public, max-age=86400"})


@router.get("/site/codexes/entry", response_class=JSONResponse)
async def site_codex_entry(
    type: str,
    path: str = Query(...),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    _site_codex_branch(branch)
    _site_codex_type(type)
    doc = await codexes_read.get_entry(branch, type, path)
    if doc is None:
        raise HTTPException(status_code=404, detail=f"No {type} entry '{path}'")
    return JSONResponse(_codex_row(doc), headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/codexes/crafting", response_class=JSONResponse)
async def site_codex_crafting(
    path: str = Query(..., description="Source prefab path of the recipe to expand"),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    """Full crafting dependency tree for a recipe, with market prices rolled up and
    a craft-vs-buy recommendation per node. 404 when ``path`` isn't a known recipe.
    Prices are best-effort - untracked ingredients come back price-unknown, never
    as zero."""
    _site_codex_branch(branch)
    tree = await codexes_crafting.build_tree(branch, path)
    if tree is None:
        raise HTTPException(status_code=404, detail=f"No recipe '{path}' on branch '{branch}'")
    return JSONResponse(tree, headers={"Cache-Control": "public, max-age=30"})


@router.get("/leaderboards", response_class=HTMLResponse)
async def leaderboards(request: Request) -> HTMLResponse:
    """Trove leaderboards browser (reads via ``/site/leaderboards/*``).

    The two anti-cheat tabs are gated on the cheater/alt-cluster calculation
    switches and rendered (or not) server-side, so a disabled tab is gone on
    first paint - no dependency on JS / the minified bundle."""
    cheaters_on = await feature_flags.is_enabled(feature_flags.CHEATER_DETECTION_FLAG)
    clusters_on = await feature_flags.is_enabled(feature_flags.ALT_CLUSTERS_FLAG)
    renames_on = await feature_flags.is_enabled(feature_flags.RENAMES_FLAG)
    duplicates_on = await feature_flags.is_enabled(feature_flags.DUPLICATES_FLAG)
    return _TEMPLATES.TemplateResponse(request, "leaderboards.html", {
        "cheater_detection_enabled": cheaters_on,
        "alt_clusters_enabled": clusters_on,
        "renames_enabled": renames_on,
        "duplicates_enabled": duplicates_on,
        "ssr": await ssr.leaderboards_view(_ssr_fetch),
    })


# /site/leaderboards/* JSON endpoints: mirror the read-side helpers from
# app/trove/router.py.

async def _lb_browse_window_days(user: SiteUser | None) -> int:
    """Day-picker history window (in days) for this caller: signed-in Dashboard
    users get the extended window (effectively the whole cold-tiered archive);
    anonymous callers are capped to the recent hot window. Deeper captures live on
    the slower cold disk, so the anon cap is what bounds anonymous cold-read load."""
    key = ("leaderboards_extended_retention_days" if user is not None
           else "leaderboards_anon_retention_days")
    return max(1, int(await runtime_config.get_setting(key)))


async def _guard_lb_archive_anchor(anchor: int, user: SiteUser | None) -> None:
    """403 if ``anchor`` predates the caller's browse window - the deeper archive
    is a signed-in perk. Signed-in users pass (their window is effectively all
    history); the client never requests older anchors for anon, so this only trips
    a direct/deep-link hit."""
    window = await _lb_browse_window_days(user)
    if anchor < int(time.time()) - window * 86400:
        anon_days = max(1, int(await runtime_config.get_setting("leaderboards_anon_retention_days")))
        from app.core.errors import APIError, ErrorCode
        raise APIError(
            status_code=403, code=ErrorCode.forbidden,
            message=f"Sign in to view captures older than {anon_days} days.",
        )


@router.get("/site/leaderboards/config", response_class=JSONResponse)
async def site_lb_config(
    user: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """Runtime tunables the leaderboards page needs to render its chrome.

    The hot-retention window (so the subtitle's "N-day live retention" line
    tracks master-panel changes within the 5s runtime_config cache window), the
    day-picker window for THIS caller (anon vs signed-in) + the top-5 chart's
    max age, plus the cheater/alt-cluster calculation switches so the page can
    hide the Possible-cheaters / Alt-clusters tabs when their compute is disabled."""
    days = await runtime_config.get_setting("leaderboards_hot_retention_days")
    picker_days = await _lb_browse_window_days(user)
    graph_max_age_days = max(1, int(await runtime_config.get_setting("leaderboards_anon_retention_days")))
    cheaters_on = await feature_flags.is_enabled(feature_flags.CHEATER_DETECTION_FLAG)
    clusters_on = await feature_flags.is_enabled(feature_flags.ALT_CLUSTERS_FLAG)
    renames_on = await feature_flags.is_enabled(feature_flags.RENAMES_FLAG)
    duplicates_on = await feature_flags.is_enabled(feature_flags.DUPLICATES_FLAG)
    return JSONResponse(
        {
            "hot_retention_days": int(days),
            # Day-picker depth for THIS caller + the top-5 chart's cutoff.
            "logged_in": user is not None,
            "picker_days": picker_days,
            "graph_max_age_days": graph_max_age_days,
            "cheater_detection_enabled": cheaters_on,
            # Independent switch - alt-clusters can run without cheater detection.
            "alt_clusters_enabled": clusters_on,
            "renames_enabled": renames_on,
            "duplicates_enabled": duplicates_on,
        },
        # Auth-dependent (picker_days/logged_in vary by the bearer token), so it
        # must not be shared-cached across callers.
        headers={"Cache-Control": "private, max-age=15", "Vary": "Authorization"},
    )


@router.get("/site/feature-flags", response_class=JSONResponse)
async def site_feature_flags() -> JSONResponse:
    """Resolved showcase-site feature toggles for the website container.

    The website (``app.web``) holds no DB connection, so it can't read
    runtime_config in-process the way the API does; it fetches this map (cached
    ~5s) to drive its per-page 404 gate and the navbar's
    ``{% if <feature>_enabled %}`` conditionals. On top of the master page
    toggles it carries the three leaderboard calc switches the /leaderboards page
    needs (cheater detection / alt clusters / renames)."""
    flags = {
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    }
    flags["cheater_detection_enabled"] = await feature_flags.is_enabled(
        feature_flags.CHEATER_DETECTION_FLAG)
    flags["alt_clusters_enabled"] = await feature_flags.is_enabled(
        feature_flags.ALT_CLUSTERS_FLAG)
    flags["renames_enabled"] = await feature_flags.is_enabled(feature_flags.RENAMES_FLAG)
    flags["duplicates_enabled"] = await feature_flags.is_enabled(
        feature_flags.DUPLICATES_FLAG)
    return JSONResponse(flags, headers={"Cache-Control": "public, max-age=5"})


@router.get("/site/changelog", response_class=JSONResponse)
async def site_changelog_view() -> JSONResponse:
    """Website source changelog (commits of ``settings.site_source_repo`` grouped
    by tag) for the public /changelog transparency page. Redis-cached ~15m."""
    from app.site import site_changelog
    return JSONResponse(
        await site_changelog.get_changelog(),
        headers={"Cache-Control": "public, max-age=300"},
    )


@router.get("/site/clubs", response_class=JSONResponse)
async def site_clubs() -> JSONResponse:
    """Public clubs directory (ordered by club-leaderboard rank) for the /clubs
    page's server-side render in the website container. Touches Mongo (``Club``)
    + Postgres (board 1100), so it lives on the API; the website fetches it."""
    from app.site import clubs_page
    clubs = await clubs_page.public_clubs_ordered()
    return JSONResponse(
        {"items": clubs, "count": len(clubs)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/leaderboards/timestamps", response_class=JSONResponse)
async def site_lb_timestamps(
    limit: int = Query(default=60, ge=1, le=365),
) -> JSONResponse:
    items = await leaderboards_cache.get_timestamps(limit)
    return JSONResponse(
        {"items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/days", response_class=JSONResponse)
async def site_lb_days(
    user: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The latest capture anchor per trove-day within THIS caller's browse window -
    anonymous callers get the recent hot window, signed-in Dashboard users get the
    whole cold-tiered archive. Powers the date-picker: one anchor per selectable
    day, newest first. Reads straight from Postgres (a per-day loose index scan),
    not the hot Redis snapshot, so it can reach past the ~365-capture cache."""
    window = await _lb_browse_window_days(user)
    # Served from the warmer-maintained Redis cache (the compute scans the cold
    # tier); slice to THIS caller's window.
    anchors = await leaderboards_cache.get_days(400)
    cutoff = int(time.time()) - window * 86400
    anchors = [a for a in anchors if a >= cutoff]
    return JSONResponse(
        {"items": anchors, "count": len(anchors)},
        # Auth-dependent (the window depends on the bearer token) - not shared-cacheable.
        headers={"Cache-Control": "private, max-age=120", "Vary": "Authorization"},
    )


@router.get("/site/leaderboards/boards", response_class=JSONResponse)
async def site_lb_boards(
    created_at: int = Query(..., description="Anchor in unix seconds"),
    user: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    await _guard_lb_archive_anchor(created_at, user)
    rows = await leaderboards_cache.get_boards(created_at)
    return JSONResponse(
        {"created_at": created_at, "items": rows, "count": len(rows)},
        # Auth-gated (old anchors are signed-in only), so not shared-cacheable.
        headers={"Cache-Control": "private, max-age=60", "Vary": "Authorization"},
    )


# LITERAL-prefix routes must come BEFORE the ``/{uuid}/...`` catch-alls
# below - FastAPI matches in declaration order, and a path-param int
# validator on "activity" / "cheaters" would 422 (not fall through) if
# the catch-all matched first. Same dance as ``/players/{name}/...`` -
# put the named segments above the parameterised ones.
@router.get("/site/leaderboards/activity", response_class=JSONResponse)
async def site_lb_activity() -> JSONResponse:
    """Same payload as the public ``/v1/activity/current``."""
    payload = await leaderboards_activity.estimate_active_players()
    # no-cache: the chart must reflect a new capture / a master Reset+rebuild
    # immediately, not 30 min later. The query is a cheap indexed read.
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/leaderboards/activity/history", response_class=JSONResponse)
async def site_lb_activity_history(days: int = 7) -> JSONResponse:
    """Same payload as the public ``/v1/activity/history``: a time-series of
    activity estimates with both raw counts and per-hour rates. The chart line
    plots the rates so missed-capture gaps don't show as spikes."""
    days = max(1, min(int(days), 30))
    payload = await leaderboards_activity.estimate_active_players_history(days=days)
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/leaderboards/activity/series", response_class=JSONResponse)
async def site_lb_activity_series(period: str = "7d") -> JSONResponse:
    """Bucketed activity-level series for the Player Activity page's charts.

    ``period`` is one of 1d / 7d / 1m / 3m / 6m / 1y / all. Returns the
    downsampled points plus period peak / average / latest so the page
    paints a chart and its stat cards from a single request."""
    payload = await leaderboards_activity.activity_series(period=period)
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/leaderboards/records", response_class=JSONResponse)
async def site_lb_records() -> JSONResponse:
    """Same payload as the public ``/v1/leaderboards/records`` - highest Trove
    Mastery / Geode Mastery / Power Rank in the game. Cached briefly (the underlying
    data only moves on the daily 11:00-UTC ingest)."""
    payload = await leaderboards_service.mastery_records()
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=900"})


@router.get("/site/leaderboards/class-activity/current", response_class=JSONResponse)
async def site_lb_class_activity_current() -> JSONResponse:
    """Same payload as ``/v1/class-activity/current`` for the Class Activity page
    (no-cache so a new capture / master rebuild shows at once)."""
    payload = await leaderboards_class_activity.class_activity_current()
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/leaderboards/class-activity/series", response_class=JSONResponse)
async def site_lb_class_activity_series(period: str = "7d") -> JSONResponse:
    """Per-class bucketed series for the Class Activity multi-line chart
    (`period` = 1d / 7d / 1m / 3m / 6m / 1y / all)."""
    payload = await leaderboards_class_activity.class_activity_series(period=period)
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/trove-status", response_class=JSONResponse)
async def site_trove_status() -> JSONResponse:
    """Live Trove server status (Live + PTS) - same payload as
    ``/v1/misc/trove-status``."""
    payload = trove_status.get_status()
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/site/trove-status/history", response_class=JSONResponse)
async def site_trove_status_history(env: str = "live", days: int = 7) -> JSONResponse:
    """Status-timeline history for the /status page (same payload as
    ``/v1/misc/trove-status/history``). ``env`` ∈ live / pts."""
    env = env if env in ("live", "pts") else "live"
    days = max(1, min(int(days), 90))
    payload = await trove_status.get_history(env, days)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/leaderboards/cheaters", response_class=JSONResponse)
async def site_lb_cheaters() -> JSONResponse:
    """Possible-cheaters analysis for the leaderboards page. Same payload as the
    public ``GET /v1/leaderboards/cheaters``. The detection module caches the result
    for ``cheaters_cache_ttl_seconds`` so this is cheap to call."""
    payload = await leaderboards_detection.detect_possible_cheaters()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    return JSONResponse(
        payload,
        headers={"Cache-Control": f"public, max-age={ttl}"},
    )


@router.get("/site/leaderboards/renames", response_class=JSONResponse)
async def site_lb_renames(
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """Detected player renames for the leaderboards page, most-recent-first. Same
    payload as ``GET /v1/leaderboards/renames``; ``enabled=false`` when the
    feature flag is off (the tab hides itself)."""
    payload = await leaderboards_renames.serve_list(limit=limit, offset=offset)
    ttl = int(await runtime_config.get_setting("renames_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/leaderboards/renames/{name}", response_class=JSONResponse)
async def site_lb_rename_history(name: str) -> JSONResponse:
    """Full rename chain touching ``name`` (both directions), for the tab's
    per-name history drill-in."""
    payload = await leaderboards_renames.history(name)
    ttl = int(await runtime_config.get_setting("renames_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/leaderboards/duplicates", response_class=JSONResponse)
async def site_lb_duplicates(
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
    kind: str | None = Query(default=None, pattern="^(same_name|case)$"),
) -> JSONResponse:
    """Names that resolve to more than one player, for the leaderboards page.
    Same payload as ``GET /v1/leaderboards/duplicates``; ``enabled=false`` when
    the feature flag is off (the tab hides itself)."""
    payload = await leaderboards_duplicates.serve_list(
        limit=limit, offset=offset, kind=kind)
    ttl = int(await runtime_config.get_setting("duplicates_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/leaderboards/duplicates/{name}", response_class=JSONResponse)
async def site_lb_duplicate_lookup(name: str) -> JSONResponse:
    """Whether ONE name resolves to more than one identity - drives the warning
    banner in the player panel and on ``/player/<name>``. ``found=false`` is the
    normal answer."""
    payload = await leaderboards_duplicates.for_name(name)
    ttl = int(await runtime_config.get_setting("duplicates_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/leaderboards/{uuid}/entries", response_class=JSONResponse)
async def site_lb_entries(
    uuid: int,
    created_at: int = Query(..., description="Anchor in unix seconds"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
    user: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    await _guard_lb_archive_anchor(created_at, user)
    items, total, comparison = await leaderboards_cache.get_entries(
        uuid, created_at, limit=limit, offset=offset,
    )
    return JSONResponse(
        {
            "uuid": uuid, "created_at": created_at,
            "items": items, "count": len(items), "total": total,
            "comparison": comparison,
        },
        # Auth-gated (old anchors are signed-in only), so not shared-cacheable.
        headers={"Cache-Control": "private, max-age=30", "Vary": "Authorization"},
    )


@router.get("/site/leaderboards/players/{player_name}/history",
            response_class=JSONResponse)
async def site_lb_player_history(
    player_name: str,
    limit: int = Query(default=50, ge=1, le=500),
    uuid: int | None = Query(default=None),
) -> JSONResponse:
    # Bound to a recent window so the query prunes to recent partitions instead
    # of merge-scanning the player's entire cross-partition history (which was
    # ~30s for prolific players). The panel only renders the LATEST capture, and
    # anyone currently ranked appears every capture, so 7 days always covers it.
    window_start = int(time.time()) - 7 * 86400
    rows = await leaderboards_service.player_history(
        player_name, limit=limit, uuid=uuid, with_deltas=True, window_start=window_start,
    )
    return JSONResponse(
        {"player_name": player_name, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/{uuid}/history", response_class=JSONResponse)
async def site_lb_board_history(
    uuid: int,
    days: int = Query(default=7, ge=1, le=30),
    top: int = Query(default=5, ge=1, le=20),
) -> JSONResponse:
    """Score-vs-time trajectories for the current top-``top`` players on
    a board over the last ``days`` days. Drives the per-board chart on
    the leaderboards page. Served from the Redis read-through cache (the
    warmer pre-computes the default 7d/top-5 for every board each ingest)."""
    payload = await leaderboards_cache.get_board_history(uuid, days, top)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/leaderboards/{uuid}/health", response_class=JSONResponse)
async def site_lb_board_health(uuid: int) -> JSONResponse:
    """Board health summary (turnover / score inflation / competitiveness), served
    same-origin for the leaderboards page. 404 when the board has no stored data."""
    payload = await leaderboards_service.board_health(uuid)
    if payload is None:
        raise HTTPException(status_code=404, detail=f"No leaderboard data for uuid {uuid}")
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/leaderboards/players/{player_name}/profile",
            response_class=JSONResponse)
async def site_lb_player_profile(player_name: str) -> JSONResponse:
    """Public player profile (appearances + verified-claim flag) for the
    /player/<name> page."""
    payload = await leaderboards_service.player_profile(player_name)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/site/leaderboards/players/{player_name}/series",
            response_class=JSONResponse)
async def site_lb_player_series(
    player_name: str,
    days: int = Query(default=7, ge=1, le=30),
) -> JSONResponse:
    """Score-vs-time trajectories for ONE player, grouped per board,
    over the last ``days`` days. Drives the per-player chart in the
    history side-panel."""
    payload = await leaderboards_service.player_history_series(
        player_name, days=days,
    )
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/updates", response_class=HTMLResponse)
async def updates(request: Request) -> HTMLResponse:
    """Trove updates browser - public site read of the ``/v1/updates/*`` archive,
    via the ``/site/updates/*`` helpers below."""
    return _TEMPLATES.TemplateResponse(
        request, "updates.html", {"ssr": await ssr.updates_view(_ssr_fetch)})


# /site/updates/* JSON endpoints: mirror the public /v1/updates/* surface.

def _site_check_branch(branch: str) -> None:
    if branch not in UPDATE_BRANCHES:
        raise HTTPException(
            status_code=404,
            detail=f"Unknown branch '{branch}' (known: {', '.join(UPDATE_BRANCHES)})",
        )


@router.get("/site/updates/branches", response_class=JSONResponse)
async def site_up_branches() -> JSONResponse:
    items = await updates_read.list_branches()
    # ``list_branches`` returns ``last_probe_at`` as a ``datetime``; the v1
    # mirror runs that through a Pydantic ``BranchInfo`` which handles
    # ISO serialisation for us. ``JSONResponse`` uses plain ``json.dumps``,
    # which would 500 on the datetime - convert manually here.
    serialised = [
        {**b, "last_probe_at": iso(b.get("last_probe_at"))}
        for b in items
    ]
    return JSONResponse(
        {"items": serialised, "count": len(serialised)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/versions", response_class=JSONResponse)
async def site_up_versions(
    branch: str,
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    _site_check_branch(branch)
    docs, total = await updates_read.list_versions(branch, limit, offset)
    items = [
        {
            "branch": d.branch, "ordinal": d.ordinal, "version_tag": d.version_tag,
            "captured_at": iso(d.captured_at),
            "completed_at": iso(d.completed_at),
            "files_added": d.files_added, "files_modified": d.files_modified,
            "files_removed": d.files_removed, "bytes_added": d.bytes_added,
        }
        for d in docs
    ]
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/changes", response_class=JSONResponse)
async def site_up_changes(
    branch: str,
    ordinal: int | None = Query(default=None),
    version: str | None = Query(default=None),
    type: str | None = Query(default=None),
    limit: int = Query(default=200, ge=1, le=2000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    _site_check_branch(branch)
    if type is not None and type not in ("added", "modified", "removed"):
        raise HTTPException(status_code=400, detail=f"Invalid type '{type}'")
    ver = await updates_read.resolve_version(branch, ordinal, version)
    if ver is None:
        raise HTTPException(status_code=404, detail="No matching version for that branch")
    docs, total = await updates_read.list_changes(branch, ver.ordinal, type, limit, offset)
    return JSONResponse(
        {
            "branch": branch, "ordinal": ver.ordinal, "version_tag": ver.version_tag,
            "entries": [
                {"path": d.path, "type": d.type,
                 "content_sha256": d.content_sha256, "size": d.size}
                for d in docs
            ],
            "count": len(docs), "total": total,
            "files_added": ver.files_added,
            "files_modified": ver.files_modified,
            "files_removed": ver.files_removed,
        },
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/tree", response_class=JSONResponse)
async def site_up_tree(
    branch: str,
    prefix: str = Query(default=""),
) -> JSONResponse:
    _site_check_branch(branch)
    if prefix and not prefix.endswith("/"):
        prefix += "/"
    entries = await updates_read.list_directory(branch, prefix)
    for e in entries:
        e["last_modified_at"] = iso(e.get("last_modified_at"))
    return JSONResponse(
        {"branch": branch, "prefix": prefix,
         "entries": entries, "count": len(entries)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/updates/{branch}/search", response_class=JSONResponse)
async def site_up_search(
    branch: str,
    q: str = Query(..., min_length=1, max_length=200),
    limit: int = Query(default=200, ge=1, le=500),
) -> JSONResponse:
    """Full-tree file search - matches paths anywhere in the branch, so files
    buried in un-expanded directories still surface (the sidebar filter alone
    only sees the level that's currently loaded)."""
    _site_check_branch(branch)
    entries, total = await updates_read.search_paths(branch, q, limit)
    for e in entries:
        e["last_modified_at"] = iso(e.get("last_modified_at"))
    return JSONResponse(
        {"branch": branch, "query": q, "entries": entries,
         "count": len(entries), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/file/meta", response_class=JSONResponse)
async def site_up_file_meta(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    _site_check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    return JSONResponse({"branch": branch, **meta},
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/updates/{branch}/file/view", response_class=JSONResponse)
async def site_up_file_view(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    _site_check_branch(branch)
    payload = await updates_read.read_file_text(branch, path)
    if payload is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/updates/{branch}/file/blueprint", response_class=Response)
async def site_up_file_blueprint(
    request: Request, branch: str, path: str = Query(...),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
) -> Response:
    """Decoded voxel payload for one ``.blueprint`` in the latest tree, for the web
    3D viewer (blueprint_viewer.js) - same body shape as the Mods Hub endpoint, and
    the same payload cache behind it (keyed on the file's own content hash, so one
    decode covers every game version that ships the file unchanged)."""
    from app.core.errors import APIError, ErrorCode
    from app.trove.render.voxel import (
        BlueprintError,
        BlueprintTooLarge,
        pack_blueprint,
    )

    _site_check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    sha = meta["content_sha256"]

    async def build() -> dict:
        raw = await asyncio.to_thread(ContentStore(settings.trove_update_store_dir).get, sha)
        if raw is None:
            raise HTTPException(status_code=404, detail="Blob missing from the store")
        try:
            return await asyncio.to_thread(pack_blueprint, raw, path)
        except BlueprintTooLarge as exc:
            logging.getLogger(__name__).info("blueprint too large: %s", exc)
            raise APIError(413, ErrorCode.bad_request,
                           "This blueprint is too large to preview.") from None
        except BlueprintError as exc:
            # Empty placeholder / undecodable - the viewer surfaces this message.
            logging.getLogger(__name__).info("blueprint decode failed: %s", exc)
            raise APIError(422, ErrorCode.bad_request,
                           "This blueprint is empty or could not be decoded.") from None

    cached = await bp_cache.get_or_build(bp_cache.key_for_file(sha, path), build, fmt)
    return bp_cache.respond(request, cached)


async def _swf_manifest(branch: str, path: str) -> tuple[dict, str]:
    """The cached asset manifest for a ``.swf`` in the latest tree, plus its sha."""
    from app.trove.swf import service as swf_service

    _site_check_branch(branch)
    if not path.lower().endswith(".swf"):
        raise HTTPException(status_code=400, detail="Not a .swf file")
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    sha = meta["content_sha256"]
    raw = await asyncio.to_thread(ContentStore(settings.trove_update_store_dir).get, sha)
    if raw is None:
        raise HTTPException(status_code=404, detail="Blob missing from the store")
    return await swf_service.manifest(raw, sha), sha


@router.get("/site/updates/{branch}/file/swf", response_class=JSONResponse)
async def site_up_file_swf(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    """Every image embedded in one Flash movie, for the asset gallery.

    Lists the artwork only - ids, recovered symbol names and dimensions. The bytes
    come from ``/file/swf/asset`` per image, so opening the gallery costs one small
    JSON body however heavy the movie is."""
    payload, sha = await _swf_manifest(branch, path)
    # Drop the store hashes: they are an internal handle, and the asset endpoint
    # resolves ids against this same manifest anyway.
    assets = [{k: v for k, v in a.items() if k not in ("sha", "thumb_sha")}
              | {"thumb": a.get("thumb_sha") is not None}
              for a in payload.get("assets", [])]
    return JSONResponse(
        {"branch": branch, "path": path, "content_sha256": sha,
         "swf": payload.get("swf"), "inventory": payload.get("inventory"),
         "assets": assets, "count": len(assets)},
        headers={"Cache-Control": "public, max-age=300"},
    )


@router.get("/site/updates/{branch}/file/swf/asset", response_class=Response)
async def site_up_file_swf_asset(
    request: Request,
    branch: str,
    path: str = Query(...),
    id: int = Query(..., ge=0, le=65535),
    thumb: bool = Query(default=False),
) -> Response:
    """One extracted image, by character id. ``thumb=1`` serves the gallery-sized
    copy when there is one (small images have no separate thumbnail)."""
    from app.trove.swf import service as swf_service

    payload, _sha = await _swf_manifest(branch, path)
    asset = next((a for a in payload.get("assets", []) if a["id"] == id), None)
    if asset is None:
        raise HTTPException(status_code=404, detail=f"No asset {id} in '{path}'")
    use_thumb = thumb and asset.get("thumb_sha")
    blob_sha = asset["thumb_sha"] if use_thumb else asset["sha"]
    etag = f'"{blob_sha}"'
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers={"ETag": etag})
    data = await swf_service.asset_bytes(blob_sha)
    if data is None:
        raise HTTPException(status_code=404, detail="Asset blob missing from the store")
    return Response(
        content=data,
        media_type="image/png" if use_thumb else asset["mime"],
        headers={"ETag": etag, "Cache-Control": "public, max-age=86400"},
    )


@router.get("/site/updates/{branch}/file/swf/zip", response_class=Response)
async def site_up_file_swf_zip(
    branch: str, path: str = Query(...),
) -> Response:
    """Every extracted image from one movie, as a single .zip."""
    from app.trove.swf import service as swf_service

    payload, sha = await _swf_manifest(branch, path)
    assets = payload.get("assets", [])
    if not assets:
        raise HTTPException(status_code=404, detail="This movie has no extractable images")
    blob = await asyncio.to_thread(swf_service.build_zip, assets)
    stem = path.rsplit("/", 1)[-1].rsplit(".", 1)[0] or "assets"
    return Response(
        content=blob,
        media_type="application/zip",
        headers={
            "Content-Disposition": f'attachment; filename="{stem}-assets.zip"',
            "ETag": f'"{sha}-zip"',
            "Cache-Control": "public, max-age=3600",
        },
    )


async def _bank_manifest(branch: str, path: str) -> tuple[dict, str]:
    """The cached sound index for a ``.bnk`` in the latest tree, plus its sha.

    The names come from the ``.txt`` Wwise wrote next to the bank, so that file is
    fetched alongside it - and its hash goes into the cache key, since a rebuild
    that only renames sounds still has to invalidate the index."""
    from app.trove.audio import service as audio_service

    _site_check_branch(branch)
    if not path.lower().endswith(".bnk"):
        raise HTTPException(status_code=400, detail="Not a .bnk file")
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    sha = meta["content_sha256"]
    store = ContentStore(settings.trove_update_store_dir)
    raw = await asyncio.to_thread(store.get, sha)
    if raw is None:
        raise HTTPException(status_code=404, detail="Blob missing from the store")

    sidecar = sidecar_sha = None
    side_meta = await updates_read.get_file_meta(branch, path[: -len(".bnk")] + ".txt")
    if side_meta is not None:
        sidecar_sha = side_meta["content_sha256"]
        blob = await asyncio.to_thread(store.get, sidecar_sha)
        if blob is not None:
            sidecar = blob.decode("utf-8", "replace")
        else:
            sidecar_sha = None
    return await audio_service.manifest(raw, sha, sidecar, sidecar_sha), sha


@router.get("/site/updates/{branch}/file/bnk", response_class=JSONResponse)
async def site_up_file_bnk(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    """Every sound embedded in one Wwise bank, for the audio browser.

    Lists them only - names, codecs, durations and the container each belongs to.
    The audio itself comes from ``/file/bnk/audio`` per sound, so opening an 87 MB
    music bank costs one small JSON body and no decoding at all."""
    payload, sha = await _bank_manifest(branch, path)
    # The store hash is an internal handle; the audio endpoint resolves ids
    # against this same manifest.
    sounds = [{k: v for k, v in s.items() if k != "sha"} for s in payload.get("sounds", [])]
    return JSONResponse(
        {"branch": branch, "path": path, "content_sha256": sha,
         "bank": payload.get("bank"), "sounds": sounds, "count": len(sounds),
         "playable": payload.get("playable"),
         "total_duration": payload.get("total_duration")},
        headers={"Cache-Control": "public, max-age=300"},
    )


@router.get("/site/updates/{branch}/file/bnk/audio", response_class=Response)
async def site_up_file_bnk_audio(
    request: Request,
    branch: str,
    path: str = Query(...),
    id: int = Query(..., ge=0, le=0xFFFFFFFF),
    raw: bool = Query(default=False),
) -> Response:
    """One sound, decoded to Ogg or WAV. ``raw=1`` serves the game's own ``.wem``."""
    from app.trove.audio import service as audio_service

    payload, _sha = await _bank_manifest(branch, path)
    sound = next((s for s in payload.get("sounds", []) if s["id"] == id), None)
    if sound is None:
        raise HTTPException(status_code=404, detail=f"No sound {id} in '{path}'")
    etag = f'"{sound["sha"]}{"-raw" if raw else ""}"'
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers={"ETag": etag})

    if raw:
        data = await audio_service.raw_bytes(sound["sha"])
        if data is None:
            raise HTTPException(status_code=404, detail="Sound blob missing from the store")
        media_type, extension = "audio/vnd.wave", "wem"
    else:
        if sound.get("error"):
            raise HTTPException(status_code=422, detail=sound["error"])
        decoded = await audio_service.audio_bytes(sound["sha"])
        if decoded is None:
            raise HTTPException(status_code=404, detail="Sound blob missing from the store")
        data, media_type, extension = decoded

    stem = sound.get("name") or f"sound_{id}"
    stem = "".join(c if c.isalnum() or c in "._-" else "_" for c in stem) or "sound"
    return Response(
        content=data,
        media_type=media_type,
        headers={
            "ETag": etag,
            "Cache-Control": "public, max-age=86400",
            "Content-Disposition": f'inline; filename="{stem}.{extension}"',
        },
    )


@router.get("/site/updates/{branch}/file/bnk/zip", response_class=Response)
async def site_up_file_bnk_zip(
    branch: str, path: str = Query(...),
) -> Response:
    """Every playable sound from one bank, as a single .zip."""
    from app.trove.audio import service as audio_service

    payload, sha = await _bank_manifest(branch, path)
    sounds = payload.get("sounds", [])
    if not any(s.get("error") is None for s in sounds):
        raise HTTPException(status_code=404, detail="This bank has no playable sounds")
    blob = await asyncio.to_thread(audio_service.build_zip, sounds)
    stem = path.rsplit("/", 1)[-1].rsplit(".", 1)[0] or "sounds"
    return Response(
        content=blob,
        media_type="application/zip",
        headers={
            "Content-Disposition": f'attachment; filename="{stem}-sounds.zip"',
            "ETag": f'"{sha}-zip"',
            "Cache-Control": "public, max-age=3600",
        },
    )


@router.get("/site/updates/{branch}/vfx", response_class=JSONResponse)
async def site_up_vfx_list(
    branch: str,
    q: str = Query(default="", max_length=200),
    limit: int = Query(default=300, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """The branch's PopcornFX effects, for the VFX previewer's picker.

    Paged and substring-filtered server-side: Trove ships ~9k effects, and the
    whole list is a megabyte of JSON nobody needs in one go."""
    from app.trove.updates import vfx as updates_vfx

    _site_check_branch(branch)
    return JSONResponse(
        await updates_vfx.list_effects(branch, q, limit, offset),
        headers={"Cache-Control": "public, max-age=120"},
    )


@router.get("/site/updates/{branch}/vfx/manifest", response_class=JSONResponse)
async def site_up_vfx_manifest(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    """One effect's ``.pkfx`` source plus the assets it references, for the web
    viewer (site/static/pkfx). Same body shape as the Mods Hub's VFX manifest, so
    both surfaces drive the same player."""
    from app.trove.updates import vfx as updates_vfx

    _site_check_branch(branch)
    payload = await updates_vfx.manifest(branch, path)
    if payload is None:
        raise HTTPException(status_code=404, detail=f"No effect '{path}'")
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/updates/{branch}/vfx/asset", response_class=Response)
async def site_up_vfx_asset(
    request: Request, branch: str, path: str = Query(...),
) -> Response:
    """One texture/mesh/atlas an effect references, resolved against the pack root
    (the viewer asks by the reference it read out of the ``.pkfx``, not by archive
    path)."""
    from app.trove.updates import vfx as updates_vfx

    _site_check_branch(branch)
    found = await updates_vfx.asset(branch, path)
    if found is None:
        raise HTTPException(status_code=404, detail=f"No asset '{path}'")
    data, media, sha = found
    etag = f'"{sha}"'
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers={"ETag": etag})
    return Response(
        content=data, media_type=media,
        headers={"ETag": etag, "Cache-Control": "public, max-age=86400"},
    )


@router.post("/site/sound-studio/build", response_class=Response)
async def site_sound_studio_build(
    spec: str = Form(...),
    clips: list[UploadFile] = File(default=[]),
) -> Response:
    """Rebuild one sound bank with the caller's changes applied.

    ``spec`` is JSON - the bank to edit and a list of changes (``mute``,
    ``replace``, ``add``). Replacement audio rides alongside as file parts named
    by each change's ``clip`` key, and is **raw interleaved 16-bit PCM**: the
    browser already decodes and resamples whatever the user picked, which is why
    nothing here needs an audio decoder.

    Nothing is stored. The bank comes out of the archive, the edits are applied in
    memory, and the result streams straight back as a ``.bnk`` or a ``.tmod``."""
    from app.core.errors import APIError, ErrorCode
    from app.trove.audio import studio

    try:
        parsed = json.loads(spec)
    except ValueError:
        raise APIError(400, ErrorCode.bad_request, "The change list was not valid JSON.") from None
    if not isinstance(parsed, dict):
        raise APIError(400, ErrorCode.bad_request, "The change list was not understood.")

    branch = str(parsed.get("branch") or "")
    path = str(parsed.get("path") or "")
    _site_check_branch(branch)
    if not path.lower().endswith(".bnk"):
        raise APIError(400, ErrorCode.bad_request, "Only a .bnk file can be edited.")
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    raw = await asyncio.to_thread(
        ContentStore(settings.trove_update_store_dir).get, meta["content_sha256"])
    if raw is None:
        raise HTTPException(status_code=404, detail="Blob missing from the store")

    uploads: dict[str, bytes] = {}
    for part in clips:
        if part.filename:
            uploads[part.filename] = await part.read()

    result = await asyncio.to_thread(studio.apply_edits, raw, parsed, uploads)
    blob, filename, media_type = await asyncio.to_thread(
        studio.package, result, path, parsed)
    return Response(
        content=blob,
        media_type=media_type,
        headers={
            "Content-Disposition": f'attachment; filename="{filename}"',
            "Cache-Control": "no-store",
            "X-Kiwi-Sounds-Replaced": str(result.replaced),
            "X-Kiwi-Sounds-Added": json.dumps([a["event"] for a in result.added]),
        },
    )


@router.get("/site/updates/{branch}/file/history", response_class=JSONResponse)
async def site_up_file_history(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    _site_check_branch(branch)
    rows = await updates_read.file_history(branch, path)
    if not rows:
        raise HTTPException(
            status_code=404,
            detail=f"No history for '{path}' on branch '{branch}'",
        )
    items = [
        {**r, "captured_at": iso(r.get("captured_at"))}
        for r in rows
    ]
    return JSONResponse(
        {"branch": branch, "path": path, "items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/file/compare", response_class=JSONResponse)
async def site_up_file_compare(
    branch: str,
    path: str = Query(...),
    from_: int = Query(..., alias="from"),
    to: int = Query(...),
) -> JSONResponse:
    """Diff two versions of a file. Same body shape as
    ``/v1/updates/{branch}/file/compare``."""
    _site_check_branch(branch)
    v_from = await updates_read.resolve_version(branch, ordinal=from_)
    v_to = await updates_read.resolve_version(branch, ordinal=to)
    if v_from is None or v_to is None:
        raise HTTPException(
            status_code=404,
            detail="One of the requested ordinals doesn't exist on this branch",
        )
    a = await updates_read.resolve_file_at_version(branch, path, from_)
    b = await updates_read.resolve_file_at_version(branch, path, to)
    a_info = {
        "ordinal": v_from.ordinal, "version_tag": v_from.version_tag,
        "captured_at": iso(v_from.captured_at),
        "content_sha256": a["content_sha256"] if a else None,
        "size": a["size"] if a else 0,
    }
    b_info = {
        "ordinal": v_to.ordinal, "version_tag": v_to.version_tag,
        "captured_at": iso(v_to.captured_at),
        "content_sha256": b["content_sha256"] if b else None,
        "size": b["size"] if b else 0,
    }
    payload = {"branch": branch, "path": path, "from": a_info, "to": b_info}

    if (a_info["content_sha256"] or "") == (b_info["content_sha256"] or "") and a_info["content_sha256"] is not None:
        payload.update({"identical": True, "is_text": True, "hunks": []})
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})
    if a is None and b is None:
        payload.update({
            "identical": True, "is_text": True, "hunks": [],
            "reason": "file did not exist at either side",
        })
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})

    store = ContentStore(settings.trove_update_store_dir)
    a_bytes = store.get(a["content_sha256"]) if a else None
    b_bytes = store.get(b["content_sha256"]) if b else None
    a_dec = updates_compare.decode_blob(a_bytes)
    b_dec = updates_compare.decode_blob(b_bytes)
    if not (a_dec.is_text and b_dec.is_text):
        payload.update({
            "identical": False, "is_text": False,
            "reason": a_dec.reason or b_dec.reason or "binary",
            "hunks": [],
        })
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})
    hunks = updates_compare.make_hunks(a_dec.lines, b_dec.lines)
    payload.update({"identical": False, "is_text": True, "hunks": hunks})
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


# --- /site/mods/* proxies for the Mods Hub pages ---------------------------
# Reads mirror ``/v1/mods/hub/*`` but pass the *site* user (Discord login) as the
# viewer - so the owner sees their own drafts + owner-only controls, which the /v1
# reads (API token, no site-user concept) never reveal. Writes still go to
# /v1/mods/hub/* directly with the site-auth bearer (CORS-allowed for the site).

@router.get("/site/mods/projects", response_class=JSONResponse)
async def site_mods_projects(
    q: str | None = Query(default=None, max_length=120),
    tag: str | None = Query(default=None, max_length=40),
    author: str | None = Query(default=None, max_length=80),
    sort: str = Query(default="recent"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    items, total = await mods_hub_service.list_public(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/mods/tags", response_class=JSONResponse)
async def site_mods_tags() -> JSONResponse:
    """Tag facets (counts) for the browse page filter bar - categories then custom."""
    return JSONResponse(
        await mods_hub_service.tag_facets(),
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/mods/profile/{handle}", response_class=JSONResponse)
async def site_mods_profile(
    handle: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """A modder's profile + their mods. The owner sees their own drafts + edit
    controls (the viewer is the *site* user, unlike the anonymous /v1 read)."""
    data = await mods_hub_service.profile_view(handle, viewer)
    if data is None:
        raise HTTPException(status_code=404, detail="No such modder.")
    return JSONResponse(data, headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/me/projects", response_class=JSONResponse)
async def site_mods_my_projects(
    viewer: SiteUser = Depends(get_optional_site_user),
) -> JSONResponse:
    """The signed-in user's own projects (drafts included) for the studio."""
    if viewer is None:
        raise HTTPException(status_code=401, detail="Sign in to view your mods.")
    return JSONResponse(
        {"items": await mods_hub_service.list_owned(viewer)},
        headers={"Cache-Control": "no-cache"},
    )


# ── creator token + API-account connections (Dashboard side) ───────────────
# The creator's half of Mods Hub API access: issue/rotate the one creator token
# for this account, and manage the dev-portal accounts that connected with it.
# The developer's half lives at /v1/mods/hub/creator-links. See mods_hub/creators.py.

@router.get("/site/mods/creator-token", response_class=JSONResponse)
async def site_mods_creator_token(
    user: SiteUser = Depends(get_current_site_user),
) -> JSONResponse:
    """The account's creator-token state + the API accounts currently connected."""
    return JSONResponse(
        {**mods_hub_creators.token_dto(user),
         "connections": await mods_hub_creators.list_for_creator(user),
         # The picker for narrowing a connection to named mods, so the panel
         # renders in one round-trip.
         "mods": await mods_hub_creators.owned_cards(user)},
        headers={"Cache-Control": "no-store"},
    )


@router.post("/site/mods/creator-token", response_class=JSONResponse)
async def site_mods_issue_creator_token(
    user: SiteUser = Depends(get_current_site_user),
) -> JSONResponse:
    """Generate this account's creator token. Returned in FULL exactly once - only
    its hash is stored, so a lost token is replaced by rotating, not re-read."""
    dto, raw = await mods_hub_creators.ensure_token(user)
    if raw is None:
        raise HTTPException(
            status_code=409,
            detail="You already have a creator token. Rotate it to get a new one "
                   "(that disconnects any API accounts using the old one).")
    return JSONResponse({**dto, "token": raw}, headers={"Cache-Control": "no-store"})


@router.post("/site/mods/creator-token/rotate", response_class=JSONResponse)
async def site_mods_rotate_creator_token(
    user: SiteUser = Depends(get_current_site_user),
) -> JSONResponse:
    """Replace the creator token and disconnect every API account connected with
    the old one. The full new token is returned once."""
    dto, raw = await mods_hub_creators.rotate_token(user)
    return JSONResponse({**dto, "token": raw}, headers={"Cache-Control": "no-store"})


@router.patch("/site/mods/creator-connections/{link_id}", response_class=JSONResponse)
async def site_mods_set_connection_scope(
    link_id: str, req: CreatorScopeRequest,
    user: SiteUser = Depends(get_current_site_user),
) -> JSONResponse:
    """Limit one connected API account to specific mods, or widen it back to all
    of them (including mods created later)."""
    return JSONResponse(
        await mods_hub_creators.set_scope(
            user, link_id, all_projects=req.all_projects, project_ids=req.project_ids),
        headers={"Cache-Control": "no-store"},
    )


@router.delete("/site/mods/creator-connections/{link_id}", status_code=204)
async def site_mods_revoke_connection(
    link_id: str, user: SiteUser = Depends(get_current_site_user),
) -> Response:
    """Cut one API account off. The others keep working."""
    await mods_hub_creators.revoke_by_creator(user, link_id)
    return Response(status_code=204)


@router.get("/site/mods/projects/{handle}/{slug}", response_class=JSONResponse)
async def site_mods_project(
    handle: str, slug: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    return JSONResponse(
        await mods_hub_service.project_detail(project, viewer),
        headers={"Cache-Control": "no-cache"},   # varies by viewer
    )


@router.get("/site/mods/projects/{handle}/{slug}/branches", response_class=JSONResponse)
async def site_mods_branches(
    handle: str, slug: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    mods_hub_service.ensure_source_visible(project, viewer)
    return JSONResponse({"items": await mods_hub_service.list_branches(project)},
                        headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/projects/{handle}/{slug}/commits", response_class=JSONResponse)
async def site_mods_commits(
    handle: str, slug: str,
    branch: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    mods_hub_service.ensure_source_visible(project, viewer)
    items, total = await mods_hub_service.list_commits(project, branch, limit, offset)
    return JSONResponse({"items": items, "count": len(items), "total": total},
                        headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/projects/{handle}/{slug}/tree", response_class=JSONResponse)
async def site_mods_tree(
    handle: str, slug: str, ref: str = Query(default=""),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    mods_hub_service.ensure_source_visible(project, viewer)
    return JSONResponse(await mods_hub_service.get_tree(project, ref),
                        headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/projects/{handle}/{slug}/placement", response_class=JSONResponse)
async def site_mods_placement(
    handle: str, slug: str, ref: str = Query(default=""),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    mods_hub_service.ensure_source_visible(project, viewer)
    return JSONResponse(await mods_hub_service.placement_report(project, ref),
                        headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/projects/{handle}/{slug}/releases", response_class=JSONResponse)
async def site_mods_releases(
    handle: str, slug: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    is_owner = viewer is not None and project.owner_id == viewer.id
    items = await mods_hub_service.list_releases(
        project, include_drafts=is_owner, include_hidden=is_owner)
    return JSONResponse({"items": items}, headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/projects/{handle}/{slug}/forks", response_class=JSONResponse)
async def site_mods_forks(
    handle: str, slug: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    return JSONResponse({"items": await mods_hub_service.list_forks(project)},
                        headers={"Cache-Control": "public, max-age=30"})


@router.get("/site/mods/projects/{handle}/{slug}/raw/{commit_ref}/{path:path}",
            response_class=Response)
async def site_mods_raw(
    handle: str, slug: str, commit_ref: str, path: str,
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    project = await mods_hub_service.get_for_view(handle, slug, viewer)
    mods_hub_service.ensure_source_visible(project, viewer)
    data = await mods_hub_service.get_file_bytes(project, commit_ref, path)
    return Response(content=data, media_type="application/octet-stream",
                    headers={"Cache-Control": "no-cache"})


@router.get("/site/mods/releases/{release_id}/download", response_class=Response)
async def site_mods_download(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Public download of a release's compiled .tmod (bumps the counter). The
    owner can also pull their own *draft* releases (to test before publishing)."""
    release, project = await mods_hub_service.release_with_project(release_id, viewer)
    data = await mods_hub_service.record_download(release, project)
    return Response(
        content=data, media_type=mods_hub_service.release_media_type(release),
        headers={
            "Content-Disposition":
                f'attachment; filename="{mods_hub_service.release_download_filename(release)}"',
            "Cache-Control": "no-cache",
        },
    )


@router.get("/site/mods/releases/{release_id}/blueprints", response_class=JSONResponse)
async def site_mods_blueprints(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """List the .blueprint models in a release + creature-rig match (drives the page's
    3D-view + assembled-creature buttons)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_blueprints(release),
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/mods/releases/{release_id}/assembled", response_class=Response)
async def site_mods_assembled(
    request: Request, release_id: str,
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """The release's blueprint parts assembled onto their creature rig (rest +
    animations) for the web model viewer. Cached + ETag'd like the single-model
    endpoint (bp_cache) - assembling a creature is the heavier of the two."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    model = await mods_hub_service.assemble_release_model(release, fmt)
    if model is None:
        from app.core.errors import APIError, ErrorCode
        raise APIError(404, ErrorCode.not_found, "No assemblable creature for this mod.")
    return bp_cache.respond(request, model)


@router.get("/site/mods/releases/{release_id}/files", response_class=JSONResponse)
async def site_mods_release_files(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The files inside a release's .tmod (path + size, preview excluded) - for the
    per-file download list on the mod page."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_files(release))


@router.get("/site/mods/releases/{release_id}/file", response_class=Response)
async def site_mods_release_file(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Download one file from inside a release's .tmod (the preview image is excluded)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    data, filename = await mods_hub_service.download_release_file(release, path)
    safe = filename.replace('"', '').replace("\r", "").replace("\n", "")
    return Response(content=data, media_type="application/octet-stream",
                    headers={"Content-Disposition": f'attachment; filename="{safe}"',
                             "Cache-Control": "no-cache"})


@router.get("/site/mods/releases/{release_id}/inspect", response_class=JSONResponse)
async def site_mods_release_inspect(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The decoded artifact (header properties + full file table) behind the mod
    page's build inspector."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.inspect_release(release))


@router.get("/site/mods/releases/{release_id}/cfgs", response_class=JSONResponse)
async def site_mods_release_cfgs(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The .cfg config files packed in a release's .tmod - drives the mod page's
    dedicated config download button (a config belongs in ModCfgs, not the mods
    folder, so it's offered separately)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_cfgs(release))


@router.get("/site/mods/releases/{release_id}/cfg", response_class=Response)
async def site_mods_release_cfg(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Download one packed .cfg, extracted from the .tmod on the fly (nothing stored)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    data, filename = await mods_hub_service.download_release_cfg(release, path)
    safe = filename.replace('"', '').replace("\r", "").replace("\n", "")
    return Response(content=data, media_type="text/plain; charset=utf-8",
                    headers={"Content-Disposition": f'attachment; filename="{safe}"',
                             "Cache-Control": "no-cache"})


@router.get("/site/rigs/{skeleton}/anim/{name}", response_class=Response)
async def site_rig_animation(skeleton: str, name: str) -> Response:
    """Lazily-loaded baked animation clip for a creature rig (the model viewer fetches
    these on demand when a clip is played). Public, shared across mods using the rig.
    Binary ``TANIM1`` - position+quaternion per attach point per frame."""
    anim = await mods_hub_service.load_rig_animation(skeleton, name)
    return Response(content=anim, media_type="application/octet-stream",
                    headers={"Cache-Control": "public, max-age=3600"})


@router.get("/site/rigs/{skeleton}/graph", response_class=Response)
async def site_rig_animation_graph(skeleton: str) -> Response:
    """The rig's animation state machine, read out of the game's own model files: which
    clip each state plays, which states lead to which, and the cross-fade on every edge.
    The viewer uses it to offer whole moves ("Jump") instead of the raw clips they are
    assembled from. Public and shared across mods on the rig, like the clips themselves."""
    graph = await mods_hub_service.load_rig_animation_graph(skeleton)
    return Response(content=graph, media_type="application/json",
                    headers={"Cache-Control": "public, max-age=3600"})


@router.get("/site/mods/releases/{release_id}/blueprint", response_class=Response)
async def site_mods_blueprint(
    request: Request, release_id: str,
    path: str = Query(..., min_length=1, max_length=400),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Decoded voxel data for one .blueprint in a release (web 3D viewer). Served
    from the payload cache (bp_cache) - decoded once per artifact, ETag'd after."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    cached = await mods_hub_service.decode_release_blueprint(release, path, fmt)
    return bp_cache.respond(request, cached)


@router.get("/site/mods/releases/{release_id}/vfx", response_class=JSONResponse)
async def site_mods_vfx_list(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The .pkfx particle effects in a release (drives the VFX-preview affordance)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_vfx(release),
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/mods/releases/{release_id}/vfx/manifest", response_class=JSONResponse)
async def site_mods_vfx_manifest(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """One effect's .pkfx text + its resolved asset dependencies (mod/game/missing) for
    the web VFX viewer."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.get_release_vfx_manifest(release, path),
                        headers={"Cache-Control": "public, max-age=120"})


@router.get("/site/mods/releases/{release_id}/vfx/asset", response_class=Response)
async def site_mods_vfx_asset(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Bytes of one asset a release's VFX references - bundled in the mod, else from the
    live game tree. Authorized against the release's .pkfx dependency set."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    data, media = await mods_hub_service.get_release_vfx_asset(release, path)
    return Response(content=data, media_type=media,
                    headers={"Cache-Control": "public, max-age=3600"})


@router.get("/site/mods/releases/{release_id}/audio", response_class=JSONResponse)
async def site_mods_audio_list(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The ``.bnk`` sound banks in a release (drives the sound-preview affordance)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_banks(release),
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/mods/releases/{release_id}/audio/bank", response_class=JSONResponse)
async def site_mods_audio_bank(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """Every sound in one of the release's banks - names, codecs, durations. Decodes
    nothing, so opening a bank of 1,600 effects costs one small JSON body."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.release_bank_index(release, path),
                        headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/mods/releases/{release_id}/audio/sound", response_class=Response)
async def site_mods_audio_sound(
    request: Request, release_id: str,
    path: str = Query(..., min_length=1, max_length=400),
    id: int = Query(..., ge=0, le=0xFFFFFFFF),
    raw: bool = Query(default=False),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """One sound, decoded to Ogg or WAV. ``raw=1`` serves the game's own ``.wem``.
    ETag'd on the media's own hash, so replaying a sound costs a 304."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    data, media, filename, etag = await mods_hub_service.release_sound(
        release, path, id, raw)
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers={"ETag": etag})
    return Response(content=data, media_type=media, headers={
        "ETag": etag,
        "Cache-Control": "public, max-age=86400",
        "Content-Disposition": f'inline; filename="{filename}"',
    })


@router.get("/site/mods/image/{sha}", response_class=Response)
async def site_mods_image(sha: str) -> Response:
    got = await mods_hub_service.get_image(sha)
    if got is None:
        raise HTTPException(status_code=404, detail="Image not found")
    data, content_type = got
    return Response(content=data, media_type=content_type,
                    headers={"Cache-Control": "public, max-age=31536000, immutable"})


# --- Modpacks (/site/modpacks/*) -------------------------------------------
# Same shape as the mods proxies. Images reuse /site/mods/image/<sha> (one CAS).

@router.get("/site/modpacks/projects", response_class=JSONResponse)
async def site_modpacks_projects(
    q: str | None = Query(default=None, max_length=120),
    tag: str | None = Query(default=None, max_length=40),
    author: str | None = Query(default=None, max_length=80),
    sort: str = Query(default="recent"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    items, total = await modpacks_service.list_public(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/modpacks/me/projects", response_class=JSONResponse)
async def site_modpacks_my_projects(
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The signed-in user's own modpacks (drafts included) for the studio."""
    if viewer is None:
        raise HTTPException(status_code=401, detail="Sign in to view your modpacks.")
    return JSONResponse(
        {"items": await modpacks_service.list_owned(viewer)},
        headers={"Cache-Control": "no-cache"},
    )


@router.get("/site/modpacks/projects/{handle}/{slug}", response_class=JSONResponse)
async def site_modpack_project(
    handle: str, slug: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    pack = await modpacks_service.get_for_view(handle, slug, viewer)
    return JSONResponse(
        await modpacks_service.pack_detail(pack, viewer),
        headers={"Cache-Control": "no-cache"},   # varies by viewer
    )


@router.get("/site/modpacks/for-mod/{handle}/{slug}", response_class=JSONResponse)
async def site_modpacks_for_mod(handle: str, slug: str) -> JSONResponse:
    """Public modpacks that include a given mod - the backlink the mod page shows."""
    items = await modpacks_service.site_packs_for_mod(handle, slug)
    return JSONResponse({"items": items, "count": len(items)},
                        headers={"Cache-Control": "public, max-age=30"})


@router.get("/site/modpacks/projects/{handle}/{slug}/download", response_class=Response)
async def site_modpack_download(
    handle: str, slug: str,
    variant: str | None = Query(default=None, max_length=80),
    format: str = Query(default="zip", pattern="^(tpack|zip)$"),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> Response:
    """Download a modpack variant (the website defaults to a ``.zip``). Public; the
    owner can also pull their own draft. Bumps the download count."""
    pack = await modpacks_service.get_for_view(handle, slug, viewer)
    blob, filename, media = await modpacks_service.build_artifact(pack, variant, format)
    await modpacks_service.record_download(pack)
    return Response(
        content=blob, media_type=media,
        headers={
            "Content-Disposition": f'attachment; filename="{filename}"',
            "Cache-Control": "no-cache",
        },
    )


@router.get("/site/screenshots.json", response_class=JSONResponse)
async def hero_screenshots() -> JSONResponse:
    """Trove screenshots (as URLs) for the landing-page hero slideshow, read from
    ``site/static/trove-screens/`` so new drops appear without an HTML edit. Empty
    list (folder missing / no recognised images) is a clean OK the JS treats as
    "no slideshow"."""
    folder = Path(settings.site_root) / "static" / "trove-screens"
    files: list[str] = []
    if folder.is_dir():
        for path in sorted(folder.iterdir()):
            if path.is_file() and path.suffix.lower() in _SCREENSHOT_EXTS:
                files.append(f"/static/trove-screens/{path.name}")
    return JSONResponse(
        {"screenshots": files, "count": len(files)},
        headers={"Cache-Control": "public, max-age=60"},
    )


# --- byte-patcher tools (removed 2026-06) -----------------------------------
# /unlock_debug and /unlock_fps used to live here as a pair of file-upload
# forms that byte-patched Trove.exe (one to enable the debug console, the
# other to lift the FPS cap). Both have been removed: Trion shipped anti-
# cheat in mid-2026 and any client-binary tampering is now grounds for a
# ban. Keep this comment as a breadcrumb for the next person who finds a
# stray link in an old changelog and wonders where the route went.
