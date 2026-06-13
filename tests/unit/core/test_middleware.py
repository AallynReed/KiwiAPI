from fastapi import FastAPI
from fastapi.testclient import TestClient
from starlette.responses import HTMLResponse


def test_every_html_page_route_gets_the_site_csp():
    """Every showcase-site HTML page must be in ``_PAGE_PATHS`` so it receives the
    relaxed ``_SITE_CSP``. The strict ``_API_CSP`` has ``style-src 'unsafe-inline'``
    (no external sheets), so a page that falls through to it renders UNSTYLED -
    its ``/static/*.min.css`` + cdnjs font-awesome links are blocked by CSP.

    This regressed for /terms + /privacy (and earlier /updates): a new page route
    was added without updating ``_PAGE_PATHS``. This test introspects the site
    router so a future page can't silently slip through again."""
    from app.core.middleware import _is_site_path
    from app.site.router import router as site_router

    html_pages = sorted(
        r.path
        for r in site_router.routes
        if getattr(r, "response_class", None) is HTMLResponse
        and "GET" in (getattr(r, "methods", set()) or set())
        and "{" not in r.path
    )
    assert html_pages, "expected to discover the site's HTML page routes"
    uncovered = [p for p in html_pages if not _is_site_path(p)]
    assert not uncovered, (
        f"these HTML pages fall through to the strict API CSP and will render "
        f"unstyled - add them to _PAGE_PATHS in app/core/middleware.py: {uncovered}"
    )


def test_legal_pages_serve_site_csp_allowing_external_stylesheets():
    """End-to-end: /terms + /privacy must emit a CSP whose style-src permits the
    site's own origin (so /static/*.css loads), not the locked-down API CSP."""
    from app.core.middleware import _API_CSP, _SITE_CSP, _is_site_path

    # The site CSP allows 'self' (+ cdnjs) stylesheets; the API CSP does not.
    assert "'self'" in _SITE_CSP.split("style-src", 1)[1].split(";", 1)[0]
    assert "'self'" not in _API_CSP.split("style-src", 1)[1].split(";", 1)[0]
    for p in ("/terms", "/privacy"):
        assert _is_site_path(p)


def test_security_headers_and_body_limit(monkeypatch):
    from app.core.config import settings

    monkeypatch.setattr(settings, "max_request_body_bytes", 10)
    from app.core.middleware import add_security_middleware

    app = FastAPI()
    add_security_middleware(app)

    @app.post("/x")
    async def x():
        return {"ok": True}

    client = TestClient(app)

    r = client.post("/x", json={})
    assert r.status_code == 200
    assert r.headers["x-content-type-options"] == "nosniff"
    assert r.headers["x-frame-options"] == "DENY"
    assert "content-security-policy" in r.headers
    assert "strict-transport-security" in r.headers

    # Oversized body (50 bytes > 10) is rejected.
    r = client.post("/x", content=b"x" * 50)
    assert r.status_code == 413
    assert r.json()["error"]["code"] == "bad_request"
