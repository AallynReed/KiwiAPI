import asyncio
import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.middleware.cors import CORSMiddleware
from fastapi.responses import HTMLResponse
from fastapi.staticfiles import StaticFiles

from app.admin.router import router as admin_router
from app.auth.account import router as account_router
from app.auth.oauth import router as oauth_router
from app.auth.router import router as auth_router
from app.auth.schemas import PublicConfig, ScopeInfo
from app.auth.sessions import router as sessions_router
from app.site_auth.router import router as site_auth_router
from app.core.bootstrap import bootstrap_admin
from app.core.config import settings
from app.core.database import close_db, init_db
from app.core.email_outbox import start_email_worker, stop_email_worker
from app.core.errors import COMMON_ERROR_RESPONSES, register_error_handlers
from app.core.idempotency import add_idempotency_middleware
from app.core.maintenance import maintenance_loop
from app.core.middleware import add_security_middleware
from app.core.observability import add_request_context_middleware, configure_logging
from app.core.redis import close_redis, init_redis
from app.core.scopes import catalog as scope_catalog
from app.scanning.router import router as scanning_router
from app.site.router import router as site_router
from app.tokens.router import router as tokens_router
from app.tokens.schemas import REVOKE_REASONS
from app.trove.btt_releases import (
    start_btt_releases_refresher,
    stop_btt_releases_refresher,
)
from app.trove.chaos import start_chaos_refresher, stop_chaos_refresher
from app.trove.status import start_status_prober, stop_status_prober
from app.trove.delves import start_delve_refresher, stop_delve_refresher
from app.trove.events import start_events_refresher, stop_events_refresher
from app.trove.leaderboards.detection import (
    start_cheaters_warmer,
    stop_cheaters_warmer,
)
from app.trove.news import start_news_refresher, stop_news_refresher
from app.trove.relays import start_feeds_refresher, stop_feeds_refresher
from app.trove.router import (
    btt_router,
    codexes_router,
    feeds_router,
    gems_router,
    leaderboards_router,
    market_router,
    misc_router,
    mods_router,
    rotations_router,
    stats_router,
    updates_router,
)
from app.trove.updates.worker import start_update_archiver, stop_update_archiver
from app.usage.middleware import add_usage_middleware
from app.usage.recorder import start_usage_recorder, stop_usage_recorder

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
    await bootstrap_admin()
    # Seed the market interest-items collection from gamedata/market_items.json
    # if it's empty (first boot). After this admins manage the list via the
    # /admin/market/interest-items endpoints; the JSON file is only a seed +
    # offline fallback.
    from app.trove.market.service import seed_interest_items_if_empty
    await seed_interest_items_if_empty()
    start_usage_recorder()
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
    maintenance_task = asyncio.create_task(maintenance_loop())
    yield
    maintenance_task.cancel()
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
    await stop_usage_recorder()
    await close_redis()
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
"""


app = FastAPI(
    title=settings.app_name,
    description=API_DESCRIPTION,
    version="1.0.0",
    debug=settings.debug,
    lifespan=lifespan,
    responses=COMMON_ERROR_RESPONSES,
    # Declare the canonical API host so the reference page (Redoc) shows request
    # URLs as api.aallyn.net. Without this, the spec has no `servers` and Redoc
    # falls back to the page's own origin - docs.aallyn.net, since the docs site
    # proxies /openapi.json same-origin.
    servers=[{"url": settings.api_url, "description": "Production"}],
)

register_error_handlers(app)
# Middleware is registered inner-first; the LAST one added is the OUTERMOST layer.
# Resulting execution order (outer → inner):
#   CORS → request-context → security → idempotency → usage → route
# Rationale:
#   • CORS outermost, so even error responses get access-control headers.
#   • request-context just inside CORS: tags the request id and converts unhandled
#     exceptions into the standard envelope (which then flows out through CORS).
#   • security (headers + body cap) outside idempotency, so even a replayed
#     response still carries the security headers.
#   • idempotency outside usage, so a replay doesn't double-count a usage event.
add_usage_middleware(app)
add_idempotency_middleware(app)
add_security_middleware(app)
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
                    "X-RateLimit-Reset", "Retry-After"],
)

# One router per endpoint-path group (feature module).
#
# Account, login, session, and token-management routes are driven by HUMANS in
# the browser portal - programs never call them. So they're hidden from the
# public OpenAPI reference (`include_in_schema=False`); they still function
# normally, they just don't appear in the docs that third-party developers read.
#
# This 1.0 base ships the full developer platform but NO data endpoints yet - the
# token-authenticated `/v1/*` product surface is added on top of this foundation.
app.include_router(auth_router, include_in_schema=False)
app.include_router(sessions_router, include_in_schema=False)
app.include_router(account_router, include_in_schema=False)
app.include_router(oauth_router, include_in_schema=False)
# Public-facing user system (trove.aallyn.net signups, dashboard). Lives
# in a separate Beanie collection from the dev portal's `User` - see
# app/site_auth/__init__.py for the rationale.
app.include_router(site_auth_router, include_in_schema=False)
app.include_router(tokens_router, include_in_schema=False)
app.include_router(admin_router, include_in_schema=False)
app.include_router(scanning_router, include_in_schema=False)
# Data surface - organized by function (token-authenticated, in the public reference).
app.include_router(rotations_router)
app.include_router(feeds_router)
app.include_router(stats_router)
app.include_router(gems_router)
app.include_router(misc_router)
app.include_router(mods_router)
app.include_router(updates_router)
app.include_router(codexes_router)
app.include_router(btt_router)
app.include_router(leaderboards_router)
app.include_router(market_router)

# BetterTroveTools showcase site (trove.aallyn.net). The site router owns
# "/", "/documentation", "/commands", "/leaderboards", "/updates",
# "/support" plus the same-origin "/site/*" JSON proxies. The proxy can
# put this container on both api.aallyn.net (filtering to /v1 + /health)
# and trove.aallyn.net (everything else). Templates + assets live in
# ./site (bind-mounted, so 19 MB of screenshots don't bake into the image).
_SITE_ROOT = Path(settings.site_root)
if (_SITE_ROOT / "static").is_dir():
    app.mount("/static", StaticFiles(directory=str(_SITE_ROOT / "static")), name="site_static")
if (_SITE_ROOT / "templates").is_dir():
    app.include_router(site_router)


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
    )


@app.get("/health", tags=["meta"])
async def health() -> dict[str, str]:
    return {"status": "ok"}
