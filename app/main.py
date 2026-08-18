import asyncio
import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import Depends, FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse, JSONResponse

from app.admin.router import router as admin_router
from app.auth.account import router as account_router
from app.auth.oauth import router as oauth_router
from app.auth.router import router as auth_router
from app.auth.schemas import PublicConfig, ScopeInfo
from app.auth.sessions import router as sessions_router
from app.bot.router import router as discord_bot_router
from app.core.bootstrap import bootstrap_admin
from app.core.config import settings
from app.core.database import close_db, init_db
from app.core.email_outbox import start_email_worker, stop_email_worker
from app.core.errors import COMMON_ERROR_RESPONSES, register_error_handlers
from app.core.features import (
    require_class_activity_enabled,
    require_codexes_enabled,
    require_dm_subs_enabled,
    require_dressing_room_enabled,
    require_embed_enabled,
    require_giveaways_enabled,
    require_image_studio_enabled,
    require_leaderboards_enabled,
    require_market_enabled,
    require_mods_hub_enabled,
    require_player_activity_enabled,
    require_store_enabled,
    require_updates_enabled,
    require_webhooks_enabled,
)
from app.core.idempotency import add_idempotency_middleware
from app.core.maintenance import maintenance_loop
from app.core.middleware import (
    add_api_host_redirect_middleware,
    add_head_method_middleware,
    add_public_asset_cors_middleware,
    add_security_middleware,
)
from app.core.observability import add_request_context_middleware, configure_logging
from app.core.postgres import close_postgres, init_postgres
from app.core.redis import close_redis, init_redis
from app.core.scopes import catalog as scope_catalog
from app.discord.router import router as discord_router
from app.dm_subs.delivery import start_dm_delivery, stop_dm_delivery
from app.dm_subs.router import router as dm_subs_router
from app.embed.router import embed_api_router, embed_page_router
from app.events.bus import start_event_bus, stop_event_bus
from app.events.router import router as events_router
from app.events.scheduler import start_event_scheduler, stop_event_scheduler
from app.giveaways.admin import router as giveaways_admin_router
from app.giveaways.router import public_router as giveaways_public_router
from app.giveaways.router import router as giveaways_router
from app.giveaways.worker import start_giveaway_worker, stop_giveaway_worker
from app.images.router import router as images_router
from app.pageviews.middleware import add_pageview_middleware
from app.pageviews.recorder import recorder as pageview_recorder
from app.scanning.router import router as scanning_router
from app.site.router import router as site_router
from app.site_auth.account import router as site_account_router
from app.site_auth.oauth import router as site_oauth_router
from app.site_auth.router import router as site_auth_router
from app.supporters.router import public_router as supporters_public_router
from app.tokens.router import router as tokens_router
from app.tokens.schemas import REVOKE_REASONS
from app.trove.btt_releases import (
    start_btt_releases_refresher,
    stop_btt_releases_refresher,
)
from app.trove.chaos import start_chaos_refresher, stop_chaos_refresher
from app.trove.delves import start_delve_refresher, stop_delve_refresher
from app.trove.dressing.router import dressing_router
from app.trove.events import start_events_refresher, stop_events_refresher
from app.trove.feeds import start_feeds_refresher, stop_feeds_refresher
from app.trove.leaderboards.detection import (
    start_cheaters_warmer,
    stop_cheaters_warmer,
)
from app.trove.moderation import router as moderation_router
from app.trove.modpacks.router import (
    modpacks_hub_router,
    modpacks_hub_write_router,
    modpacks_public_router,
)
from app.trove.mods_hub.git_http import git_router as mods_git_router
from app.trove.mods_hub.router import (
    mods_creator_router,
    mods_creator_write_router,
    mods_hub_router,
    mods_hub_write_router,
    mods_public_router,
)
from app.trove.news import start_news_refresher, stop_news_refresher
from app.trove.router import (
    activity_router,
    btt_router,
    class_activity_router,
    codexes_router,
    feeds_router,
    gems_router,
    leaderboards_router,
    market_router,
    misc_router,
    mods_router,
    ocr_router,
    rotations_router,
    stats_router,
    store_router,
    updates_router,
)
from app.trove.status import start_status_prober, stop_status_prober
from app.trove.updates.worker import start_update_archiver, stop_update_archiver
from app.usage.middleware import add_usage_middleware
from app.usage.recorder import recorder as usage_recorder
from app.webhooks.delivery import start_webhook_delivery, stop_webhook_delivery
from app.webhooks.router import router as webhooks_router

logger = logging.getLogger("kiwi")

DEFAULT_SECRET = "CHANGE_ME_insecure_dev_secret_do_not_use_in_production"


@asynccontextmanager
async def lifespan(app: FastAPI):
    configure_logging(logging.DEBUG if settings.debug else logging.INFO)
    if settings.secret_key == DEFAULT_SECRET:
        logger.warning(
            "SECRET_KEY is the insecure default - set a strong SECRET_KEY in "
            "production (sessions are forgeable otherwise)."
        )
    if not settings.captcha_secret:
        logger.warning(
            "CAPTCHA_SECRET is unset - signup captcha is DISABLED (dev mode)."
        )
    await init_db()
    await init_redis()
    await init_postgres()  # leaderboards datastore (entries/boards/players/activity)
    await bootstrap_admin()
    # Seed the market interest-items collection from gamedata/market_items.json
    # if it's empty (first boot). After this admins manage the list via the
    # /admin/market/interest-items endpoints; the JSON file is only a seed +
    # offline fallback.
    from app.trove.market.service import seed_interest_items_if_empty
    await seed_interest_items_if_empty()
    from app.supporters.service import seed_supporters_if_empty
    await seed_supporters_if_empty()
    from app.trove.mods_hub.service import backfill_owner_handles
    await backfill_owner_handles()   # set owner_handle on mods predating per-owner slugs
    usage_recorder.start()
    pageview_recorder.start()  # buffered writer for showcase-site page-view analytics
    start_email_worker()
    start_news_refresher()
    start_feeds_refresher()
    start_events_refresher()
    start_chaos_refresher()
    start_status_prober()  # Trove server status (auth + optional game socket), every 60s
    start_delve_refresher()
    start_btt_releases_refresher()
    start_update_archiver()  # off unless trove_update_enabled
    # Cheater-detection cache warmer: runs detection at boot + every
    # cheaters_cache_ttl_seconds so the /v1/leaderboards/cheaters
    # endpoint always serves a fresh cached result instantly.
    start_cheaters_warmer()
    start_giveaway_worker()  # auto-opens scheduled giveaways + draws ended ones (60s)
    start_event_bus()  # live SSE event stream: per-worker Redis fan-out + safety-net watcher
    start_event_scheduler()  # time-driven rotation events -> Redis (SSE + bot react)
    start_webhook_delivery()  # outbound Discord webhooks: per-worker delivery queue consumer
    start_dm_delivery()  # inbound Discord DM alert subscriptions: per-worker DM queue consumer
    maintenance_task = asyncio.create_task(maintenance_loop())
    yield
    maintenance_task.cancel()
    await stop_dm_delivery()
    await stop_webhook_delivery()
    await stop_event_scheduler()
    await stop_event_bus()
    await stop_giveaway_worker()
    await stop_cheaters_warmer()
    await stop_update_archiver()
    await stop_btt_releases_refresher()
    await stop_delve_refresher()
    await stop_chaos_refresher()
    await stop_status_prober()
    await stop_events_refresher()
    await stop_feeds_refresher()
    await stop_news_refresher()
    await stop_email_worker()
    await usage_recorder.stop()
    await pageview_recorder.stop()
    await close_redis()
    await close_postgres()
    await close_db()


API_DESCRIPTION = """\
Sign up, mint API tokens, host data, and run calculations.

**Errors.** Every error responds with the same envelope:

```json
{ "error": { "code": "rate_limited", "message": "…", "details": null } }
```

`code` is a stable, machine-readable slug (e.g. `not_authenticated`,
`insufficient_scope`, `ip_not_allowed`, `rate_limited`, `validation_error`) -
branch on it rather than the human-readable `message`.

**Rate limits.** Responses carry `X-RateLimit-Limit`, `X-RateLimit-Remaining`
and `X-RateLimit-Reset`; a `429` also includes `Retry-After`.

**Authorization.** Most endpoints need an API token
(`Authorization: Bearer kiwi_…`). Endpoints marked 🔓 are **tokenless** - they work
with no token at all, but anonymous callers share a tighter per-IP rate limit;
send a token carrying the endpoint's scope to earn the higher per-token limit (and
usage accounting). Tokenless endpoints show their auth as *optional* under
**Authorizations**. Endpoints marked ⭐ are **master-only** - they require a
superuser-owned token.
"""


# Reference sidebar. Every section is a tag; listing them fixes the ORDER (Redoc
# follows this array, then appends anything unlisted). Only "creators" carries a
# description - it's the one section that isn't self-explanatory, because it needs
# a connection before any of it works. The rest are ordered only.
API_TAGS: list[dict] = [
    {"name": t} for t in (
        "giveaways", "misc", "rotations", "feeds", "stats", "gems", "mods",
        "modpacks",
    )
] + [{
    "name": "creators",
    "description": (
        "**Control endpoints** - manage a creator's Mods Hub mods from your own app. "
        "Everything else in this reference reads public data; this section writes.\n\n"
        "Mods belong to **Dashboard** accounts (Discord sign-in on trove.aallyn.net), "
        "not to API accounts, so a creator has to invite you before you can touch "
        "theirs. The invite is a **creator token** (`kiwi_creator_…`): one per "
        "Dashboard account, generated under **Mods → API access**, shown once.\n\n"
        "1. The creator gives you their creator token.\n"
        "2. You paste it once on **dev.aallyn.net → Creators** (`POST "
        "/v1/mods/hub/creator-links`, authenticated with your portal session). That "
        "creates a **connection**; the token is a connect code and isn't used again.\n"
        "3. Every call below then uses an ordinary API token carrying `mods:write`.\n\n"
        "A connection covers **all of that creator's mods, including ones they create "
        "later**, until they narrow it to named mods from their Dashboard. They can "
        "revoke one connection, or rotate their creator token to cut every connection "
        "at once. You can hold connections to many creators, and a creator can connect "
        "many API accounts.\n\n"
        "The mod in the URL decides which creator you act as. On the two routes that "
        "name no mod (create a mod, list mods) add `?creator=<handle>` or the "
        "`X-Kiwi-Creator` header - optional when you're connected to exactly one "
        "creator.\n\n"
        "The public reads elsewhere in this reference answer as an anonymous visitor, "
        "so they can't show a draft mod or an unpublished release. The `me/projects` "
        "routes below are the creator's own view of the same data.\n\n"
        "**Not available here, by design:** deleting a **mod** (that takes its files, "
        "history and every release with it - deleting a single release is fine), "
        "minting git tokens, editing the creator's profile, and changing "
        "collaborators. Those stay with the creator on the website.\n\n"
        "`403 insufficient_scope` = your token lacks `mods:write`. `403 forbidden` = no "
        "connection of yours covers that mod, or the action is website-only."
    ),
}] + [
    {"name": t} for t in (
        "embed", "updates", "codexes", "btt", "leaderboards", "market", "store",
        "activity", "class-activity", "events", "ocr", "meta",
    )
]


app = FastAPI(
    title=settings.app_name,
    description=API_DESCRIPTION,
    version="1.0.0",
    debug=settings.debug,
    lifespan=lifespan,
    responses=COMMON_ERROR_RESPONSES,
    openapi_tags=API_TAGS,
    # Declare the canonical API host so the reference page (Redoc) shows request
    # URLs as api.aallyn.net. Without this, the spec has no `servers` and Redoc
    # falls back to the page's own origin - docs.aallyn.net, since the docs site
    # proxies /openapi.json same-origin.
    servers=[{"url": settings.api_url, "description": "Production"}],
)

register_error_handlers(app)
# Middleware is registered inner-first; the LAST one added is the OUTERMOST layer.
# Resulting execution order (outer → inner):
#   CORS → request-context → api-host-redirect → security → idempotency → usage
#     → pageview → head → route
# Rationale:
#   • CORS outermost, so even error responses get access-control headers.
#   • request-context just inside CORS: tags the request id and converts unhandled
#     exceptions into the standard envelope (which then flows out through CORS).
#   • security (headers + body cap) outside idempotency, so even a replayed
#     response still carries the security headers.
#   • idempotency outside usage, so a replay doesn't double-count a usage event.
#   • pageview: GET page loads aren't idempotency-keyed, so no replay double-count;
#     it reads the matched route + final response to log site views.
#   • head innermost: flips HEAD→GET for routing only, so the analytics layers
#     above still see the real HEAD and skip their GET-only accounting.
#   • api-host-redirect just inside request-context: 301s showcase pages that
#     leaked onto the api host to app_url, short-circuiting before the heavier
#     inner layers (a bare redirect needs none of them).
add_head_method_middleware(app)
add_pageview_middleware(app)
add_usage_middleware(app)
add_idempotency_middleware(app)
add_security_middleware(app)
add_api_host_redirect_middleware(app)
add_request_context_middleware(app)

# CORS is the outermost layer so even error responses get access-control headers.
app.add_middleware(
    CORSMiddleware,
    allow_origins=settings.cors_origins,
    allow_origin_regex=settings.cors_origin_regex,
    allow_credentials=True,
    allow_methods=["*"],
    allow_headers=["*"],
    expose_headers=["X-Request-ID", "X-RateLimit-Limit", "X-RateLimit-Remaining",
                    "X-RateLimit-Reset", "Retry-After", "X-Dressing-Dropped"],
)

# Outside that, and therefore the last word: the 3D viewers' public assets answer any
# origin without credentials, so a partner site can point the viewer at the API
# instead of proxying it. Deliberately NOT done by adding partners to the allowlist
# above - that one carries credentials.
add_public_asset_cors_middleware(app)

# One router per endpoint-path group (feature module).
#
# Account, login, session, and token-management routes are driven by HUMANS in
# the browser portal - programs never call them. So they're hidden from the
# public OpenAPI reference (`include_in_schema=False`); they still function
# normally, they just don't appear in the docs that third-party developers read.
app.include_router(auth_router, include_in_schema=False)
app.include_router(sessions_router, include_in_schema=False)
app.include_router(account_router, include_in_schema=False)
app.include_router(oauth_router, include_in_schema=False)
app.include_router(discord_router, include_in_schema=False)
# Public-facing user system (trove.aallyn.net signups, dashboard). Lives
# in a separate Beanie collection from the dev portal's `User` - see
# app/site_auth/__init__.py for the rationale.
app.include_router(site_auth_router, include_in_schema=False)
app.include_router(site_account_router, include_in_schema=False)  # GDPR export + self-delete (site_auth)
app.include_router(moderation_router, include_in_schema=False)  # public notice-and-action reports (DSA)
app.include_router(site_oauth_router, include_in_schema=False)
app.include_router(tokens_router, include_in_schema=False)
app.include_router(admin_router, include_in_schema=False)
app.include_router(giveaways_admin_router, include_in_schema=False)
# Public + user-facing giveaways ride the master feature toggle (the admin
# management router above stays reachable so draws can be administered while
# the public surface is hidden).
_GIVEAWAYS_GATE = [Depends(require_giveaways_enabled)]
app.include_router(giveaways_router, include_in_schema=False, dependencies=_GIVEAWAYS_GATE)
app.include_router(giveaways_public_router, dependencies=_GIVEAWAYS_GATE)   # public giveaways:read - in schema
app.include_router(supporters_public_router)  # public misc:read (tokenless) - in schema
app.include_router(discord_bot_router, include_in_schema=False)  # User Dashboard "Discord Bot" tab (site_auth)
app.include_router(  # User Dashboard "DM Alerts" tab (site_auth); inbound Discord DM subscriptions
    dm_subs_router, include_in_schema=False,
    dependencies=[Depends(require_dm_subs_enabled)],
)
app.include_router(  # User Dashboard "Webhooks" tab (site_auth); outbound Discord webhooks
    webhooks_router, include_in_schema=False,
    dependencies=[Depends(require_webhooks_enabled)],
)
app.include_router(  # User Dashboard "Image Studio" (site_auth) + public PNG render URL
    images_router, include_in_schema=False,
    dependencies=[Depends(require_image_studio_enabled)],
)
app.include_router(scanning_router, include_in_schema=False)
# Data surface - organized by function (token-authenticated, in the public reference).
app.include_router(rotations_router)
app.include_router(feeds_router)
app.include_router(stats_router)
app.include_router(gems_router)
app.include_router(misc_router)
app.include_router(mods_router)
# Mods Hub. Three surfaces, and only two of them are public API:
#   - `mods_hub_router` / `mods_hub_write_router` - the website's own hub (browse +
#     the writes only the creator's Dashboard session can make). Hidden from the
#     reference: they're driven by the website studio, not by API developers.
#   - `mods_public_router` (/v1/mods/*) - the documented app-facing READ catalog.
#   - `mods_creator_write_router` + `mods_creator_router` - the documented CONTROL
#     surface ("creators" tag): what an API account can do for a creator who
#     connected to it (app/trove/mods_hub/write_auth.py), plus managing those
#     connections. Reads stay tokenless `mods:read`; control needs `mods:write`.
# All gated by feature_mods_hub_enabled (OFF -> every endpoint 404s; see
# app/core/features.py).
_MODS_GATE = [Depends(require_mods_hub_enabled)]
app.include_router(mods_hub_router, include_in_schema=False, dependencies=_MODS_GATE)
app.include_router(mods_hub_write_router, include_in_schema=False, dependencies=_MODS_GATE)
app.include_router(mods_creator_write_router, dependencies=_MODS_GATE)  # control surface
app.include_router(mods_creator_router, dependencies=_MODS_GATE)        # its connections
app.include_router(mods_public_router, dependencies=_MODS_GATE)  # documented app-facing API (/v1/mods/*)
app.include_router(mods_git_router, dependencies=_MODS_GATE)   # authenticated git smart-HTTP (/git/mods/*.git)
# Modpacks (user-curated bundles of hub mods) ride the same master toggle - they're
# a layer over the hub and meaningless without it. The website-internal hub surface
# is hidden; the app-facing catalog API (/v1/modpacks/*) is documented like /v1/mods/*.
app.include_router(modpacks_hub_router, include_in_schema=False, dependencies=_MODS_GATE)
app.include_router(modpacks_hub_write_router, include_in_schema=False, dependencies=_MODS_GATE)
app.include_router(modpacks_public_router, dependencies=_MODS_GATE)  # documented app-facing API (/v1/modpacks/*)
# Embeddable viewers. Partner sites iframe /embed/viewer to preview a blueprint
# model, an assembled creature or a .pkfx effect - from a hub release, a .tmod they
# uploaded, or a path in the game files. The page + its same-origin data endpoints
# are browser-internal (hidden from the reference); the partner-facing upload API
# (/v1/embed/tmod) is documented. Who may FRAME it is embed.allowed_origins, applied
# as CSP frame-ancestors in app/core/middleware.py.
_EMBED_GATE = [Depends(require_embed_enabled)]
app.include_router(embed_page_router, dependencies=_EMBED_GATE)
app.include_router(embed_api_router, dependencies=_EMBED_GATE)
# Each of these rides its own master feature toggle (the "features" category in
# the admin Configuration tab); OFF -> every endpoint 404s, matching how the
# website page + navbar link disappear. See app/core/features.py.
app.include_router(updates_router, dependencies=[Depends(require_updates_enabled)])
app.include_router(codexes_router, dependencies=[Depends(require_codexes_enabled)])
app.include_router(  # compose a character from the game's costumes + styles (/v1/dressing/*)
    dressing_router, dependencies=[Depends(require_dressing_room_enabled)])
app.include_router(btt_router)
app.include_router(leaderboards_router, dependencies=[Depends(require_leaderboards_enabled)])
app.include_router(market_router, dependencies=[Depends(require_market_enabled)])
app.include_router(store_router, dependencies=[Depends(require_store_enabled)])  # in-game Kiwi Store catalog (store:read)
app.include_router(activity_router, dependencies=[Depends(require_player_activity_enabled)])
app.include_router(class_activity_router, dependencies=[Depends(require_class_activity_enabled)])
app.include_router(events_router)  # live SSE event stream (events:read)
app.include_router(ocr_router)     # self-hosted character-stat OCR (ocr:read)

# BetterTroveTools showcase site - this container serves its DATA plane only: the
# same-origin "/site/*" JSON + binary proxies, the OG PNG renders, the status badge,
# robots.txt and sitemap.xml. The HTML pages and the "/static" mount belong to the
# website container (app/web/main.py); a page path arriving on an api-side host is
# 301'd there instead (see add_api_host_redirect_middleware).
#
# ./site stays bind-mounted (19 MB of screenshots don't bake into the image) because
# the router still READS it off disk - the screenshots proxy walks
# ./site/static/trove-screens. It just isn't served over HTTP from here any more,
# which is why the include is gated on that directory rather than ./site/templates.
_SITE_ROOT = Path(settings.site_root)
if (_SITE_ROOT / "static").is_dir():
    app.include_router(site_router)


# ── OpenAPI: flag tokenless (🔓) + master-only (⭐) endpoints in the reference ──
# In the raw spec, a ``public_scope`` (tokenless) endpoint references the APIToken
# scheme exactly like a token-required one, and a master-only endpoint looks like
# any other token endpoint - so the Redoc reference can't tell them apart. This
# post-processes the generated schema to mark them in the summary (shows in the
# sidebar + header) + a description note; tokenless ops also get optional auth so
# "Authorizations" renders the token as not-required.
_TOKENLESS_NOTE = (
    "\n\n> 🔓 **No token required.** This endpoint works without an API token - "
    "anonymous callers are allowed but share a tighter per-IP rate limit; send a "
    "token carrying this scope for the higher per-token limit (and usage accounting)."
)
_TOKENLESS_MARKER = "🔓 **No token required"   # distinct sentinel (prose already says "Tokenless")
_MASTER_NOTE = (
    "\n\n> ⭐ **Master only.** Requires a superuser-owned API token (or a master "
    "session in the dev portal); a normal token gets 403."
)
_MASTER_MARKER = "⭐ **Master only"

_HTTP_METHODS = ("get", "post", "put", "patch", "delete", "options", "head", "trace")


def _ops_with_marker(attr: str) -> set[tuple[str, str]]:
    """``{(path, http_method_lower)}`` for every in-schema operation whose dependant
    tree contains a callable tagged with ``attr`` (e.g. ``_tokenless_scope`` set by
    ``public_scope`` / ``_master_only`` set on ``require_master_ingest``)."""
    from fastapi.routing import APIRoute

    def _walk(dep):
        yield dep
        for sub in dep.dependencies:
            yield from _walk(sub)

    out: set[tuple[str, str]] = set()
    for route in app.routes:
        if not isinstance(route, APIRoute) or not route.include_in_schema:
            continue
        if any(getattr(d.call, attr, None) is not None for d in _walk(route.dependant)):
            for method in (route.methods or set()):
                out.add((route.path_format, method.lower()))
    return out


def custom_openapi() -> dict:
    if app.openapi_schema:
        return app.openapi_schema
    from fastapi.openapi.utils import get_openapi

    schema = get_openapi(
        title=app.title,
        version=app.version,
        openapi_version=app.openapi_version,
        description=app.description,
        routes=app.routes,
        tags=app.openapi_tags,
        servers=app.servers,
    )
    tokenless = _ops_with_marker("_tokenless_scope")
    master = _ops_with_marker("_master_only")
    for path, methods in schema.get("paths", {}).items():
        for method, op in methods.items():
            if method not in _HTTP_METHODS or not isinstance(op, dict):
                continue
            # Tokenless: public_scope (marker) OR an endpoint flagged x-tokenless
            # (fully-open routes with no scope dependency, e.g. /v1/misc/feedback).
            if (path, method) in tokenless or op.get("x-tokenless") is True:
                summary = op.get("summary") or ""
                if not summary.startswith("🔓"):
                    op["summary"] = f"🔓 {summary}".rstrip()
                if _TOKENLESS_MARKER not in (op.get("description") or ""):
                    op["description"] = (op.get("description") or "") + _TOKENLESS_NOTE
                # Prepend the empty requirement so the token reads as OPTIONAL
                # (FastAPI lists the scheme even when auto_error=False).
                sec = op.get("security")
                if isinstance(sec, list) and {} not in sec:
                    op["security"] = [{}, *sec]
            # Master-only: superuser token required.
            elif (path, method) in master:
                summary = op.get("summary") or ""
                if not summary.startswith("⭐"):
                    op["summary"] = f"⭐ {summary}".rstrip()
                if _MASTER_MARKER not in (op.get("description") or ""):
                    op["description"] = (op.get("description") or "") + _MASTER_NOTE
    # Explain the 🔓 / ⭐ conventions on the APIToken scheme itself (Redoc auth UI).
    scheme = schema.get("components", {}).get("securitySchemes", {}).get("APIToken")
    if scheme and "🔓" not in (scheme.get("description") or ""):
        scheme["description"] = ((scheme.get("description") or "").rstrip()
                                 + " Endpoints marked 🔓 are tokenless (callable "
                                   "without this token at a tighter per-IP rate limit); "
                                   "endpoints marked ⭐ require a superuser token.").strip()
    app.openapi_schema = schema
    return schema


app.openapi = custom_openapi


def _feature_gated_ops() -> dict[tuple[str, str], str]:
    """``{(path, http_method_lower): flag}`` for every in-schema operation whose
    dependant tree contains a feature-gate dependency (tagged ``_feature_flag`` by
    ``app.core.features._gate``). Lets the ``/openapi.json`` route drop a disabled
    feature's operations from the reference the same moment its master toggle flips
    OFF - so a hidden feature 404s AND vanishes from the docs, no restart."""
    from fastapi.routing import APIRoute

    def _walk(dep):
        yield dep
        for sub in dep.dependencies:
            yield from _walk(sub)

    out: dict[tuple[str, str], str] = {}
    for route in app.routes:
        if not isinstance(route, APIRoute) or not route.include_in_schema:
            continue
        flag = next(
            (getattr(d.call, "_feature_flag", None) for d in _walk(route.dependant)
             if getattr(d.call, "_feature_flag", None) is not None),
            None,
        )
        if flag is None:
            continue
        for method in (route.methods or set()):
            out[(route.path_format, method.lower())] = flag
    return out


# Serve /openapi.json ourselves (async) so feature-gated operations whose master
# toggle is currently OFF disappear from the Redoc reference. FastAPI's built-in
# openapi route is sync and can't read the async runtime-config flags, so we drop
# it and register our own that prunes the disabled ops from a copy of the spec.
_OPENAPI_PATH = app.openapi_url or "/openapi.json"
app.router.routes = [
    r for r in app.router.routes if getattr(r, "path", None) != _OPENAPI_PATH
]


@app.get(_OPENAPI_PATH, include_in_schema=False)
async def openapi_json() -> JSONResponse:
    from copy import deepcopy

    from app.core import features

    base = custom_openapi()
    gated = _feature_gated_ops()
    disabled = {
        flag for flag in set(gated.values()) if not await features.is_enabled(flag)
    }
    if not disabled:
        return JSONResponse(base)
    schema = deepcopy(base)
    paths = schema.get("paths", {})
    for (path, method), flag in gated.items():
        if flag in disabled and path in paths:
            paths[path].pop(method, None)
            if not paths[path]:  # last operation on this path removed → drop it
                paths.pop(path, None)
    return JSONResponse(schema)


# Fallback API-card landing for `/api-info`, served at api.aallyn.net for
# developers (the site router owns the real "/"). HTMLResponse kept inline so
# the file is self-contained.
@app.get("/api-info", response_class=HTMLResponse, include_in_schema=False)
async def api_info() -> HTMLResponse:
    return HTMLResponse(
        f"""<!doctype html><html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1.0">
<title>{settings.app_name}</title>
<style>
  body {{ margin:0; min-height:100vh; display:flex; align-items:center; justify-content:center;
    font-family:-apple-system,BlinkMacSystemFont,"Segoe UI",Roboto,sans-serif;
    background:#0d1117; color:#e6edf3; }}
  .card {{ text-align:center; padding:40px; max-width:440px; }}
  h1 {{ font-size:2rem; margin:0 0 10px; }}
  h1 .mark {{ color:#58a6ff; }}
  p {{ color:#9aa4b2; margin:0 0 24px; }}
  a {{ color:#58a6ff; text-decoration:none; margin:0 10px; font-weight:600; }}
  a:hover {{ text-decoration:underline; }}
</style></head><body>
  <div class="card">
    <h1><span class="mark">◆</span> {settings.app_name}</h1>
    <p>Programmatic API access. Authenticate requests with an API token.</p>
    <a href="{settings.docs_url}">Documentation</a>·<a href="{settings.dev_url}">Developer Portal</a>
  </div>
</body></html>"""
    )


@app.get("/config", response_model=PublicConfig, tags=["meta"])
async def public_config() -> PublicConfig:
    """Non-sensitive settings the developer portal needs to render itself."""
    return PublicConfig(
        app_name=settings.app_name,
        api_url=settings.api_url,
        captcha_provider=settings.captcha_provider,
        captcha_sitekey=settings.captcha_sitekey,
        require_verified_for_tokens=settings.require_verified_for_tokens,
        scopes=[ScopeInfo(**s) for s in scope_catalog()],
        token_creation_daily_limit=settings.token_creation_daily_limit,
        revoke_reasons=REVOKE_REASONS,
        github_oauth_enabled=settings.github_oauth_enabled,
        discord_oauth_enabled=settings.discord_oauth_enabled,
    )


@app.get("/health", tags=["meta"])
async def health() -> dict[str, str]:
    return {"status": "ok"}
