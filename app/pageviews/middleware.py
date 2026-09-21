import hashlib
import logging

from fastapi import FastAPI, Request

from app.core.config import settings
from app.core.middleware import _PAGE_PATHS, _PAGE_PREFIXES
from app.core.utils import client_ip, utcnow
from app.pageviews.models import PageView
from app.pageviews.recorder import recorder

logger = logging.getLogger("kiwi.pageviews")


def is_page_template(template: str) -> bool:
    """True for showcase-site PAGE routes. Reuses ``app.core.middleware``'s
    single-sourced page list so a new page is tracked automatically (same list that
    grants the relaxed site CSP)."""
    return template in _PAGE_PATHS or template.startswith(_PAGE_PREFIXES)


def visitor_hash(request: Request) -> str:
    """A cookieless, daily-rotating visitor id.

    ``sha256(secret | UTC-date | client_ip | user_agent)``. The date in the salt
    means the same visitor hashes differently each day (so they're counted once
    per day), and no raw IP / User-Agent is ever persisted - only this digest.
    Reuses ``client_ip`` (proxy-header aware) so it sees the real client behind the
    reverse proxy, not the edge IP.

    The website container has no ``SECRET_KEY`` (it issues no tokens), so the salt
    falls back to ``internal_key`` - shared by both containers, which keeps a
    visitor's hash identical whichever tier served the page.
    """
    ip = client_ip(request) or "unknown"
    ua = request.headers.get("user-agent", "")
    secret = settings.internal_key or settings.secret_key
    salt = f"{secret}|{utcnow():%Y-%m-%d}"
    return hashlib.sha256(f"{salt}|{ip}|{ua}".encode()).hexdigest()[:32]


def add_pageview_middleware(app: FastAPI) -> None:
    """Record one PageView per showcase-site page load (best-effort, never breaks the
    request; gated by ``settings.pageview_tracking_enabled``).

    The matched route template decides whether a request is a trackable page, but the
    CONCRETE URL is what's stored - so each individual mod / player page gets its own
    row.
    """

    @app.middleware("http")
    async def record_pageview(request: Request, call_next):
        response = await call_next(request)

        if not settings.pageview_tracking_enabled or request.method != "GET":
            return response
        if response.status_code != 200:
            return response
        if not response.headers.get("content-type", "").startswith("text/html"):
            return response

        # The matched route template (e.g. /player/{name}); the concrete path for
        # unmatched requests. Set on the scope by the router during call_next.
        route = request.scope.get("route")
        template = getattr(route, "path", request.url.path)
        if not is_page_template(template):
            return response

        try:
            recorder.record(PageView(
                route=template,
                path=request.url.path,
                visitor_hash=visitor_hash(request),
            ))
        except Exception:
            logger.exception("Failed to queue page-view event")

        return response
