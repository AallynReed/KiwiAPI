"""Discord "Sign in with Discord" for SITE users (the trove.aallyn.net dashboard).

Separate from the dev-portal OAuth (``app/auth/oauth.py``): this creates/links a
``SiteUser`` and issues SITE tokens (``kind=site``), then hands them to the
browser via a one-time exchange code in the URL fragment so the showcase site's
``site_auth.js`` can finish the login (``POST /v1/site-auth/oauth/exchange``).

The scope is ``identify email guilds`` (guilds so the Dashboard's "Discord Bot"
tab can list the user's servers) - it never user-installs the app (that's the
separate "Add to Discord" home-page button). The email must be VERIFIED on the
Discord side before we sign in / link.
"""
import json
import logging
import re
import secrets
from urllib.parse import quote, urlencode, urlsplit, urlunsplit

import httpx
from fastapi import APIRouter, Request, Response
from fastapi.responses import HTMLResponse, RedirectResponse
from pydantic import BaseModel

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.redis import get_redis
from app.core.utils import utcnow
from app.site_auth.cookies import set_session_cookies
from app.site_auth.models import SiteUser
from app.site_auth.schemas import SiteTokenResponse
from app.site_auth.sessions import issue_tokens

logger = logging.getLogger("kiwi.site_auth.oauth")

router = APIRouter(prefix="/v1/site-auth/oauth", tags=["site-auth"])

_AUTHORIZE = "https://discord.com/oauth2/authorize"
_TOKEN = "https://discord.com/api/oauth2/token"
_API = "https://discord.com/api"

# Short-lived Discord access token, cached in Redis so the Dashboard "Discord Bot"
# tab can fetch the user's server list ON DEMAND (see fetch_discord_guilds) instead
# of us persisting that list. GDPR: the server membership is never stored at rest;
# the token self-expires with Discord's own lifetime (capped at 7 days).
_DISCORD_TOK_PREFIX = "site_discord_tok:"
_DISCORD_TOK_MAX_TTL = 7 * 24 * 3600


async def store_discord_token(user_id, access_token: str, expires_in) -> None:
    """Cache the user's Discord access token so guilds can be fetched on demand."""
    r = get_redis()
    if r is None or not access_token:
        return
    try:
        ttl = int(expires_in)
    except (TypeError, ValueError):
        ttl = 0
    ttl = max(60, min(ttl or _DISCORD_TOK_MAX_TTL, _DISCORD_TOK_MAX_TTL))
    await r.set(f"{_DISCORD_TOK_PREFIX}{user_id}", access_token, ex=ttl)


async def get_discord_token(user_id) -> str | None:
    r = get_redis()
    if r is None:
        return None
    return await r.get(f"{_DISCORD_TOK_PREFIX}{user_id}")


async def clear_discord_token(user_id) -> None:
    r = get_redis()
    if r is None:
        return
    await r.delete(f"{_DISCORD_TOK_PREFIX}{user_id}")


async def fetch_discord_guilds(access_token: str) -> list[dict] | None:
    """Live-fetch the user's Discord servers with their cached token. Returns the
    normalized list (possibly empty), or ``None`` if the token is missing, expired,
    revoked, or Discord is unreachable - the caller then reprompts a reconnect. The
    result is used transiently and never written to our database."""
    if not access_token:
        return None
    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            gr = await client.get(
                f"{_API}/users/@me/guilds",
                headers={"Authorization": f"Bearer {access_token}"},
            )
    except httpx.HTTPError:
        return None
    if gr.status_code != 200:
        return None
    try:
        raw = gr.json()
    except ValueError:
        return None
    if not isinstance(raw, list):
        return None
    return [
        {"id": str(g["id"]), "name": g.get("name", ""), "icon": g.get("icon"),
         "owner": bool(g.get("owner")), "permissions": str(g.get("permissions", "0"))}
        for g in raw if isinstance(g, dict) and g.get("id")
    ]


class _ExchangeBody(BaseModel):
    code: str


def _require_enabled() -> None:
    if not settings.discord_oauth_enabled:
        raise APIError(404, ErrorCode.not_found, "Discord sign-in is not configured")


def _redirect_uri() -> str:
    return f"{settings.api_url}/v1/site-auth/oauth/discord/callback"


def _safe_next(raw: str | None) -> str:
    """The post-sign-in destination, or "" if it isn't one we may bounce to.

    Only a site-relative path survives, and it is rebuilt from its parsed parts
    rather than passed through: anything naming a scheme or a host is dropped,
    so a crafted ?next= can never turn sign-in into an open redirect. Backslashes
    are folded to "/" first because browsers treat "/\\evil.com" as "//evil.com"
    while a plain startswith() check does not. The client re-checks the result
    against its own origin before navigating (site_auth.js).
    """
    if not raw:
        return ""
    parts = urlsplit(raw.replace("\\", "/"))
    if parts.scheme or parts.netloc or not parts.path.startswith("/"):
        return ""
    return urlunsplit(("", "", parts.path, parts.query, parts.fragment))


def _back(fragment: str, next_: str = "") -> RedirectResponse:
    # Land back on the showcase-site login page; the one-time code rides in the
    # URL fragment so it never reaches a server log. ``next`` rides in the query
    # string instead - the page reads it to resume wherever sign-in interrupted.
    query = f"?next={quote(next_, safe='/')}" if next_ else ""
    return RedirectResponse(f"{settings.app_url}/login{query}#{fragment}", status_code=307)


def _app_return(fragment: str) -> HTMLResponse:
    # Desktop-app (BetterTroveTools) login. The browser can't be 307'd straight
    # to a custom scheme, so we serve a tiny interstitial that navigates to the
    # app's `btt://` deep link (handled by web/js/main.js handle_deep_link). The
    # one-time code rides in the query string of the local-only btt:// URL.
    target = f"btt://auth/discord?{fragment}"
    target_js = json.dumps(target)
    target_attr = target.replace('"', "%22")
    html = f"""<!doctype html>
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Better Trove Tools — Sign in</title>
<style>
  html,body{{height:100%;margin:0}}
  body{{display:flex;align-items:center;justify-content:center;background:#0f1216;
       color:#e8edf2;font-family:'Segoe UI',system-ui,sans-serif}}
  .card{{text-align:center;max-width:420px;padding:40px 32px;background:#171b21;
        border:1px solid #232a33;border-radius:16px}}
  h1{{font-size:20px;margin:0 0 8px}}
  p{{color:#9aa7b4;margin:0 0 20px;line-height:1.5}}
  a.btn{{display:inline-block;background:#5ec6ff;color:#06202b;text-decoration:none;
        font-weight:600;padding:11px 22px;border-radius:10px}}
</style></head>
<body>
  <div class="card">
    <h1>Returning to Better Trove Tools…</h1>
    <p>You're signed in. If the app didn't come back to the front,
       use the button below. You can close this tab.</p>
    <a class="btn" href="{target_attr}">Open Better Trove Tools</a>
  </div>
  <script>location.replace({target_js});</script>
</body></html>"""
    return HTMLResponse(html)


def _emit(mode: str, *, ok: str | None = None, error: str | None = None, next_: str = ""):
    """Hand the OAuth result back to the right client. ``mode == "app"`` returns
    the btt:// interstitial; anything else returns the showcase-site redirect.
    ``next_`` is web-only - the desktop app has no page to resume."""
    if mode == "app":
        return _app_return(f"code={ok}" if ok is not None else f"error={error}")
    return _back(f"discord={ok}" if ok is not None else f"oauth_error={error}", next_)


@router.get("/discord/start", include_in_schema=False)
async def discord_start(client: str | None = None, next: str | None = None):
    _require_enabled()
    # client=app -> finish the login inside the BetterTroveTools desktop app via
    # a btt:// deep link instead of redirecting back to the showcase site. The
    # mode is carried in the state value so the callback knows where to return,
    # and so is the page the user was heading for - Discord round-trips through
    # its own domain, so nothing else survives the trip.
    mode = "app" if client == "app" else "web"
    next_ = _safe_next(next)
    r = get_redis()
    if r is None:
        return _emit(mode, error="unavailable", next_=next_)
    state = secrets.token_urlsafe(24)
    await r.set(f"site_oauthstate:{state}", json.dumps({"mode": mode, "next": next_}), ex=600)
    params = {
        "client_id": settings.discord_client_id,
        "redirect_uri": _redirect_uri(),
        "response_type": "code",
        "scope": "identify guilds",
        "state": state,
    }
    return RedirectResponse(f"{_AUTHORIZE}?{urlencode(params)}", status_code=307)


@router.get("/discord/callback", include_in_schema=False)
async def discord_callback(request: Request, code: str | None = None, state: str | None = None):
    _require_enabled()
    r = get_redis()
    if r is None:
        return _back("oauth_error=unavailable")
    # Consume the state first (Discord echoes it back even on a denial), so we
    # know whether to return to the app or the website - and to which page - for
    # every later branch.
    mode, next_ = "web", ""
    if state:
        stored = await r.getdel(f"site_oauthstate:{state}")
        if not stored:
            return _back("oauth_error=state")
        try:
            parsed = json.loads(stored)
        except (TypeError, ValueError):
            parsed = {"mode": stored}   # a state issued before ``next`` was carried
        mode = "app" if parsed.get("mode") == "app" else "web"
        next_ = _safe_next(parsed.get("next"))
    if not code or not state:
        return _emit(mode, error="missing", next_=next_)

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            tok = await client.post(_TOKEN, data={
                "client_id": settings.discord_client_id,
                "client_secret": settings.discord_client_secret,
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": _redirect_uri(),
            })
            tok_json = tok.json()
            access = tok_json.get("access_token")
            if not access:
                return _emit(mode, error="exchange", next_=next_)
            expires_in = tok_json.get("expires_in")
            auth_header = {"Authorization": f"Bearer {access}"}
            me = (await client.get(f"{_API}/users/@me", headers=auth_header)).json()
            # The guild (server) list is NOT fetched or stored here anymore - it's
            # pulled live from Discord only when the user opens the Dashboard's
            # "Discord Bot" tab (see fetch_discord_guilds), using the cached token.
    except httpx.HTTPError:
        logger.exception("Discord OAuth request failed")
        return _emit(mode, error="discord", next_=next_)

    raw_id = me.get("id")
    # We no longer request the `email` scope or store an email - the Discord
    # account id is the sole identity (data minimization).
    if not raw_id:
        return _emit(mode, error="discord", next_=next_)
    try:
        discord_id = int(raw_id)
    except (TypeError, ValueError):
        return _emit(mode, error="discord", next_=next_)

    user = await _find_or_create(discord_id, me)
    if not user.is_active:
        return _emit(mode, error="inactive", next_=next_)
    user.last_login_at = utcnow()
    await user.save()
    # Cache the Discord token (not the guild list) so the Dashboard can fetch the
    # user's servers on demand. Best-effort: a Redis outage just means the "Discord
    # Bot" tab shows the reconnect prompt until the next login.
    await store_discord_token(user.id, access, expires_in)
    tokens = await issue_tokens(user, request)

    xcode = secrets.token_urlsafe(24)
    await r.set(
        f"site_oauthx:{xcode}",
        json.dumps({"a": tokens.access_token, "r": tokens.refresh_token}),
        ex=120,
    )
    return _emit(mode, ok=xcode, next_=next_)


@router.post("/exchange", response_model=SiteTokenResponse)
async def oauth_exchange(payload: _ExchangeBody, response: Response) -> SiteTokenResponse:
    """Swap the one-time code from the Discord redirect for real site tokens.

    This is where a browser session becomes cookie-backed: the tokens are still
    returned in the body (the desktop app reads them from there), but a browser
    gets the HttpOnly pair set here and never touches the body values.
    """
    r = get_redis()
    raw = await r.getdel(f"site_oauthx:{payload.code}") if r is not None else None
    if not isinstance(raw, str):
        raise APIError(400, ErrorCode.bad_request, "Invalid or expired exchange code")
    d = json.loads(raw)
    tokens = SiteTokenResponse(
        access_token=d["a"],
        refresh_token=d["r"],
        expires_in=settings.access_token_expire_minutes * 60,
    )
    set_session_cookies(response, tokens)
    return tokens


async def _unique_username(seed: str) -> str:
    """A valid (``[a-z0-9_.]{3,24}``, Discord-style), unique SiteUser username derived
    from the Discord name, with a random suffix on collision / too-short. Periods are
    kept (Discord allows them, trailing too) but cleaned to satisfy our rules: no
    ``..`` and no leading dot."""
    base = re.sub(r"[^a-z0-9_.]", "", (seed or "").lower())
    base = re.sub(r"\.{2,}", ".", base).lstrip(".")[:20]
    if len(base) < 3:
        base = (base + "kiwi")[:20]
    candidate = base
    for _ in range(12):
        if await SiteUser.find_one(SiteUser.username == candidate) is None:
            return candidate
        candidate = (base[:18] + secrets.token_hex(2))[:24]
    return "kiwi" + secrets.token_hex(8)


async def _find_or_create(discord_id: int, me: dict) -> SiteUser:
    avatar = me.get("avatar")
    handle = (me.get("username") or me.get("global_name") or "").strip()
    user = await SiteUser.find_one(SiteUser.discord_id == discord_id)
    if user is not None:
        # Keep the avatar hash + LIVE Discord handle in sync on every login. NOTE:
        # `username` (the frozen Trove handle) is deliberately NOT touched here - it
        # only changes via the admin-approved request flow, so a Discord rename can't
        # shift the user's mod handles/URLs. display_name is user-editable too.
        dirty = False
        if user.discord_avatar != avatar:
            user.discord_avatar = avatar
            dirty = True
        if handle and user.discord_handle != handle:
            user.discord_handle = handle
            dirty = True
        if not user.discord_handle:           # backfill for accounts created pre-discord_handle
            user.discord_handle = handle or user.username
            dirty = True
        if dirty:
            await user.save()
        return user
    # New account - the Discord id is the sole identity (no email stored/linked).
    user = SiteUser(
        # The frozen Trove username INHERITS the Discord handle at signup (sanitized
        # to a url-safe, unique handle); it diverges only via an approved change.
        username=await _unique_username(handle),
        discord_handle=handle,
        display_name=me.get("global_name") or me.get("username"),
        is_verified=True,          # a Discord login is inherently identity-verified
        discord_id=discord_id,
        discord_avatar=avatar,
    )
    await user.insert()
    return user
