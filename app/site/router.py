"""Data plane for the BetterTroveTools showcase site (`trove.aallyn.net`).

A JSON + binary surface under ``/site/*`` that mirrors the read-side of ``/v1/*``
but tokenless + same-origin, so the page-side JS isn't throttled by the public
API's per-token caps. The data is already public, so the bypass costs nothing.
Every ``/site/<feature>/*`` proxy is feature-gated in ``_feature_blocks``. Also
serves the OG PNG renders, the embeddable status badge, robots.txt and sitemap.xml.

**The HTML pages are NOT here** - they belong to the website container
(``app/web/pages.py``), which is what the proxy points trove.aallyn.net at. This
module used to carry a duplicate copy of all ~44 of them for the api host; those
were deleted once every api-side host (api, apex and www) started 301ing page
paths to ``app_url`` - see ``add_api_host_redirect_middleware``. Server-rendered
first paint lives in ``app/site/ssr.py`` and is driven from the web tier alone.
"""

import asyncio
import hashlib
import json
import logging
import re
import time
from pathlib import Path
from urllib.parse import quote

from fastapi import APIRouter, Depends, File, Form, HTTPException, Query, Request, UploadFile
from fastapi.encoders import jsonable_encoder
from fastapi.responses import JSONResponse, RedirectResponse, Response

from app.admin import runtime_config
from app.core import features as feature_flags
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.ratelimit import check_rate_limit
from app.core.utils import client_ip, iso
from app.site import search as site_search_mod
from app.site.feature_map import SITE_FEATURE_FLAGS as _SITE_FEATURE_FLAGS
from app.site.feature_map import SITEMAP_PAGES as _SITEMAP_PAGES
from app.site.feature_map import apply_derived as _apply_derived
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
from app.trove.blueprint import editor as bp_editor
from app.trove.blueprint import model as bp_model
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
from app.trove.mods_hub import store as mods_store
from app.trove.mods_hub import workshop as mods_workshop
from app.trove.mods_hub.schemas import CreatorScopeRequest
from app.trove.render import bp_cache
from app.trove.render.service import render_blueprint_cached, render_creature_cached
from app.trove.tomes import service as trove_tomes
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
    flags = _apply_derived({
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    })
    for attr, value in flags.items():
        setattr(request.state, attr, value)
    if _feature_blocks(request.url.path, flags):
        raise HTTPException(status_code=404)


def _flag_map(request: Request) -> dict[str, bool]:
    """The resolved feature flags as a plain dict - what the SSR builders need to
    skip fetching for a feature that's switched off."""
    return {attr: bool(getattr(request.state, attr, True))
            for attr in _SITE_FEATURE_FLAGS}



router = APIRouter(
    tags=["site"], include_in_schema=False,
    dependencies=[Depends(_resolve_feature_flags)],
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
    # The <text> nodes are drawn in 10x space and scaled back down, so every
    # value inside that group - anchors, baseline, font-size - is *10 too.
    lx, vx = lw * 5, (lw * 2 + vw) * 5
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
        f'font-family="Verdana,Geneva,DejaVu Sans,sans-serif" font-size="110">'
        f'<text x="{lx}" y="140" transform="scale(.1)" textLength="{(lw - 12) * 10}">{label}</text>'
        f'<text x="{vx}" y="140" transform="scale(.1)" textLength="{(vw - 12) * 10}">{value}</text>'
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
    flags = _apply_derived({
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    })
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


@router.get("/site/tomes", response_class=JSONResponse)
async def site_tomes() -> JSONResponse:
    """Tome payouts priced at current marketplace medians for the /tomes page.
    Short cache - the underlying medians only move when the hourly scrape lands."""
    return JSONResponse(
        await trove_tomes.valued_tomes(),
        headers={"Cache-Control": "public, max-age=120"},
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


def _plain_excerpt(md: str | None, limit: int = 280) -> str:
    """Crude markdown/HTML → plain text for a meta description."""
    t = re.sub(r"<[^>]+>", " ", md or "")            # strip HTML tags
    t = re.sub(r"!?\[([^\]]*)\]\([^)]*\)", r"\1", t)  # links/images -> their text
    t = re.sub(r"[#*`_>~|]", "", t)                   # strip md markers
    t = re.sub(r"\s+", " ", t).strip()
    return t[:limit]


# Period-keyed social cards: a shared `/activity?period=1y` link previews the
# 1Y graph. `period` MUST be a query param (or path) - URL #fragments never
# reach the server/scrapers, so they can't drive a per-period embed.
_OG_PERIODS = ("1d", "7d", "1m", "3m", "6m", "1y", "all")
_OG_PERIOD_LABEL = {
    "1d": "Last 24 hours", "7d": "Last 7 days", "1m": "Last 30 days",
    "3m": "Last 3 months", "6m": "Last 6 months", "1y": "Last 12 months",
    "all": "All time",
}


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


@router.get("/site/market/analytics/signals", response_class=JSONResponse)
async def site_market_analytics_signals(
    days: int = Query(default=21, ge=10, le=60),
) -> JSONResponse:
    """Unusual market activity: per-item price / supply / stack anomalies scored
    against each item's own history, plus a market-wide breadth reading that
    separates "this item moved" from "flux moved". Longer cache than the other
    analytics reads - it scans every item's full series, and the answer only
    changes when a new day of listings lands."""
    from app.trove.market import signals as market_signals
    payload = await market_signals.scan(days=days)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=600"})


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


@router.get("/site/codexes/stat-keys", response_class=JSONResponse)
async def site_codex_stat_keys(
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    type: str | None = Query(default=None),
) -> JSONResponse:
    """Options for the /codexes stat filter: what entries of a type actually grant."""
    _site_codex_branch(branch)
    if type is not None:
        _site_codex_type(type)
    rows = await codexes_read.stat_keys(branch, type)
    return JSONResponse(
        {"branch": branch, "type": type, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/codexes/abilities", response_class=JSONResponse)
async def site_codex_abilities(
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
    type: str | None = Query(default=None),
) -> JSONResponse:
    """Options for the /codexes ability filter (displayed refs only)."""
    _site_codex_branch(branch)
    if type is not None:
        _site_codex_type(type)
    rows = await codexes_read.ability_refs(branch, type)
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
    stat: str | None = Query(default=None),
    ability: str | None = Query(default=None),
    sort: str = Query(default="name"),
    limit: int = Query(default=60, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    """Cross-type / per-type search for the /codexes grid. Every filter is optional
    and ANDed; each result carries its own ``type``. With ``stat`` set, each row also
    carries its best ``stat_value`` for it, which the stat_value sorts order by."""
    _site_codex_branch(branch)
    if type is not None:
        _site_codex_type(type)
    if sort not in codexes_read.SORTS:
        raise HTTPException(status_code=400, detail=f"Invalid sort '{sort}'")
    docs, total = await codexes_read.query_entries(
        branch, codex_type=type, search=q, category=category, tradable=tradable,
        stat=stat, ability=ability, sort=sort, limit=limit, offset=offset,
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


@router.get("/site/search", response_class=JSONResponse)
async def site_search(
    request: Request,
    q: str = Query(default="", description="What to search for"),
    subject: str | None = Query(default=None, description="One subject; omit for the cross-subject preview"),
    limit: int = Query(default=site_search_mod.PAGE_SIZE, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    """Search pages, codex entries, players and mods at once.

    Without ``subject`` this is the type-ahead preview (a few rows from each);
    with one, that subject is paged and the rest contribute their counts for the
    sidebar. Disabled features are absent from both - search never offers a page
    the site isn't serving."""
    _site_codex_branch(branch)
    # The leaderboards analysis tabs are gated by flags that govern a CALCULATION, not
    # a page, so they're deliberately absent from SITE_FEATURE_FLAGS. Search still has
    # to honour them - the tabs aren't rendered when the analysis is off - so they're
    # resolved here rather than widening the shared map for everyone.
    flags = dict(_flag_map(request))
    flags.update({
        "cheater_detection_enabled": await feature_flags.is_enabled(feature_flags.CHEATER_DETECTION_FLAG),
        "alt_clusters_enabled": await feature_flags.is_enabled(feature_flags.ALT_CLUSTERS_FLAG),
        "leaderboard_renames_enabled": await feature_flags.is_enabled(feature_flags.RENAMES_FLAG),
    })
    payload = await site_search_mod.search(
        q, flags, branch=branch, subject=subject, limit=limit, offset=offset,
    )
    # Short cache: results move with the codex and the mod hub, and the dropdown
    # re-asks on every keystroke, so a few seconds absorbs the repeats without
    # showing a stale answer after someone uploads a mod.
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=15"})


@router.get("/site/codexes/related", response_class=JSONResponse)
async def site_codex_related(
    path: str = Query(..., description="Source prefab path of the entry"),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    """Everything the codex knows that CONNECTS to one entry, in one call.

    The detail panel needs all of it at once, and issuing four requests per card
    open would be four round-trips for a panel that is mostly empty on most entries.
    Each section is omitted when empty so the client renders only what exists.
    """
    _site_codex_branch(branch)
    out: dict = {"path": path}

    outgoing = await codexes_read.links_for(branch, path, direction="out", limit=300)
    incoming = await codexes_read.links_for(branch, path, direction="in", limit=300)

    def rows(items, rel):
        return [
            {"path": r["path"], "type": r.get("codex_type"), "name": r.get("name") or "",
             "qty": r.get("qty"), "blueprint": r.get("blueprint"),
             "data": r.get("data") or {}}
            for r in items if r["rel"] == rel
        ]

    # Outgoing: what this thing produces / consumes / is made at.
    for rel in ("crafts", "ingredient", "craftable_at", "unlocks", "member_of"):
        found = rows(outgoing, rel)
        if found:
            out[rel] = found
    # Incoming - the reverse questions, which are the interesting half.
    made_by = rows(incoming, "crafts")
    if made_by:
        out["made_by"] = made_by
    used_in = rows(incoming, "ingredient")
    if used_in:
        out["used_in"] = used_in
    unlocked_by = rows(incoming, "unlocks")
    if unlocked_by:
        out["unlocked_by"] = unlocked_by
    upgrade_cost_of = rows(incoming, "upgrade_cost")
    if upgrade_cost_of:
        out["upgrade_cost_of"] = upgrade_cost_of

    # Badges carry a rank ladder keyed on the collection path, not the prefab path.
    rel_path = path[len("prefabs/"):].removesuffix(".binfab") if path.startswith("prefabs/") else path
    requirements = await codexes_read.requirements_for(branch, rel_path)
    if requirements:
        out["requirements"] = requirements

    return JSONResponse(out, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/codexes/upgrades", response_class=JSONResponse)
async def site_codex_upgrades(
    system: str | None = Query(default=None, description="One system key; omit to list them all"),
    branch: str = Query(default=_DEFAULT_CODEX_BRANCH),
) -> JSONResponse:
    """Progression trees: the system list, or one system's nodes in rank order."""
    _site_codex_branch(branch)
    if system:
        nodes = await codexes_read.upgrade_system(branch, system)
        if not nodes:
            raise HTTPException(status_code=404, detail=f"No upgrade system '{system}'")
        return JSONResponse({"system_key": system, "items": nodes},
                            headers={"Cache-Control": "public, max-age=300"})
    systems = await codexes_read.upgrade_systems(branch)
    return JSONResponse({"items": systems}, headers={"Cache-Control": "public, max-age=300"})


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
    flags = _apply_derived({
        attr: await feature_flags.is_enabled(flag)
        for attr, flag in _SITE_FEATURE_FLAGS.items()
    })
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
    nothing here needs an audio decoder. A change flagged ``"wem": true`` instead
    carries a finished Wwise media object, which is validated and passed through.

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


# --- /site/mod-workshop/* - the /mod-workshop page's tools ------------------
# Stateless mod compiler + unpacker. Files arrive with the request, the answer goes
# back with the response, nothing is stored and no account is needed. The engine is
# ``mods_hub/workshop.py``, which is the hub's own placement rules and .tmod
# reader/builder pointed at loose files instead of a repo.


async def _workshop_throttle(request: Request) -> None:
    """Per-IP budget for the Mod Workshop's endpoints.

    These are tokenless *and* login-free, and every one of them unpacks a ``.zip``,
    parses a ``.tmod`` or builds one - so they get their own bucket instead of riding
    the shared anonymous budget, where a build storm would crowd out the read-side
    proxies the rest of the site runs on (and vice versa). Tuned by
    ``mod_workshop_rate_limit_*``; one person's session is a placement check, a build
    and a request per file they look at, so the default sits well above that."""
    max_, window = await runtime_config.get_rate_limit("mod_workshop_rate_limit")
    await check_rate_limit(f"modworkshop:{client_ip(request) or 'unknown'}", max_, window)


_WORKSHOP_LIMIT = Depends(_workshop_throttle)


def _workshop_spec(raw: str) -> dict:
    try:
        parsed = json.loads(raw or "{}")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The request wasn't understood.") from None
    if not isinstance(parsed, dict):
        raise APIError(400, ErrorCode.bad_request, "The request wasn't understood.")
    return parsed


def _workshop_error(exc: mods_workshop.WorkshopError) -> APIError:
    return APIError(400, ErrorCode.bad_request, str(exc))


def _workshop_download(data: bytes, filename: str, media_type: str,
                       plan: dict | None = None) -> Response:
    # A mod title may be non-ASCII, which a bare ``filename=`` can't carry - so the
    # header gives every browser a plain fallback and the real name in RFC 5987 form.
    fallback = re.sub(r'[^\x20-\x7e]', "_", filename).replace('"', "") or "mod"
    headers = {
        "Content-Disposition": (f'attachment; filename="{fallback}"; '
                                f"filename*=UTF-8''{quote(filename)}"),
        "Cache-Control": "no-store",
    }
    if plan is not None:
        headers["X-Kiwi-Packed"] = str(plan["packed"])
        headers["X-Kiwi-Moved"] = str(plan["counts"]["moved"])
        headers["X-Kiwi-Skipped"] = str(plan["counts"]["skipped"])
    return Response(content=data, media_type=media_type, headers=headers)


async def _workshop_source(
    paths: str, archive: UploadFile | None,
) -> tuple[str, dict[str, str], list[tuple[str, bytes]] | None, list[str]]:
    """Resolve what the page is asking about into ``(kind, header, files, paths)``.

    Two shapes reach every workshop route. Loose files picked in a browser are
    described by their paths alone - the bytes stay in the tab until there is
    something worth building, so a placement check costs a few hundred bytes rather
    than the whole mod. A ``.zip`` or an existing ``.tmod`` has to be unpacked here
    to know what is inside it at all, and then ``files`` comes back too."""
    if archive is not None and archive.filename:
        data = await archive.read()
        try:
            kind, header, files = mods_workshop.read_archive(data, archive.filename)
        except mods_workshop.WorkshopError as e:
            raise _workshop_error(e) from e
        return kind, header, files, [p for p, _ in files]
    try:
        names = json.loads(paths or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The file list wasn't understood.") from None
    if not isinstance(names, list) or not names:
        raise APIError(400, ErrorCode.bad_request, "No files were selected.")
    if len(names) > mods_workshop.MAX_FILES:
        raise APIError(400, ErrorCode.bad_request,
                       f"That's more than {mods_workshop.MAX_FILES} files - too many for one mod.")
    return "files", {}, None, [str(p) for p in names]


@router.post("/site/mod-workshop/inspect", response_class=JSONResponse)
async def site_workshop_inspect(
    paths: str = Form(default="[]"),
    archive: UploadFile | None = File(default=None),
    _limit: None = _WORKSHOP_LIMIT,
) -> JSONResponse:
    """Where every selected file would land in a build, before anything is built.

    Send ``paths`` (a JSON array - the browser's own relative paths) for loose files,
    or an ``archive`` part holding a ``.zip`` or a ``.tmod``. Each file that automatic
    placement would move also carries what happens if it's left alone (``alt``), so
    the page can answer that without asking again."""
    kind, header, files, names = await _workshop_source(paths, archive)
    plan = await mods_workshop.preview(names)
    payload = {**plan, "source": kind, "properties": header}
    payload.pop("mapping", None)     # server-side detail; every entry carries its own `final`
    # Which of these files could be the mod's settings file / preview image. Both
    # answers come off the paths alone, so loose files get them without their bytes
    # ever leaving the tab.
    payload["config_candidates"] = mods_workshop.config_candidates(names)
    payload["preview_candidates"] = mods_workshop.preview_candidates(names)
    if files is not None:
        sizes = {i: len(data) for i, (_, data) in enumerate(files)}
        for entry in payload["entries"]:
            entry["size"] = sizes.get(entry["index"], 0)
    return JSONResponse(payload, headers={"Cache-Control": "no-store"})


@router.post("/site/mod-workshop/build", response_class=Response)
async def site_workshop_build(
    spec: str = Form(...),
    paths: str = Form(default="[]"),
    files: list[UploadFile] = File(default=[]),
    archive: UploadFile | None = File(default=None),
    config: UploadFile | None = File(default=None),
    preview: UploadFile | None = File(default=None),
    _limit: None = _WORKSHOP_LIMIT,
) -> Response:
    """Compile the selected files into a ``.tmod`` and hand it straight back.

    ``spec`` is JSON: the header to stamp (``title`` / ``author`` / ``modVersion`` /
    ``notes`` / ``tags``) plus the same ``fix`` / ``keep`` placement choices the
    preview was made with, and which file is the mod's settings file
    (``config_path``) and preview image (``preview_path``). Sources are either an
    ``archive`` (``.zip`` or ``.tmod``) or ``files`` parts aligned with the ``paths``
    array; a settings file or preview image picked from disk instead of from the mod
    arrives as its own part. The placement plan is recomputed here, so
    the build always matches what the page described - it never trusts a mapping sent
    back to it. Nothing is stored: the mod is built in memory and discarded once the
    response is written."""
    parsed = _workshop_spec(spec)
    if archive is not None and archive.filename:
        data = await archive.read()
        try:
            _, header, source = mods_workshop.read_archive(data, archive.filename)
        except mods_workshop.WorkshopError as e:
            raise _workshop_error(e) from e
        # An existing .tmod carries its own header; the page's fields win where set
        # (it prefills them from exactly this), the rest of the original survives.
        # Categories are the exception: the page always sends the full set, so an
        # empty one has to mean "cleared" rather than "leave the old tags alone".
        page = parsed.get("properties") or {}
        properties = {**header,
                      **{k: v for k, v in page.items() if v or k == "tags"}}
    else:
        try:
            names = json.loads(paths or "[]")
        except ValueError:
            raise APIError(400, ErrorCode.bad_request, "The file list wasn't understood.") from None
        if not isinstance(names, list) or len(names) != len(files):
            raise APIError(400, ErrorCode.bad_request,
                           "The file list didn't match the files that arrived.")
        if not files:
            raise APIError(400, ErrorCode.bad_request, "No files were selected.")
        if len(files) > mods_workshop.MAX_FILES:
            raise APIError(400, ErrorCode.bad_request,
                           f"That's more than {mods_workshop.MAX_FILES} files - too many for one mod.")
        source = [(str(name), await part.read())
                  for name, part in zip(names, files, strict=True)]
        properties = parsed.get("properties") or {}

    # A settings file or preview image chosen from disk isn't one of the mod's own
    # files, so it is kept out of the placement pass entirely and packed by name.
    config_path = str(parsed.get("config_path") or "")
    preview_path = str(parsed.get("preview_path") or "")
    attached: list[tuple[str, bytes]] = []
    for part, is_config in ((config, True), (preview, False)):
        if part is None or not part.filename:
            continue
        name = mods_workshop.norm_path(part.filename)
        attached.append((name, await part.read()))
        if is_config:
            config_path = name
        else:
            preview_path = name

    keep = parsed.get("keep")
    try:
        artifact, plan = await mods_workshop.build_mod(
            source, properties,
            fix=bool(parsed.get("fix", True)),
            keep=keep if isinstance(keep, list) else [],
            config_path=config_path,
            preview_path=preview_path,
            attached=attached,
        )
    except mods_workshop.WorkshopError as e:
        raise _workshop_error(e) from e
    title = mods_workshop.safe_title(
        (properties or {}).get("title") if isinstance(properties, dict) else None)
    return _workshop_download(artifact, f"{title}.tmod",
                              "application/octet-stream", plan)


@router.post("/site/mod-workshop/extract", response_class=JSONResponse)
async def site_workshop_extract(
    file: UploadFile = File(...), _limit: None = _WORKSHOP_LIMIT,
) -> JSONResponse:
    """Open a ``.tmod`` and describe it: the header the game reads off it, every file
    packed inside, and whether those files sit where the game would actually look.

    The same breakdown a Mods Hub release shows under Contents, for a file that was
    never uploaded anywhere - it is read in memory and forgotten."""
    data = await file.read()
    try:
        info = await asyncio.to_thread(mods_workshop.describe, data)
    except mods_workshop.WorkshopError as e:
        raise _workshop_error(e) from e
    if not info["files"]:
        raise APIError(400, ErrorCode.bad_request, "That .tmod has no files packed in it.")
    plan = await mods_workshop.plan([f["path"] for f in info["files"]], fix=False)
    sizes = {i: f["size"] for i, f in enumerate(info["files"])}
    for entry in plan["entries"]:
        entry["size"] = sizes.get(entry["index"], 0)
    plan.pop("mapping", None)
    return JSONResponse({**plan, **info}, headers={"Cache-Control": "no-store"})


@router.post("/site/mod-workshop/preview/blueprint", response_class=Response)
async def site_workshop_blueprint(
    request: Request,
    file: UploadFile = File(...), path: str = Form(...),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
    _limit: None = _WORKSHOP_LIMIT,
) -> Response:
    """Decoded voxel data for one ``.blueprint`` inside an uploaded ``.tmod``, for the
    in-page 3D preview - the same payload (and the same payload cache) the Mods Hub's
    viewer reads, keyed on the artifact's own hash rather than on a stored release."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")

    async def build() -> dict:
        return await asyncio.to_thread(
            mods_hub_service._decode_blueprint_payload, data, path)

    cached = await bp_cache.get_or_build(
        bp_cache.key_for_tmod(hashlib.sha256(data).hexdigest(), path), build, fmt)
    return bp_cache.respond(request, cached)


@router.post("/site/mod-workshop/extract/download", response_class=Response)
async def site_workshop_extract_download(
    file: UploadFile = File(...), path: str = Form(default=""),
    _limit: None = _WORKSHOP_LIMIT,
) -> Response:
    """Unpack a ``.tmod``: the whole thing as a ``.zip``, or one file on its own
    when ``path`` names one."""
    data = await file.read()
    try:
        _, files = mods_workshop.read_mod(data)
    except mods_workshop.WorkshopError as e:
        raise _workshop_error(e) from e
    if not files:
        raise APIError(400, ErrorCode.bad_request, "That .tmod has no files packed in it.")
    wanted = mods_workshop.norm_path(path)
    if wanted:
        match = next((b for p, b in files if p == wanted), None)
        if match is None:
            raise HTTPException(status_code=404, detail=f"No file '{wanted}' in that mod.")
        return _workshop_download(match, wanted.rsplit("/", 1)[-1],
                                  "application/octet-stream")
    stem = (file.filename or "mod").rsplit("/", 1)[-1]
    stem = stem[:-5] if stem.lower().endswith(".tmod") else stem
    archive = await asyncio.to_thread(mods_workshop.to_zip, files)
    return _workshop_download(archive, f"{mods_workshop.safe_title(stem)}.zip",
                              "application/zip")


# --- /site/unlock-debug - the /unlock-debug page's patcher ------------------
# Restored 2026-08 (it was removed in 2026-06). Trove ships its debug console
# behind a single conditional jump; this rewrites that jump and hands the binary
# straight back. Nothing is stored, no account is needed, and the whole feature
# hangs off ``feature_unlock_debug_enabled``, which DEFAULTS OFF - Trion runs
# anti-cheat now and tampering with the client is grounds for a ban, so serving
# this is a deliberate choice rather than something a fresh deploy does on its own.

# The seven bytes the patch turns off. ``7C`` is JL (jump if less) followed by the
# push of the branch it guards; NOPping the jump (``90 90``) makes the console path
# unconditional. Everything else in the sequence is left alone so the match stays
# specific to that one site rather than to a two-byte opcode that occurs everywhere.
_DEBUG_FIND = bytes.fromhex("7C3968E0020000")
_DEBUG_REPL = bytes.fromhex("909068E0020000")


async def _unlock_debug_throttle(request: Request) -> None:
    """Per-IP budget for the patcher.

    Tokenless, login-free, and each call carries a whole game executable in and
    back out, so it gets its own - deliberately tight - bucket rather than riding
    the shared anonymous budget. Tuned by ``unlock_debug_rate_limit_*``; a person
    patches their client once, not once a minute."""
    max_, window = await runtime_config.get_rate_limit("unlock_debug_rate_limit")
    await check_rate_limit(f"unlockdebug:{client_ip(request) or 'unknown'}", max_, window)


@router.post("/site/unlock-debug", response_class=Response)
async def site_unlock_debug(
    trove_exe: UploadFile | None = File(default=None),
    _limit: None = Depends(_unlock_debug_throttle),
) -> Response:
    """Byte-patch an uploaded ``Trove.exe`` to enable the in-client debug console.

    The whole file comes in, seven bytes change, the whole file goes back out - the
    upload is never written to disk and nothing about it is kept. A file that does
    not contain the sequence is rejected rather than returned unchanged, because a
    silent no-op is indistinguishable from a successful patch once it is downloaded:
    the visitor replaces their client, the console still is not there, and there is
    nothing to tell them why."""
    if trove_exe is None or not trove_exe.filename:
        raise APIError(400, ErrorCode.bad_request, "No file was uploaded.")
    data = await trove_exe.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    if data[:2] != b"MZ":
        raise APIError(400, ErrorCode.bad_request,
                       "That doesn't look like a Windows executable.")
    if _DEBUG_FIND not in data:
        raise APIError(
            400, ErrorCode.bad_request,
            "This build of Trove.exe doesn't contain the sequence this patch edits - "
            "it may already be patched, or the game may have changed.",
        )
    patched = data.replace(_DEBUG_FIND, _DEBUG_REPL)
    return Response(
        content=patched,
        media_type="application/octet-stream",
        headers={
            "Content-Disposition": 'attachment; filename="Trove.exe"',
            "Cache-Control": "no-store",
        },
    )


# --- /site/blueprint-editor/* - the /blueprint-editor page's tools ----------
# Stateless voxel editor. A .blueprint arrives with the request and the answer goes
# back with the response; nothing is stored and no account is needed. On save the
# page posts the ORIGINAL file back alongside its edit list, so the server never has
# to hold the model between opening it and writing it. The engine is
# ``app/trove/blueprint/editor.py``.


async def _blueprint_editor_throttle(request: Request) -> None:
    """Per-IP budget for the Blueprint Editor's endpoints.

    Tokenless and login-free, and each call decodes or re-encodes a voxel model, so
    it gets its own bucket instead of riding the shared anonymous budget - a save
    storm shouldn't crowd out the read-side proxies the rest of the site runs on.
    Tuned by ``blueprint_editor_rate_limit_*``."""
    max_, window = await runtime_config.get_rate_limit("blueprint_editor_rate_limit")
    await check_rate_limit(f"bpeditor:{client_ip(request) or 'unknown'}", max_, window)


_BP_EDITOR_LIMIT = Depends(_blueprint_editor_throttle)


def _blueprint_editor_error(exc: bp_editor.EditorError) -> APIError:
    return APIError(400, ErrorCode.bad_request, str(exc))


@router.post("/site/blueprint-editor/inspect", response_class=JSONResponse)
async def site_blueprint_editor_inspect(
    file: UploadFile = File(...),
    _limit: None = _BP_EDITOR_LIMIT,
) -> JSONResponse:
    """Open a ``.blueprint`` for editing: the voxel payload plus each voxel's real
    material.

    On top of the ``x/y/z/rgb/kind/level/spec`` arrays the 3D viewers already read,
    this carries the raw ``type`` and ``w`` per voxel and an ``edit`` flag marking
    which ones the material palette may rewrite. Voxels the editor can't safely
    reinterpret - deco placeholders, terrain, anything unrecognised - come back
    flagged read-only rather than quietly treated as plain solid blocks."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    name = (file.filename or "blueprint").rsplit("/", 1)[-1]
    try:
        payload = await asyncio.to_thread(bp_editor.inspect, data, name=name)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return JSONResponse(payload, headers={"Cache-Control": "no-store"})


async def _bp_stack(layers, stack) -> tuple[list[bytes], list]:
    """The layer files plus their placements, as posted alongside a base blueprint.

    Layering stays non-destructive while editing: a layer hides what is under it rather
    than replacing it. Every OUTPUT path takes the stack so that the flattening - and
    therefore which voxel wins a shared cell - happens once, at output, in
    ``editor.composite``."""
    data = [await part.read() for part in (layers or []) if part and part.filename]
    if not data:
        return [], []
    try:
        specs = json.loads(stack or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The layer list wasn't understood.") from None
    if not isinstance(specs, list):
        raise APIError(400, ErrorCode.bad_request, "The layer list wasn't understood.")
    return data, specs


@router.post("/site/blueprint-editor/flatten", response_class=Response)
async def site_blueprint_editor_flatten(
    file: UploadFile = File(...),
    layers: list[UploadFile] = File(default=[]),
    edits: str = Form(default="[]"),
    stack: str = Form(default="[]"),
    anchor_at: int = Form(default=0),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Collapse the layer stack into the base and hand back the single blueprint.

    The same resolution ``save`` does, offered on its own so a stack can be turned into
    one model and carried on with - painting across a seam needs the seam to exist.

    ``layers`` are the stacked blueprints bottom to top; ``stack`` is a JSON array of
    ``{"mode": "attachment"|"centre"|"corner", "offset": [x, y, z]}`` aligned with them.
    A cell claimed by more than one model goes to the highest layer that has it."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    parts, specs = await _bp_stack(layers, stack)
    if not parts:
        raise APIError(400, ErrorCode.bad_request, "There are no layers to flatten.")
    try:
        parsed = json.loads(edits or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request, "The edit list wasn't understood.") from None
    try:
        out, summary = await asyncio.to_thread(
            bp_editor.composite, data, parsed, parts, specs, anchor_at)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return Response(
        content=out,
        media_type="application/octet-stream",
        headers={"Cache-Control": "no-store", "X-Kiwi-Summary": json.dumps(summary)},
    )


@router.post("/site/blueprint-editor/transform", response_class=Response)
async def site_blueprint_editor_transform(
    file: UploadFile = File(...),
    edits: str = Form(default="[]"),
    ops: str = Form(...),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Rotate and/or mirror the model, and hand back the turned blueprint.

    ``ops`` is a JSON array of ``rotate_x`` / ``rotate_y`` / ``rotate_z`` /
    ``mirror_x`` / ``mirror_y`` / ``mirror_z``, applied in order. Rotations are 90
    degrees clockwise looking down that axis.

    Turning a model moves three things that don't move on their own: the bounding box,
    the attachment point (which lives in the origin, so a rotated sword would otherwise
    be held by a point out in the air), and the positions of any placed decos. All
    three are handled, and a model whose entity section can't be read exactly is
    refused rather than turned half-way.

    The response is the new blueprint, because a transform renumbers every voxel and
    the page has to reopen it - edit indices stop meaning anything once the axes move."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    try:
        parsed_edits = json.loads(edits or "[]")
        parsed_ops = json.loads(ops or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The request wasn't understood.") from None
    try:
        out, summary = await asyncio.to_thread(
            bp_editor.transform, data, parsed_edits, parsed_ops)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return Response(
        content=out,
        media_type="application/octet-stream",
        headers={"Cache-Control": "no-store", "X-Kiwi-Summary": json.dumps(summary)},
    )


@router.post("/site/blueprint-editor/export-qb", response_class=Response)
async def site_blueprint_editor_export_qb(
    file: UploadFile = File(...),
    layers: list[UploadFile] = File(default=[]),
    edits: str = Form(default="[]"),
    stack: str = Form(default="[]"),
    anchor_at: int = Form(default=0),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Export the model as Trove's four authoring ``.qb`` files, zipped.

    A blueprint is compiled; the ``.qb`` set is what a modder actually works in - the
    base model plus the ``_a`` / ``_s`` / ``_t`` material maps - so this is the way out
    of the editor and into Qubicle or MagicaVoxel. The attachment point is written back
    as a magenta voxel and as the matrix offset, which is how both Trove and Troxel
    expect to find it.

    The zip's ``X-Kiwi-Notes`` header carries anything the conversion had to flatten
    (a specular finish the map palette can't express, a game-internal material), so the
    page can say what changed rather than letting the user discover it later."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    try:
        parsed = json.loads(edits or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The edit list wasn't understood.") from None
    stem = (file.filename or "model").rsplit("/", 1)[-1]
    if stem.lower().endswith(".blueprint"):
        stem = stem[: -len(".blueprint")]
    stem = re.sub(r"[^A-Za-z0-9._-]", "_", stem) or "model"
    try:
        parts, specs = await _bp_stack(layers, stack)
        archive, summary = await asyncio.to_thread(
            bp_editor.export_qb, data, parsed, parts, specs, anchor_at, stem=stem)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return Response(
        content=archive,
        media_type="application/zip",
        headers={
            "Content-Disposition": f'attachment; filename="{stem}_qb.zip"',
            "Cache-Control": "no-store",
            "X-Kiwi-Notes": json.dumps(summary["notes"]),
        },
    )


@router.post("/site/blueprint-editor/import-qb", response_class=Response)
async def site_blueprint_editor_import_qb(
    files: list[UploadFile] = File(...),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Compile ``.qb`` files into a ``.blueprint`` and hand the bytes straight back.

    Send the base model and, optionally, its ``_a`` / ``_s`` / ``_t`` maps - they are
    matched by filename suffix, the same way Trove's own pipeline matches them. A
    missing map means its default (opaque, rough, solid), which is what the game
    assumes too. The attachment point comes from the magenta voxel if there is one,
    otherwise from a negative matrix offset.

    The response is the blueprint itself so the page can open it in the editor; what
    the conversion made of it rides along in ``X-Kiwi-Summary``."""
    parts: dict[str, bytes] = {}
    for part in files:
        if not part.filename:
            continue
        name = part.filename.rsplit("/", 1)[-1]
        if not name.lower().endswith(".qb"):
            raise APIError(400, ErrorCode.bad_request,
                           f"'{name}' isn't a .qb file.")
        parts[name] = await part.read()
    if not parts:
        raise APIError(400, ErrorCode.bad_request, "No .qb files were sent.")
    if len(parts) > 4:
        raise APIError(400, ErrorCode.bad_request,
                       "Send the model and up to three material maps.")
    try:
        data, summary = await asyncio.to_thread(bp_editor.import_qb, parts)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return Response(
        content=data,
        media_type="application/octet-stream",
        headers={"Cache-Control": "no-store", "X-Kiwi-Summary": json.dumps(summary)},
    )


@router.post("/site/blueprint-editor/check", response_class=JSONResponse)
async def site_blueprint_editor_check(
    file: UploadFile = File(...),
    layers: list[UploadFile] = File(default=[]),
    edits: str = Form(default="[]"),
    stack: str = Form(default="[]"),
    anchor_at: int = Form(default=0),
    kind: str = Form(default="other"),
    _limit: None = _BP_EDITOR_LIMIT,
) -> JSONResponse:
    """Check a model against the Trove Creations guidelines.

    Same inputs as ``save`` plus a creation type (``melee``, ``hat``, ``deco``, ...),
    because the rules are per type: a spear must be exactly 45 long, a hat must hang
    at least 6 voxels above its attachment point, a bow may not be more than 3 thick.
    The edits are applied before checking, so the answer describes the file that would
    be downloaded rather than the one that was uploaded.

    Findings carry the voxel indices they refer to where a rule can point at specific
    offenders (near-black voxels, disconnected geometry, a crowded grip), so the page
    can highlight them in the 3D view."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    try:
        parsed = json.loads(edits or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The edit list wasn't understood.") from None
    try:
        parts, specs = await _bp_stack(layers, stack)
        report = await asyncio.to_thread(
            bp_editor.check, data, parsed, kind, parts, specs, anchor_at)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    return JSONResponse(report, headers={"Cache-Control": "no-store"})


@router.post("/site/blueprint-editor/save", response_class=Response)
async def site_blueprint_editor_save(
    file: UploadFile = File(...),
    layers: list[UploadFile] = File(default=[]),
    edits: str = Form(default="[]"),
    stack: str = Form(default="[]"),
    anchor_at: int = Form(default=0),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Apply the page's edits to the posted ``.blueprint`` and hand the result back.

    ``edits`` is a JSON array of ``{"i": voxel index, "type": .., "w": .., "rgb": ..}``,
    indexed against the order ``inspect`` returned. Everything structural - version,
    origin, bounding box, entity section - is carried over from the file that arrived,
    so the saved model sits exactly where the original did and its decos survive
    untouched. Nothing is stored: the blueprint is rebuilt in memory and discarded
    once the response is written."""
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")
    try:
        parsed = json.loads(edits or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The edit list wasn't understood.") from None
    parts, specs = await _bp_stack(layers, stack)
    try:
        out, summary = await asyncio.to_thread(
            bp_editor.composite, data, parsed, parts, specs, anchor_at)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    name = (file.filename or "blueprint.blueprint").rsplit("/", 1)[-1]
    if not name.lower().endswith(".blueprint"):
        name += ".blueprint"
    fallback = re.sub(r"[^\x20-\x7e]", "_", name).replace('"', "") or "edited.blueprint"
    return Response(
        content=out,
        media_type="application/octet-stream",
        headers={
            "Content-Disposition": (f'attachment; filename="{fallback}"; '
                                    f"filename*=UTF-8''{quote(name)}"),
            "Cache-Control": "no-store",
            "X-Kiwi-Recoloured": str(summary["recoloured"]),
            "X-Kiwi-Rematerialised": str(summary["rematerialised"]),
            "X-Kiwi-Ignored": str(summary["ignored"]),
        },
    )


# --- model projects: a whole creature open at once --------------------------
# A mount is sixteen .blueprint files that only mean something together. These two
# endpoints open the set and write it back; ``app/trove/blueprint/model.py`` is the
# engine and every per-part tool above still works on one part of a project, because
# each part's own bytes go back to the browser with it.


async def _bp_model_files(file: UploadFile | None,
                          files: list[UploadFile]) -> tuple[str, dict, list, str]:
    """The project's source, however it was picked: a ``.tmod``/``.zip``, or loose
    ``.blueprint`` files straight off a computer. Returns ``(kind, header, files,
    name)``."""
    if file is not None and file.filename:
        data = await file.read()
        if not data:
            raise APIError(400, ErrorCode.bad_request, "That file is empty.")
        name = file.filename.rsplit("/", 1)[-1]
        try:
            kind, props, unpacked = await asyncio.to_thread(bp_model.unpack, data, name)
        except bp_editor.EditorError as e:
            raise _blueprint_editor_error(e) from e
        return kind, props, unpacked, name

    loose: list[tuple[str, bytes]] = []
    for part in files or []:
        if not part.filename:
            continue
        path = part.filename.replace("\\", "/").rsplit("/", 1)[-1]
        if not path.lower().endswith(".blueprint"):
            raise APIError(400, ErrorCode.bad_request, f"'{path}' isn't a .blueprint.")
        loose.append((path, await part.read()))
    if not loose:
        raise APIError(400, ErrorCode.bad_request,
                       "Send a .tmod or .zip, or the .blueprint files themselves.")
    return "files", {}, loose, "model"


@router.post("/site/blueprint-editor/model", response_class=JSONResponse)
async def site_blueprint_editor_model(
    file: UploadFile | None = File(default=None),
    files: list[UploadFile] = File(default=[]),
    _limit: None = _BP_EDITOR_LIMIT,
) -> JSONResponse:
    """Open every ``.blueprint`` in a mod as one editable model.

    Send a ``.tmod`` or ``.zip`` as ``file``, or the loose blueprints as ``files``.
    Each part comes back as the payload ``inspect`` returns plus its own bytes and the
    attach point it sits at, alongside the rig's rest pose - which is what lets the page
    draw the assembled creature and edit any part of it in place.

    The rig and the per-part attach points are resolved from the game's own prefab
    bindings (``rig_index.resolve``), the same authoritative map the 3D viewers use.
    There is no name-overlap fallback: a part the map doesn't place comes back with
    ``ap: null`` and is laid out beside the model rather than guessed onto a bone."""
    from app.trove.mods_hub import assembly, rig_index

    kind, _props, unpacked, name = await _bp_model_files(file, files)
    blueprints = bp_model.parts_of(unpacked)
    if not blueprints:
        raise APIError(400, ErrorCode.bad_request,
                       "There are no .blueprint files in there.")
    skeleton, attach = await rig_index.resolve(
        [bp_model.basename_of(p) for p, _ in blueprints])
    if skeleton and not assembly.has_baked_rig(skeleton):
        skeleton = None                    # known creature, no baked pose -> lay it out
    try:
        payload = await asyncio.to_thread(
            bp_model.open_project, unpacked, rig_name=skeleton, attach=attach, name=name)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    payload["source"] = kind
    return JSONResponse(payload, headers={"Cache-Control": "no-store"})


@router.get("/site/blueprint-editor/game-model", response_class=JSONResponse)
async def site_blueprint_editor_game_model(
    prefab: str = Query(..., min_length=1, description="Codex entry path (the creature's prefab)"),
    _limit: None = _BP_EDITOR_LIMIT,
) -> JSONResponse:
    """Open one of the GAME's own creatures as a model project.

    The codex knows a mount, ally, dragon or costume by its prefab path, and the prefab
    binds every blueprint the creature is made of to the bone it hangs off. So the page
    can search the codex, show the creature's render, and open the real thing to edit -
    which is where most mods start: an existing model, recoloured.

    Its parts are read out of the archived game files, and the same no-guess rule
    applies as everywhere else - a prefab the bindings don't know returns 404 rather
    than a plausible-looking creature made of parts that aren't its own."""
    from app.embed.service import _read_game_file
    from app.trove.mods_hub import assembly, rig_index
    from app.trove.mods_hub.trove_layout import LIVE_BRANCH, game_file_paths, nearest_path

    canonical, candidates = await rig_index.prefab_path(prefab)
    if not canonical:
        if candidates:
            raise APIError(400, ErrorCode.bad_request,
                           f"'{prefab}' names {len(candidates)} different creatures. "
                           f"Use the full path: {candidates[0]}")
        raise APIError(404, ErrorCode.not_found,
                       f"The game data has no creature at '{prefab}'.")
    skeleton, parts = await rig_index.creature_by_prefab(canonical)
    if not skeleton or not parts:
        raise APIError(404, ErrorCode.not_found, "That prefab binds no blueprint parts.")

    # Same resolution the embeddable viewer uses: Trove reuses part filenames across
    # skins and NPC sets, so each one is taken from the copy nearest this creature.
    paths = await game_file_paths(LIVE_BRANCH)
    files: list[tuple[str, bytes]] = []
    for basename in parts:
        game_path = nearest_path(paths.get(f"{basename}.blueprint", []), canonical)
        raw = await _read_game_file(game_path) if game_path else None
        if raw:
            files.append((f"{basename}.blueprint", raw))
    if not files:
        raise APIError(404, ErrorCode.not_found,
                       "None of that creature's parts are in the game archive.")
    rig_name = skeleton if assembly.has_baked_rig(skeleton) else None
    try:
        payload = await asyncio.to_thread(
            bp_model.open_project, files, rig_name=rig_name, attach=parts,
            name=rig_index.prefab_stem(canonical) + ".zip")
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    payload["source"] = "game"
    payload["prefab"] = canonical
    return JSONResponse(payload, headers={"Cache-Control": "no-store"})


@router.post("/site/blueprint-editor/model-save", response_class=Response)
async def site_blueprint_editor_model_save(
    file: UploadFile | None = File(default=None),
    files: list[UploadFile] = File(default=[]),
    paths: str = Form(default="[]"),
    edits: str = Form(default="{}"),
    moves: str = Form(default="{}"),
    name: str = Form(default=""),
    _limit: None = _BP_EDITOR_LIMIT,
) -> Response:
    """Write a project's edits back and hand the whole mod back.

    ``edits`` is ``{"<path in the mod>": [edit list], ...}`` - the same per-voxel edit
    entries ``save`` takes, one list per part, indexed against the order that part's
    payload came back in. The archive is posted again alongside them, so the server
    still holds nothing between opening a model and writing it.

    ``files`` + ``paths`` carry parts that weren't in the mod when it was opened - a
    blueprint dropped onto an open model - with the path each should be packed at. A
    path already in the mod is replaced; the rest are added.

    ``moves`` is ``{"<path>": [dx, dy, dz]}`` - a part slid along its bone. That moves
    the model's ORIGIN, not its voxels (``transform.move_on_rig``), which is the same
    number as moving its attachment point relative to the model.

    Everything that isn't an edited blueprint is carried through byte for byte: the
    config, the preview image, the textures, and any part that wasn't touched. A
    ``.tmod`` comes back as a ``.tmod``, anything else as a ``.zip``."""
    kind, props, unpacked, source_name = await _bp_model_files(file, [] if file else files)
    # A project of loose parts has no archive to take a name from - a game creature
    # opened by prefab, for instance - so the page says what it should be called.
    name = name.rsplit("/", 1)[-1] or source_name
    try:
        parsed = json.loads(edits or "{}")
        parsed_moves = json.loads(moves or "{}")
        extra_paths = json.loads(paths or "[]")
    except ValueError:
        raise APIError(400, ErrorCode.bad_request,
                       "The edit list wasn't understood.") from None
    if not isinstance(parsed, dict) or not isinstance(extra_paths, list) \
            or not isinstance(parsed_moves, dict):
        raise APIError(400, ErrorCode.bad_request, "The edit list wasn't understood.")
    extra: list[tuple[str, bytes]] = []
    if file:                          # loose-file projects post everything as `files`
        if len(extra_paths) != len(files or []):
            raise APIError(400, ErrorCode.bad_request,
                           "The added parts didn't match the paths that arrived.")
        for part, want in zip(files or [], extra_paths, strict=True):
            extra.append((bp_model.pack_path(str(want) or part.filename or ""),
                          await part.read()))
    try:
        edited, summary = await asyncio.to_thread(
            bp_model.apply_project, unpacked, parsed, extra, parsed_moves)
        out, ext = await asyncio.to_thread(bp_model.repack, kind, props, edited)
    except bp_editor.EditorError as e:
        raise _blueprint_editor_error(e) from e
    stem = re.sub(r"\.(tmod|zip)$", "", name, flags=re.I) or "model"
    fallback = re.sub(r'[^\x20-\x7e]', "_", f"{stem}.{ext}").replace('"', "")
    return Response(
        content=out,
        media_type="application/octet-stream",
        headers={
            "Content-Disposition": (f'attachment; filename="{fallback}"; '
                                    f"filename*=UTF-8''{quote(stem + '.' + ext)}"),
            "Cache-Control": "no-store",
            "X-Kiwi-Summary": json.dumps(summary),
        },
    )


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


@router.get("/site/mods/releases/{release_id}/swfs", response_class=JSONResponse)
async def site_mods_release_swfs(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The .swf movies in a release's build - drives the inspector's Code button
    (only shown when the build ships one and this server can decompile)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.list_release_swfs(release),
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/mods/releases/{release_id}/swf/scripts", response_class=JSONResponse)
async def site_mods_release_swf_scripts(
    request: Request, release_id: str,
    path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """One movie decompiled back to ActionScript - the whole class tree in one body,
    which is what the code viewer reads and searches client-side."""
    from app.trove.swf import service as swf_service

    await swf_service.decompile_throttle(request)
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.release_swf_scripts(release, path),
                        headers={"Cache-Control": "public, max-age=300"})


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
async def site_mods_image(sha: str, w: int | None = Query(default=None)) -> Response:
    if w is not None and w not in mods_store.THUMB_WIDTHS:
        raise HTTPException(status_code=400,
                            detail=f"w must be one of {', '.join(map(str, mods_store.THUMB_WIDTHS))}")
    got = await mods_hub_service.get_image(sha, w)
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
