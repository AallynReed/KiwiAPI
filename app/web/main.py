"""Website container entrypoint (``app.web.main:app``) - the presentation plane.

Renders the showcase HTML pages + serves ``/static`` for ``trove.aallyn.net``.
Holds NO database connection and starts NO background workers: dynamic data is
fetched from the API over HTTP (``internal_get``) server-side, and by the browser
cross-origin against ``api.aallyn.net``. See ``app/web/__init__.py``.

Run it like the API but with this module:
    uvicorn app.web.main:app --host 0.0.0.0 --port 8000 --proxy-headers \
        --forwarded-allow-ips '*'
"""
import logging
from contextlib import asynccontextmanager
from pathlib import Path

from fastapi import FastAPI
from fastapi.staticfiles import StaticFiles

from app.core.config import settings
from app.core.errors import register_error_handlers
from app.core.middleware import (
    add_head_method_middleware,
    add_security_middleware,
    add_static_compression_middleware,
)
from app.core.observability import add_request_context_middleware, configure_logging
from app.web.meta import router as meta_router
from app.web.pages import router as pages_router

logger = logging.getLogger("kiwi.web")


@asynccontextmanager
async def lifespan(app: FastAPI):
    # Presentation only - no DB / Redis / Postgres, no workers. The one setup step
    # is logging; everything dynamic is fetched from the API at request time.
    configure_logging(logging.DEBUG if settings.debug else logging.INFO)
    logger.info("website container up (api=%s, internal=%s)",
                settings.api_url, settings.internal_api_url)
    yield


app = FastAPI(
    title=f"{settings.app_name} — Website",
    description="BetterTroveTools showcase site (presentation only).",
    version="1.0.0",
    debug=settings.debug,
    lifespan=lifespan,
    openapi_url=None,   # no API schema on the website host
)

register_error_handlers(app)   # themed HTML 404 for pages (branches on Accept + path)

# Middleware registered inner-first; the LAST one added is the OUTERMOST layer.
# Mirrors the API's page-relevant subset (request-context → security → head):
#   • request-context outermost: tags the request id + converts unhandled
#     exceptions into the standard envelope.
#   • security: applies the relaxed _SITE_CSP to pages/static + the body cap.
#   • head innermost: flips HEAD→GET for routing only.
# Omitted vs the API: CORS (pages are same-origin), pageview/usage/idempotency
# accounting (no DB here), and the api-host redirect (the website IS the
# canonical host).
# Compression sits outermost: the edge proxy only gzips text/html, so /static
# CSS, JS and locale JSON are compressed here or not at all.
add_head_method_middleware(app)
add_security_middleware(app)
add_request_context_middleware(app)
add_static_compression_middleware(app)

# Static assets (CSS/JS/fonts/screenshots), served straight from the bind-mounted
# ./site/static - same directory the API used to mount.
_SITE_ROOT = Path(settings.site_root)
if (_SITE_ROOT / "static").is_dir():
    app.mount("/static", StaticFiles(directory=str(_SITE_ROOT / "static")), name="site_static")

app.include_router(meta_router)    # robots.txt / sitemap.xml / BingSiteAuth.xml
app.include_router(pages_router)   # the HTML page routes


@app.get("/health", include_in_schema=False)
async def health() -> dict[str, str]:
    return {"status": "ok"}
