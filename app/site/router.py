"""Routes for the BetterTroveTools showcase site (`trove.aallyn.net`).

Page routes: ``/``, ``/documentation``, ``/commands``, ``/leaderboards``,
``/updates``, ``/support``.

Plus a small JSON surface under ``/site/*`` (leaderboards + updates +
market + codexes proxies, screenshots index) that the page-side JS calls
same-origin so visitors don't get throttled by per-token caps.

Templates were ported from a Quart app; the old ``url_for('static', ...)``
calls were rewritten to hardcoded ``/static/...`` paths (the mount lives
at ``/static`` in ``app/main.py``), so the templates render straight
through Jinja2Templates without a custom url-builder.
"""

import logging
import re
from pathlib import Path

from fastapi import APIRouter, Depends, HTTPException, Query, Request
from fastapi.responses import HTMLResponse, JSONResponse, RedirectResponse, Response
from fastapi.templating import Jinja2Templates

from app.admin import runtime_config
from app.core import features as feature_flags
from app.core.config import settings
from app.site_auth.dependencies import get_optional_site_user
from app.site_auth.models import SiteUser
from app.trove import server_time as trove_server_time
from app.trove import status as trove_status
from app.trove.codexes import read as codexes_read
from app.trove.codexes.types import ALL_TYPES as CODEX_TYPES
from app.trove.render.service import render_blueprint_cached
from app.trove.leaderboards import activity as leaderboards_activity
from app.trove.leaderboards import cache as leaderboards_cache
from app.trove.leaderboards import class_activity as leaderboards_class_activity
from app.trove.leaderboards import detection as leaderboards_detection
from app.trove.leaderboards import service as leaderboards_service
from app.trove.mods_hub import service as mods_hub_service
from app.trove.modpacks import service as modpacks_service
from app.trove.updates import compare as updates_compare
from app.trove.updates import read as updates_read
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES as UPDATE_BRANCHES

logger = logging.getLogger("kiwi.site.router")

# Filename extensions accepted as Trove screenshots for the hero slideshow.
# Anything else in the folder (READMEs, .DS_Store, etc.) is silently skipped.
_SCREENSHOT_EXTS = {".webp", ".png", ".jpg", ".jpeg", ".gif"}

# Site features surfaced to templates: template context key (also the
# ``request.state`` attr) → runtime-config flag. Each is a master switch the
# admin Configuration tab flips; resolved once per request in
# ``_resolve_feature_flags`` and injected into every template by
# ``_feature_context`` so the navbar can hide a disabled feature's link.
_SITE_FEATURE_FLAGS = {
    "mods_hub_enabled": feature_flags.MODS_HUB_FLAG,
    "market_enabled": feature_flags.MARKET_FLAG,
    "leaderboards_enabled": feature_flags.LEADERBOARDS_FLAG,
    "player_activity_enabled": feature_flags.PLAYER_ACTIVITY_FLAG,
    "class_activity_enabled": feature_flags.CLASS_ACTIVITY_FLAG,
    "clubs_enabled": feature_flags.CLUBS_FLAG,
    "updates_enabled": feature_flags.UPDATES_FLAG,
    "codexes_enabled": feature_flags.CODEXES_FLAG,
    "server_status_enabled": feature_flags.SERVER_STATUS_FLAG,
    "giveaways_enabled": feature_flags.GIVEAWAYS_FLAG,
    "commands_enabled": feature_flags.COMMANDS_FLAG,
    "server_time_enabled": feature_flags.SERVER_TIME_FLAG,
}


def _feature_blocks(p: str, f: dict) -> bool:
    """True if the request path ``p`` belongs to a feature that is OFF (``f`` is
    the resolved flag map). Covers both the page route and that feature's
    ``/site/<feature>/*`` JSON proxies + OG images, so a disabled feature is
    hidden, not just unlinked."""
    # Mods Hub + Modpacks ride the Mods Hub toggle (modpacks are a layer over it).
    if not f["mods_hub_enabled"] and (
        p == "/mods" or p.startswith("/mods/") or p.startswith("/site/mods/")
        or p == "/modpacks" or p.startswith("/modpacks/")
        or p.startswith("/site/modpacks/")
    ):
        return True
    if not f["market_enabled"] and (p == "/market" or p.startswith("/site/market/")):
        return True
    # Leaderboards: board browser + per-player profile pages. The activity /
    # class-activity proxies share the /site/leaderboards/ root but have their
    # own toggles, so they're explicitly excluded here.
    if not f["leaderboards_enabled"] and (
        p == "/leaderboards"
        or p.startswith("/player/")
        or (p.startswith("/site/leaderboards/")
            and not p.startswith("/site/leaderboards/activity")
            and not p.startswith("/site/leaderboards/class-activity"))
    ):
        return True
    if not f["player_activity_enabled"] and (
        p == "/activity" or p.startswith("/activity/")           # page + /activity/og.png
        or p.startswith("/site/leaderboards/activity")
    ):
        return True
    if not f["class_activity_enabled"] and (
        p == "/class-activity"
        or p.startswith("/site/leaderboards/class-activity")
    ):
        return True
    if not f["clubs_enabled"] and p == "/clubs":
        return True
    if not f["updates_enabled"] and (p == "/updates" or p.startswith("/site/updates/")):
        return True
    if not f["codexes_enabled"] and (p == "/codexes" or p.startswith("/site/codexes/")):
        return True
    if not f["server_status_enabled"] and (
        p == "/status" or p.startswith("/status/")               # page + /status/og.png
        or p.startswith("/site/trove-status")
    ):
        return True
    if not f["giveaways_enabled"] and (p == "/giveaways" or p == "/site/giveaways"):
        return True
    if not f["commands_enabled"] and p == "/commands":
        return True
    if not f["server_time_enabled"] and (
        p == "/server-time" or p == "/site/server-time"
    ):
        return True
    return False


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


@router.get("/", response_class=HTMLResponse)
async def home(request: Request) -> HTMLResponse:
    """The BetterTroveTools landing page."""
    return _TEMPLATES.TemplateResponse(
        request, "index.html", {"discord_install_url": settings.discord_install_link},
    )


@router.get("/documentation", response_class=HTMLResponse)
async def documentation(request: Request) -> HTMLResponse:
    """The user manual."""
    return _TEMPLATES.TemplateResponse(request, "docs.html", {})


@router.get("/commands", response_class=HTMLResponse)
async def commands(request: Request) -> HTMLResponse:
    """In-game Trove slash-command reference. Page shell + JS only -
    actual command data lives in ``site/static/commands.json`` and is
    fetched + rendered client-side so language switches don't reload."""
    return _TEMPLATES.TemplateResponse(request, "commands.html", {})


@router.get("/support", response_class=HTMLResponse)
async def support(request: Request) -> HTMLResponse:
    """Dedicated 'Support the project' page - landing for the red-heart
    navbar link. The bottom-right floating widget is also rendered on
    every page; this one gives a richer pitch for visitors who want a
    full read on what the donations actually fund. Renders the supporters
    credits list (managed via /admin/supporters)."""
    from app.supporters import service as supporters_service
    return _TEMPLATES.TemplateResponse(
        request, "support.html", {"supporters": await supporters_service.list_public()},
    )


@router.get("/status", response_class=HTMLResponse)
async def status_page(request: Request) -> HTMLResponse:
    """Dedicated Trove server-status page - live Live/PTS state plus a
    downtime-history timeline. Page shell + JS; data comes from
    ``/site/trove-status`` + ``/site/trove-status/history``."""
    return _TEMPLATES.TemplateResponse(request, "status.html", {})


@router.get("/server-time", response_class=HTMLResponse)
async def server_time_page(request: Request) -> HTMLResponse:
    """Dedicated server-time page - a big live Trove server clock (UTC-11), the
    same instant across common player time zones, daily/weekly reset countdowns,
    and a Discord-timestamp maker. Page shell + JS; the clock anchors to
    ``/site/server-time`` (falling back to the local clock)."""
    return _TEMPLATES.TemplateResponse(request, "server-time.html", {})


@router.get("/site/server-time", response_class=JSONResponse)
async def site_server_time() -> JSONResponse:
    """Authoritative Trove server time for the /server-time page - same payload as
    the public ``/v1/rotations/server-time``, served same-origin so the page can
    anchor its clock without CORS. Short cache; the page re-fetches each minute."""
    return JSONResponse(
        trove_server_time.server_time(),
        headers={"Cache-Control": "public, max-age=15"},
    )


@router.get("/terms", response_class=HTMLResponse)
async def terms(request: Request) -> HTMLResponse:
    """Terms of Service - reachable from the footer fine print (no navbar link)."""
    return _TEMPLATES.TemplateResponse(request, "terms.html", {})


@router.get("/privacy", response_class=HTMLResponse)
async def privacy(request: Request) -> HTMLResponse:
    """Privacy Policy - reachable from the footer fine print (no navbar link)."""
    return _TEMPLATES.TemplateResponse(request, "privacy.html", {})


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
    """In-game marketplace browser (Beta). Reads from the
    ``market_listings`` collection via the /site/market/* proxies
    below - bypasses the public API's per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "market.html", {})


@router.get("/codexes", response_class=HTMLResponse)
async def codexes(request: Request) -> HTMLResponse:
    """Codexes browser - parsed Trove game data (allies, mounts, dragons, mementos,
    recipes, items, fish, badges) with mastery / power rank / stat & ability bonuses.
    Reads the same data as ``/v1/codexes/*`` via the ``/site/codexes/*`` proxies
    below (same-origin, no per-token caps)."""
    return _TEMPLATES.TemplateResponse(request, "codexes.html", {})


@router.get("/mods", response_class=HTMLResponse)
async def mods_hub(request: Request) -> HTMLResponse:
    """Mods Hub - browse + download shared Trove mods (public, no login). The
    grid + search are painted client-side from the ``/site/mods/*`` proxies
    below; creating/developing a mod needs a signed-in site user."""
    return _TEMPLATES.TemplateResponse(request, "mods.html", {})


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
    ctx = {
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
    ctx = {
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
    })
    return _TEMPLATES.TemplateResponse(request, "mods_profile.html", ctx)


@router.get("/modpacks", response_class=HTMLResponse)
async def modpacks_hub(request: Request) -> HTMLResponse:
    """Modpacks - browse + download user-curated bundles of hub mods (public, no
    login). Grid painted client-side from ``/site/modpacks/*``; creating one needs
    a signed-in site user."""
    return _TEMPLATES.TemplateResponse(request, "modpacks.html", {})


@router.get("/modpacks/{handle}/{slug}", response_class=HTMLResponse)
async def modpack_project_page(request: Request, handle: str, slug: str) -> HTMLResponse:
    """A single modpack's page: banner, description, variants and the mods each
    bundles (with version per mod), plus download. Owner gets the inline editor.
    Client-rendered from ``/site/modpacks/*``; we fetch it here (anonymously) to
    emit real Open Graph / Twitter-card tags for link unfurls. Drafts / private /
    not-found fall back to generic tags so nothing private leaks into an embed."""
    base = settings.app_url.rstrip("/")
    page_url = f"{base}/modpacks/{handle}/{slug}"
    ctx = {
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
    return _TEMPLATES.TemplateResponse(request, "modpacks_project.html", ctx)


@router.get("/giveaways", response_class=HTMLResponse)
async def giveaways(request: Request) -> HTMLResponse:
    """Public giveaways page. Lists open / upcoming / past draws (data from
    the /site/giveaways proxy); entering needs a signed-in site user."""
    return _TEMPLATES.TemplateResponse(request, "giveaways.html", {})


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
    })


@router.get("/class-activity", response_class=HTMLResponse)
async def class_activity_page(request: Request) -> HTMLResponse:
    """Class Activity page - per-class active players over time (multi-line) plus
    a class player-share donut, derived from the Effort/Paragon leaderboards."""
    return _TEMPLATES.TemplateResponse(request, "class-activity.html", {})


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


# --- /site/market/* - same-origin JSON proxies for the page ---------------
# Same shape as /site/leaderboards/* and /site/updates/* - call the
# service layer directly, skip the public API's auth + scope + rate-limit
# pipeline. Same trade-off: data is already public, no reason to throttle
# the same browsers we'd be happy to serve via api.aallyn.net anyway.

@router.get("/site/market/items", response_class=JSONResponse)
async def site_market_items() -> JSONResponse:
    from app.trove.market import service as market_service
    items = await market_service.list_distinct_items()
    return JSONResponse(
        {"items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=60"},
    )


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


# --- /site/codexes/* - same-origin JSON proxies for the /codexes page ------
# Mirror the public ``/v1/codexes/*`` surface but tokenless + same-origin. The
# two "modes" are branches (live-us / pts); default to live-us.

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
) -> Response:
    """Same-origin PNG render of a codex item's blueprint (the card thumbnail).
    Cached in Redis; 404 on a missing/unrenderable blueprint so the grid's
    ``<img onerror>`` hides cleanly."""
    _site_codex_branch(branch)
    try:
        png = await render_blueprint_cached(blueprint, dim=dim, branch=branch)
    except Exception:  # noqa: BLE001 - never let a render error break the grid
        logger.warning("codex render failed for %r", blueprint, exc_info=True)
        png = None
    if png is None:
        raise HTTPException(status_code=404, detail="no render")
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


@router.get("/leaderboards", response_class=HTMLResponse)
async def leaderboards(request: Request) -> HTMLResponse:
    """Trove leaderboards browser - public site read of the same data the
    ``/v1/leaderboards/*`` API exposes. The page hits dedicated JSON
    endpoints under ``/site/leaderboards/*`` (see below) which bypass the
    public API's token/scope/rate-limit pipeline and call the service
    layer directly. The data is public anyway, so the bypass costs us
    nothing and avoids subjecting site browsers to per-token caps.

    The two anti-cheat tabs are gated on the cheater/alt-cluster calculation
    switches and rendered (or not) server-side, so a disabled tab is gone on
    first paint - no dependency on JS / the minified bundle."""
    cheaters_on = await feature_flags.is_enabled(feature_flags.CHEATER_DETECTION_FLAG)
    clusters_on = await feature_flags.is_enabled(feature_flags.ALT_CLUSTERS_FLAG)
    return _TEMPLATES.TemplateResponse(request, "leaderboards.html", {
        "cheater_detection_enabled": cheaters_on,
        "alt_clusters_enabled": cheaters_on and clusters_on,
    })


# --- /leaderboards JSON endpoints ------------------------------------------
# These mirror the four read-side helpers from app/trove/router.py but skip
# the TokenContext dep + archive-rate-limit. They're intentionally NOT
# include_in_schema (the router already opts out) - the public surface is
# still /v1/leaderboards/*, this is just a site convenience.

@router.get("/site/leaderboards/config", response_class=JSONResponse)
async def site_lb_config() -> JSONResponse:
    """Runtime tunables the leaderboards page needs to render its chrome.

    The hot-retention window (so the subtitle's "N-day live retention" line
    tracks master-panel changes within the 5s runtime_config cache window), plus
    the cheater/alt-cluster calculation switches so the page can hide the
    Possible-cheaters / Alt-clusters tabs when their compute is disabled."""
    days = await runtime_config.get_setting("leaderboards_hot_retention_days")
    cheaters_on = await feature_flags.is_enabled(feature_flags.CHEATER_DETECTION_FLAG)
    clusters_on = await feature_flags.is_enabled(feature_flags.ALT_CLUSTERS_FLAG)
    return JSONResponse(
        {
            "hot_retention_days": int(days),
            "cheater_detection_enabled": cheaters_on,
            # Alt-clusters only meaningful when cheater detection runs at all.
            "alt_clusters_enabled": cheaters_on and clusters_on,
        },
        headers={"Cache-Control": "public, max-age=30"},
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


@router.get("/site/leaderboards/boards", response_class=JSONResponse)
async def site_lb_boards(
    created_at: int = Query(..., description="Anchor in unix seconds"),
) -> JSONResponse:
    rows = await leaderboards_cache.get_boards(created_at)
    return JSONResponse(
        {"created_at": created_at, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=60"},
    )


# LITERAL-prefix routes must come BEFORE the ``/{uuid}/...`` catch-alls
# below - FastAPI matches in declaration order, and a path-param int
# validator on "activity" / "cheaters" would 422 (not fall through) if
# the catch-all matched first. Same dance as ``/players/{name}/...`` -
# put the named segments above the parameterised ones.
@router.get("/site/leaderboards/activity", response_class=JSONResponse)
async def site_lb_activity() -> JSONResponse:
    """Same payload as the public `/v1/activity/current` but served
    same-origin so the page can fetch without CORS."""
    payload = await leaderboards_activity.estimate_active_players()
    # no-cache: the chart must reflect a new capture / a master Reset+rebuild
    # immediately, not 30 min later. The query is a cheap indexed read.
    return JSONResponse(payload, headers={"Cache-Control": "no-cache"})


@router.get("/site/leaderboards/activity/history", response_class=JSONResponse)
async def site_lb_activity_history(days: int = 7) -> JSONResponse:
    """Same payload as the public ``/v1/activity/history`` - same-origin
    proxy so the showcase page can fetch without CORS / token gymnastics.
    Returns a time-series of activity estimates with both raw counts
    and per-hour rates, the latter being what the chart line plots so
    missed-capture gaps don't show as spikes."""
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


@router.get("/site/leaderboards/class-activity/current", response_class=JSONResponse)
async def site_lb_class_activity_current() -> JSONResponse:
    """Same payload as `/v1/class-activity/current`, served same-origin for the
    Class Activity page (no-cache so a new capture / master rebuild shows at once)."""
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
    ``/v1/misc/trove-status``, served same-origin so the landing + status
    pages can fetch it without CORS."""
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
    """Possible-cheaters analysis for the leaderboards page. Same payload
    as the public ``GET /v1/leaderboards/cheaters`` endpoint - served
    here so the page can fetch same-origin (no CORS dance from
    trove.aallyn.net). The detection module itself caches the result
    for ``cheaters_cache_ttl_seconds`` so this is cheap to call."""
    payload = await leaderboards_detection.detect_possible_cheaters()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    return JSONResponse(
        payload,
        headers={"Cache-Control": f"public, max-age={ttl}"},
    )


@router.get("/site/leaderboards/{uuid}/entries", response_class=JSONResponse)
async def site_lb_entries(
    uuid: int,
    created_at: int = Query(..., description="Anchor in unix seconds"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    items, total, comparison = await leaderboards_cache.get_entries(
        uuid, created_at, limit=limit, offset=offset,
    )
    return JSONResponse(
        {
            "uuid": uuid, "created_at": created_at,
            "items": items, "count": len(items), "total": total,
            "comparison": comparison,
        },
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/players/{player_name}/history",
            response_class=JSONResponse)
async def site_lb_player_history(
    player_name: str,
    limit: int = Query(default=50, ge=1, le=500),
    uuid: int | None = Query(default=None),
) -> JSONResponse:
    rows = await leaderboards_service.player_history(
        player_name, limit=limit, uuid=uuid, with_deltas=True,
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
    """Public player profile (appearances + verified-claim flag), same-origin for
    the /player/<name> page."""
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
    """Trove updates browser - public site read of the same archive that
    ``/v1/updates/*`` exposes. The page hits the JSON helpers below
    (``/site/updates/*``) which bypass the token/scope/rate-limit pipeline
    so site browsers don't get throttled by per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "updates.html", {})


# --- /updates JSON endpoints ----------------------------------------------
# Mirrors the public ``/v1/updates/*`` surface but tokenless + same-origin
# (no CORS, no scope, no archive rate-limit). Implementation is one-line
# calls into the shared read module.

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
        {**b, "last_probe_at": b["last_probe_at"].isoformat() if b.get("last_probe_at") else None}
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
            "captured_at": d.captured_at.isoformat() if d.captured_at else None,
            "completed_at": d.completed_at.isoformat() if d.completed_at else None,
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
    return JSONResponse(
        {"branch": branch, "prefix": prefix,
         "entries": entries, "count": len(entries)},
        headers={"Cache-Control": "public, max-age=60"},
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
        {**r, "captured_at": r["captured_at"].isoformat() if r.get("captured_at") else None}
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
    ``/v1/updates/{branch}/file/compare`` - kept here so the page can
    fetch same-origin and avoid the OpenAPI surface for what is, after
    all, a UI-driven request."""
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
        "captured_at": v_from.captured_at.isoformat() if v_from.captured_at else None,
        "content_sha256": a["content_sha256"] if a else None,
        "size": a["size"] if a else 0,
    }
    b_info = {
        "ordinal": v_to.ordinal, "version_tag": v_to.version_tag,
        "captured_at": v_to.captured_at.isoformat() if v_to.captured_at else None,
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


# --- /site/mods/* - same-origin proxies for the Mods Hub pages -------------
# Reads mirror the public ``/v1/mods/hub/*`` surface but tokenless + same-
# origin, and they pass the *site* user (Discord login) as the viewer - so the
# owner sees their own drafts + owner-only controls, which the /v1 reads (API
# token, no site-user concept) never reveal. Writes still go to /v1/mods/hub/*
# directly with the site-auth bearer (CORS-allowed for trove.aallyn.net).

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


@router.get("/site/mods/releases/{release_id}/assembled", response_class=JSONResponse)
async def site_mods_assembled(
    release_id: str, viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """The release's blueprint parts assembled onto their creature rig (rest +
    animations) for the web model viewer."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    model = await mods_hub_service.assemble_release_model(release)
    if model is None:
        from app.core.errors import APIError, ErrorCode
        raise APIError(404, ErrorCode.not_found, "No assemblable creature for this mod.")
    return JSONResponse(model, headers={"Cache-Control": "public, max-age=300"})


@router.get("/site/rigs/{skeleton}/anim/{name}", response_class=JSONResponse)
async def site_rig_animation(skeleton: str, name: str) -> JSONResponse:
    """Lazily-loaded baked animation frames for a creature rig (the model viewer fetches
    these on demand when a clip is played). Public, shared across mods using the rig."""
    anim = await mods_hub_service.load_rig_animation(skeleton, name)
    return JSONResponse(anim, headers={"Cache-Control": "public, max-age=3600"})


@router.get("/site/mods/releases/{release_id}/blueprint", response_class=JSONResponse)
async def site_mods_blueprint(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    viewer: SiteUser | None = Depends(get_optional_site_user),
) -> JSONResponse:
    """Decoded voxel data for one .blueprint in a release (web 3D viewer)."""
    release, _ = await mods_hub_service.release_with_project(release_id, viewer)
    return JSONResponse(await mods_hub_service.decode_release_blueprint(release, path),
                        headers={"Cache-Control": "public, max-age=300"})


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
    """List of Trove screenshots for the landing-page hero slideshow.

    Reads ``site/static/trove-screens/`` and returns every image (by file
    extension whitelist) sorted alphabetically. Lets the user drop new
    screenshots into the folder and have them appear on the next page
    load without an HTML edit. Filenames are exposed as URLs only - full
    paths never leak.

    Empty list (folder missing, no recognised images) is a clean OK that
    the landing-page JS treats as "no slideshow"; the orbs + grid stay.
    """
    folder = Path(settings.site_root) / "static" / "trove-screens"
    files: list[str] = []
    if folder.is_dir():
        for path in sorted(folder.iterdir()):
            if path.is_file() and path.suffix.lower() in _SCREENSHOT_EXTS:
                files.append(f"/static/trove-screens/{path.name}")
    # 60-second client cache: long enough that a back-button hit doesn't
    # re-list the folder, short enough that adding a new screenshot shows
    # up within a minute without a hard refresh.
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
