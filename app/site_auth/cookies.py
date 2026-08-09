"""Session cookies for site users - the storage half of Discord sign-in.

Why cookies at all: the tokens used to live in ``localStorage``, which any
script on the page can read. An HTML injection anywhere on the site could walk
off with a 30-day refresh token. These cookies are ``HttpOnly``, so script
cannot read them at all; the worst an injection can now do is make requests
while the page is open, and it can never exfiltrate the session itself.

Two things make that safe rather than just different:

1. **Scope.** The session is consumed from BOTH hosts - ``/v1/*`` on
   api.aallyn.net and the same-origin ``/site/*`` proxies on trove.aallyn.net -
   so the cookie needs ``Domain=.aallyn.net`` to reach both. That is wider than
   the old origin-scoped localStorage, and it is why ``cookie_auth_allowed``
   below pins the *caller* to an exact origin allowlist instead of merely
   trusting "same-site". Without that pin, an XSS on any sibling subdomain
   (docs., dev.) could ride a trove.aallyn.net session.

2. **CSRF.** ``SameSite=Lax`` already withholds the cookie from cross-site
   fetch/XHR, and trove->api is *same-site* (shared registrable domain
   ``aallyn.net``) so ordinary page calls still carry it. The origin check is
   the belt to that suspenders, and it is what lets us skip a CSRF-token layer
   entirely.

Non-browser clients (the BetterTroveTools desktop app, which signs in through
the same endpoints via a ``btt://`` deep link) are unaffected: every endpoint
still returns the tokens in the response body and still accepts
``Authorization: Bearer``. They simply ignore the ``Set-Cookie``.
"""
from urllib.parse import urlsplit

from fastapi import Request, Response

from app.core.config import settings
from app.site_auth.schemas import SiteTokenResponse

ACCESS_COOKIE = "kiwi_site_access"
REFRESH_COOKIE = "kiwi_site_refresh"
# JS-READABLE and deliberately valueless: it carries no secret, it only tells
# site_auth.js "you probably have a session, go call /me". Without it every
# anonymous pageview would spend a pointless request discovering it is
# anonymous, because HttpOnly means the client can no longer tell by looking.
SESSION_HINT_COOKIE = "kiwi_site_session"

# The refresh token is only ever presented to the rotate/logout endpoints, so
# it does not ride along on the hundreds of ordinary API calls a session makes.
_REFRESH_PATH = "/v1/site-auth"


def _cookie_domain() -> str | None:
    """``.aallyn.net`` in prod so both the API host and the website host get the
    cookie. ``None`` (host-only) for a bare-host or localhost deployment, where
    a Domain attribute would be rejected by the browser anyway."""
    host = urlsplit(settings.app_url).hostname or ""
    parts = host.split(".")
    if len(parts) < 3:
        return None
    return "." + ".".join(parts[-2:])


def _secure() -> bool:
    return urlsplit(settings.app_url).scheme == "https"


def allowed_cookie_origins() -> set[str]:
    """Origins whose requests may authenticate with the session COOKIE.

    Deliberately not "anything under aallyn.net": a sibling subdomain is
    same-site and would otherwise inherit the session. Bearer callers are
    unaffected by this - they bring their own credential.
    """
    origins = set()
    for url in (settings.app_url, settings.api_url):
        p = urlsplit(url)
        if p.scheme and p.hostname:
            origins.add(f"{p.scheme}://{p.netloc}")
    if settings.debug:
        origins.update({"http://localhost:8913", "http://127.0.0.1:8913"})
    return origins


def cookie_auth_allowed(request: Request) -> bool:
    """Whether this request may use the cookie as its credential.

    ``Origin`` is present on every cross-origin fetch and absent on same-origin
    GETs, so both cases are checked. A browser that sends neither header falls
    through to False and can still authenticate with a Bearer token.
    """
    origin = request.headers.get("origin")
    if origin:
        return origin in allowed_cookie_origins()
    return request.headers.get("sec-fetch-site") == "same-origin"


def set_session_cookies(response: Response, tokens: SiteTokenResponse) -> None:
    domain, secure = _cookie_domain(), _secure()
    max_age = settings.refresh_token_expire_days * 86400
    response.set_cookie(
        ACCESS_COOKIE, tokens.access_token,
        max_age=settings.access_token_expire_minutes * 60,
        httponly=True, secure=secure, samesite="lax", domain=domain, path="/",
    )
    response.set_cookie(
        REFRESH_COOKIE, tokens.refresh_token, max_age=max_age,
        httponly=True, secure=secure, samesite="lax", domain=domain,
        path=_REFRESH_PATH,
    )
    response.set_cookie(
        SESSION_HINT_COOKIE, "1", max_age=max_age,
        httponly=False, secure=secure, samesite="lax", domain=domain, path="/",
    )


def clear_session_cookies(response: Response) -> None:
    domain = _cookie_domain()
    response.delete_cookie(ACCESS_COOKIE, domain=domain, path="/")
    response.delete_cookie(REFRESH_COOKIE, domain=domain, path=_REFRESH_PATH)
    response.delete_cookie(SESSION_HINT_COOKIE, domain=domain, path="/")
