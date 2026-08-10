"""HTML page routes for the showcase website (``trove.aallyn.net``).

These render Jinja templates from ``site/templates``. The client-side JS then
hydrates each page by calling the API's ``/site/*`` + ``/v1/*`` endpoints
cross-origin (see ``site/static/_site_util.js`` ``window.API_BASE``). The few
pages that need data server-side (OG meta for link unfurls, the browse / clubs /
support first paint, and the leaderboards tab gating) fetch it from the API over
HTTP via ``internal_get`` - this module imports NO ``app.trove.*`` service
modules and opens no database connection.

The data plane - every ``/site/*`` JSON/binary proxy, the OG PNG renders and the
CAS reads - lives on the API in ``app/site/router.py``; robots/sitemap live in
``app/web/meta.py``.
"""
import logging
import re
from pathlib import Path
from urllib.parse import quote

from fastapi import APIRouter, Depends, HTTPException, Request
from fastapi.responses import HTMLResponse
from fastapi.templating import Jinja2Templates

from app.core.config import settings
from app.core.internal_api import internal_get
from app.site import classes_page, commands_page, ssr
from app.site.feature_map import SITE_FEATURE_FLAGS
from app.web import feature_flags as web_flags

logger = logging.getLogger("kiwi.web.pages")


async def _ssr_fetch(path: str, params: dict | None = None) -> object | None:
    """Data source for the server-rendered first paint (see ``app/site/ssr.py``).

    In this container the ``/site/*`` proxies live on the API, so the SSR
    builders' fetches go over the compose network. ``internal_get`` never raises
    - a failure returns ``None`` and the builder falls back to an empty model, so
    a blip on the data plane costs the crawlable copy, never the page."""
    return await internal_get(path, params, timeout=3.0)


def _flag_map(request: Request) -> dict[str, bool]:
    """The resolved feature flags as a plain dict - what the SSR builders need to
    skip fetching for a feature that's switched off."""
    return {attr: bool(getattr(request.state, attr, True))
            for attr in SITE_FEATURE_FLAGS}


_API = settings.api_url.rstrip("/")   # data-plane origin (api.aallyn.net)
_APP = settings.app_url.rstrip("/")   # this site's own origin (trove.aallyn.net)

_TEMPLATES = Jinja2Templates(
    directory=str(Path(settings.site_root) / "templates"),
    context_processors=[web_flags.context],
)

router = APIRouter(
    tags=["web"], include_in_schema=False,
    dependencies=[Depends(web_flags.resolve)],
)


# ── Embeddable viewer ──────────────────────────────────────────────────────

@router.get("/embed/viewer", response_class=HTMLResponse, include_in_schema=False)
async def embed_viewer(
    request: Request,
    release: str | None = None,
    tmod: str | None = None,
    game: str | None = None,
    dress: str | None = None,
    path: str | None = None,
    mode: str = "auto",
    theme: str = "dark",
) -> HTMLResponse:
    """The chrome-free viewer other sites put in an ``<iframe>``.

    Served from THIS host on purpose: the website is the only origin allowed to be
    framed (the API host refuses framing outright), so partners embed a brand URL
    and the data plane stays unframable.

    Presentation only, like every other page here - the shell is rendered and the
    client fetches its manifest/model/effect from the API cross-origin, the same way
    the rest of the site gets its data. Params are handed to the client untouched: a
    bad one should paint a readable message inside the frame, not a bare error inside
    somebody else's page."""
    return _TEMPLATES.TemplateResponse(request, "embed_viewer.html", {
        "release": release or "", "tmod": tmod or "", "game": game or "",
        "dress": dress or "",
        "path": path or "", "mode": mode if mode in
        ("auto", "blueprint", "assembled", "vfx") else "auto",
        "theme": theme if theme in ("dark", "light") else "dark",
        "api_base": _API,
        "app_url": _APP,
    })


# ── Static / marketing / legal (no backend) ────────────────────────────────
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


async def _all_public_modpack_cards(cap: int = 25_000) -> list[dict]:
    """Page through the tokenless ``/site/modpacks/projects`` proxy and return
    every public modpack card. Truncates at ``cap`` with a warning - a silent cap
    would read as 'whole catalog indexed' when it isn't."""
    out: list[dict] = []
    offset, page = 0, 100   # the /site proxy caps limit at 100
    while True:
        data = await internal_get("/site/modpacks/projects",
                                  {"limit": page, "offset": offset})
        if not isinstance(data, dict):
            break
        rows = data.get("items") or []
        total = data.get("total") or 0
        out.extend(rows)
        offset += page
        if not rows or offset >= total or len(out) >= cap:
            break
    if len(out) > cap:
        logger.warning("browse: modpack catalog (%d) exceeds cap %d - truncating",
                       len(out), cap)
        out = out[:cap]
    return out


@router.get("/browse", response_class=HTMLResponse)
async def browse_index(request: Request) -> HTMLResponse:
    """Human-readable site index ("HTML sitemap"): real ``<a>`` links to every
    public modpack page. The catalog grids render client-side, so mod/modpack pages
    otherwise have no crawlable internal links - only the XML sitemap. Linked from
    the footer so it's reachable everywhere."""
    packs: list[dict] = []
    if getattr(request.state, "mods_hub_enabled", True):
        packs = await _all_public_modpack_cards()
    packs.sort(key=lambda c: (c.get("title") or c.get("slug") or "").lower())
    return _TEMPLATES.TemplateResponse(request, "browse.html", {"modpacks": packs})


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
    data = await internal_get("/v1/misc/supporters")
    supporters = data.get("supporters") if isinstance(data, dict) else None
    return _TEMPLATES.TemplateResponse(
        request, "support.html", {"supporters": supporters or []},
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


@router.get("/changelog", response_class=HTMLResponse)
async def changelog(request: Request) -> HTMLResponse:
    """Website changelog - the commit history of the site's own open-source repo
    (``settings.site_source_repo``), surfaced for transparency. Shell + JS; the
    data comes from the API's ``/site/changelog`` proxy."""
    return _TEMPLATES.TemplateResponse(
        request, "changelog.html",
        {"repo_url": f"https://github.com/{settings.site_source_repo}"},
    )


# ── Auth pages (client-side token; no server gate) ─────────────────────────
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


# ── Feature pages (shell + JS; data via the API's /site/* proxies) ─────────
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
    all data comes from ``/site/mods/*`` (on the API).

    The page is client-rendered, but we fetch the mod here (anonymously, over the
    internal API) to emit real Open Graph / Twitter-card tags, so link unfurls
    (Discord, Twitter, …) show the actual mod - title, summary and banner. Drafts /
    private / not-found fall back to generic tags so nothing private leaks into an
    embed; the page itself still renders (the client reveals owner-only content
    when logged in)."""
    page_url = f"{_APP}/mods/{handle}/{slug}"
    ctx: dict = {
        "slug": slug, "handle": handle, "og_page_url": page_url,
        "page_title": f"{slug} · Trove mod · Better Trove Tools",
        "og_title": f"{slug} · Trove mod",
        "og_desc": "A Trove mod shared on the Better Trove Tools Mods Hub.",
        "og_image": f"{_APP}/static/assets/favicon.png",
        "og_image_alt": "Better Trove Tools",
        "og_author": "",
        "twitter_card": "summary",
    }
    project = await internal_get(f"/site/mods/projects/{quote(handle)}/{quote(slug)}")
    if isinstance(project, dict):
        title = project.get("title") or slug
        owner = project.get("owner_username") or ""
        desc = (project.get("summary") or "").strip() \
            or _plain_excerpt(project.get("description")) \
            or (f"A Trove mod by {owner}." if owner else ctx["og_desc"])
        previews = project.get("preview_shas") or []
        img_sha = project.get("banner_sha") or (previews[0] if previews else None)
        ctx.update({
            "page_title": f"{title} · Trove mod · Better Trove Tools",
            "og_title": f"{title} · Trove mod",
            "og_desc": desc[:300],
            "og_image": f"{_API}/site/mods/image/{img_sha}" if img_sha else ctx["og_image"],
            "og_image_alt": title,
            "og_author": owner,
            "twitter_card": "summary_large_image" if img_sha else "summary",
        })
        # Same payload, no extra round-trip: the mod's title, description, tags
        # and stats become server-rendered body copy, not just meta tags.
        ctx["ssr"] = ssr.mod_project_view(project)
    return _TEMPLATES.TemplateResponse(request, "mods_project.html", ctx)


@router.get("/mods/{handle}", response_class=HTMLResponse)
async def mods_profile_page(request: Request, handle: str) -> HTMLResponse:
    """A modder's profile page (`/mods/<handle>`): avatar, banner, README, socials
    and their mods. Client-rendered from ``/site/mods/profile/<handle>``; this route
    fills per-modder Open Graph tags so a shared profile link unfurls properly.

    A profile only exists once the modder has ≥1 public mod, so this 404s otherwise
    (the front-facing 404 handler serves the themed HTML page)."""
    page_url = f"{_APP}/mods/{handle}"
    data = await internal_get(f"/site/mods/profile/{quote(handle)}")
    if not isinstance(data, dict):
        raise HTTPException(status_code=404, detail="No such modder.")
    ctx: dict = {
        "handle": handle, "og_page_url": page_url,
        "page_title": f"{handle} · Trove modder · Better Trove Tools",
        "og_title": f"{handle} · Trove modder",
        "og_desc": f"{handle}'s mods on the Better Trove Tools Mods Hub.",
        "og_image": f"{_APP}/static/assets/favicon.png",
        "og_image_alt": handle,
        "og_author": "",
        "twitter_card": "summary",
    }
    name = data.get("display_name") or handle
    desc = (data.get("tagline") or "").strip() or _plain_excerpt(data.get("readme")) \
        or f"{name}'s mods on the Better Trove Tools Mods Hub."
    img = data.get("banner_url") or data.get("avatar_url")
    ctx.update({
        "page_title": f"{name} · Trove modder · Better Trove Tools",
        "og_title": f"{name} · Trove modder",
        "og_desc": desc[:300],
        "og_image": img or ctx["og_image"],
        "og_image_alt": name,
        "og_author": name,
        "twitter_card": "summary_large_image" if data.get("banner_url") else "summary",
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
    Client-rendered from ``/site/modpacks/*``; we fetch it here (anonymously, over
    the internal API) to emit real Open Graph / Twitter-card tags for link unfurls.
    Drafts / private / not-found fall back to generic tags so nothing private leaks
    into an embed."""
    page_url = f"{_APP}/modpacks/{handle}/{slug}"
    ctx: dict = {
        "slug": slug, "handle": handle, "og_page_url": page_url,
        "page_title": f"{slug} · Trove modpack · Better Trove Tools",
        "og_title": f"{slug} · Trove modpack",
        "og_desc": "A Trove modpack shared on Better Trove Tools.",
        "og_image": f"{_APP}/static/assets/favicon.png",
        "og_image_alt": "Better Trove Tools",
        "og_author": "",
        "twitter_card": "summary",
    }
    pack = await internal_get(f"/site/modpacks/projects/{quote(handle)}/{quote(slug)}")
    if isinstance(pack, dict):
        title = pack.get("title") or slug
        owner = pack.get("owner_username") or ""
        desc = (pack.get("summary") or "").strip() \
            or _plain_excerpt(pack.get("description")) \
            or (f"A Trove modpack by {owner}." if owner else ctx["og_desc"])
        previews = pack.get("preview_shas") or []
        img_sha = pack.get("banner_sha") or (previews[0] if previews else None)
        ctx.update({
            "page_title": f"{title} · Trove modpack · Better Trove Tools",
            "og_title": f"{title} · Trove modpack",
            "og_desc": desc[:300],
            "og_image": f"{_API}/site/mods/image/{img_sha}" if img_sha else ctx["og_image"],
            "og_image_alt": title,
            "og_author": owner,
            "twitter_card": "summary_large_image" if img_sha else "summary",
        })
        ctx["ssr"] = ssr.modpack_project_view(pack)
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
    ordered by their rank on the in-game club leaderboard (board 1100). Server-side
    data comes from the API's ``/site/clubs`` (Mongo + Postgres)."""
    data = await internal_get("/site/clubs")
    clubs = data.get("items") if isinstance(data, dict) else None
    return _TEMPLATES.TemplateResponse(request, "clubs.html", {"clubs": clubs or []})


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
        # The OG PNG render stays on the API (data-plane); the page URL is our own.
        "og_image_url": f"{_API}/activity/og.png{qs}",
        "og_page_url": f"{_APP}/activity{qs}",
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
        "og_page_url": f"{_APP}/player/{name}",
        "ssr": await ssr.player_view(_ssr_fetch, name),
    })


@router.get("/leaderboards", response_class=HTMLResponse)
async def leaderboards(request: Request) -> HTMLResponse:
    """Trove leaderboards browser (reads via ``/site/leaderboards/*``).

    The two anti-cheat tabs are gated on the cheater/alt-cluster calculation
    switches and rendered (or not) server-side, so a disabled tab is gone on
    first paint. The switches come from the feature-flag map resolved on
    ``request.state`` by ``web_flags.resolve``."""
    return _TEMPLATES.TemplateResponse(request, "leaderboards.html", {
        "cheater_detection_enabled": getattr(request.state, "cheater_detection_enabled", True),
        "alt_clusters_enabled": getattr(request.state, "alt_clusters_enabled", True),
        "renames_enabled": getattr(request.state, "renames_enabled", True),
        "duplicates_enabled": getattr(request.state, "duplicates_enabled", True),
        "ssr": await ssr.leaderboards_view(_ssr_fetch),
    })


@router.get("/updates", response_class=HTMLResponse)
async def updates(request: Request) -> HTMLResponse:
    """Trove updates browser - public site read of the ``/v1/updates/*`` archive,
    via the ``/site/updates/*`` helpers (on the API)."""
    return _TEMPLATES.TemplateResponse(
        request, "updates.html", {"ssr": await ssr.updates_view(_ssr_fetch)})
