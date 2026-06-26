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
from urllib.parse import urlencode

import httpx
from fastapi import APIRouter, Request
from fastapi.responses import HTMLResponse, RedirectResponse
from pydantic import BaseModel

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.redis import get_redis
from app.core.utils import utcnow
from app.site_auth.models import SiteUser
from app.site_auth.schemas import SiteTokenResponse
from app.site_auth.sessions import issue_tokens

logger = logging.getLogger("kiwi.site_auth.oauth")

router = APIRouter(prefix="/v1/site-auth/oauth", tags=["site-auth"])

_AUTHORIZE = "https://discord.com/oauth2/authorize"
_TOKEN = "https://discord.com/api/oauth2/token"
_API = "https://discord.com/api"


class _ExchangeBody(BaseModel):
    code: str


def _require_enabled() -> None:
    if not settings.discord_oauth_enabled:
        raise APIError(404, ErrorCode.not_found, "Discord sign-in is not configured")


def _redirect_uri() -> str:
    return f"{settings.api_url}/v1/site-auth/oauth/discord/callback"


def _back(fragment: str) -> RedirectResponse:
    # Land back on the showcase-site login page; the one-time code rides in the
    # URL fragment so it never reaches a server log.
    return RedirectResponse(f"{settings.app_url}/login#{fragment}", status_code=307)


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


def _emit(mode: str, *, ok: str | None = None, error: str | None = None):
    """Hand the OAuth result back to the right client. ``mode == "app"`` returns
    the btt:// interstitial; anything else returns the showcase-site redirect."""
    if mode == "app":
        return _app_return(f"code={ok}" if ok is not None else f"error={error}")
    return _back(f"discord={ok}" if ok is not None else f"oauth_error={error}")


@router.get("/discord/start", include_in_schema=False)
async def discord_start(client: str | None = None):
    _require_enabled()
    # client=app -> finish the login inside the BetterTroveTools desktop app via
    # a btt:// deep link instead of redirecting back to the showcase site. The
    # mode is carried in the state value so the callback knows where to return.
    mode = "app" if client == "app" else "web"
    r = get_redis()
    if r is None:
        return _emit(mode, error="unavailable")
    state = secrets.token_urlsafe(24)
    await r.set(f"site_oauthstate:{state}", mode, ex=600)
    params = {
        "client_id": settings.discord_client_id,
        "redirect_uri": _redirect_uri(),
        "response_type": "code",
        "scope": "identify email guilds",   # + guilds so the dashboard can list the user's servers
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
    # know whether to return to the app or the website for every later branch.
    mode = "web"
    if state:
        stored = await r.getdel(f"site_oauthstate:{state}")
        if not stored:
            return _back("oauth_error=state")
        mode = "app" if stored == "app" else "web"
    if not code or not state:
        return _emit(mode, error="missing")

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            tok = await client.post(_TOKEN, data={
                "client_id": settings.discord_client_id,
                "client_secret": settings.discord_client_secret,
                "grant_type": "authorization_code",
                "code": code,
                "redirect_uri": _redirect_uri(),
            })
            access = tok.json().get("access_token")
            if not access:
                return _emit(mode, error="exchange")
            auth_header = {"Authorization": f"Bearer {access}"}
            me = (await client.get(f"{_API}/users/@me", headers=auth_header)).json()
            # Best-effort guild list (needs the `guilds` scope). A declined scope
            # or an older grant just leaves this None; the Dashboard reprompts.
            guilds_raw = None
            try:
                gr = await client.get(f"{_API}/users/@me/guilds", headers=auth_header)
                if gr.status_code == 200:
                    guilds_raw = gr.json()
            except (httpx.HTTPError, ValueError):
                guilds_raw = None
    except httpx.HTTPError:
        logger.exception("Discord OAuth request failed")
        return _emit(mode, error="discord")

    raw_id = me.get("id")
    email = me.get("email")
    # Only a VERIFIED Discord email is trusted for sign-in / linking.
    if not raw_id or not email or not me.get("verified"):
        return _emit(mode, error="noemail")
    try:
        discord_id = int(raw_id)
    except (TypeError, ValueError):
        return _emit(mode, error="discord")
    email = email.lower()

    user = await _find_or_create(discord_id, email, me)
    if not user.is_active:
        return _emit(mode, error="inactive")
    if isinstance(guilds_raw, list):
        user.discord_guilds = [
            {"id": str(g["id"]), "name": g.get("name", ""), "icon": g.get("icon"),
             "owner": bool(g.get("owner")), "permissions": str(g.get("permissions", "0"))}
            for g in guilds_raw if isinstance(g, dict) and g.get("id")
        ]
        user.discord_guilds_synced_at = utcnow()
    user.last_login_at = utcnow()
    await user.save()
    tokens = await issue_tokens(user, request)

    xcode = secrets.token_urlsafe(24)
    await r.set(
        f"site_oauthx:{xcode}",
        json.dumps({"a": tokens.access_token, "r": tokens.refresh_token}),
        ex=120,
    )
    return _emit(mode, ok=xcode)


@router.post("/exchange", response_model=SiteTokenResponse)
async def oauth_exchange(payload: _ExchangeBody) -> SiteTokenResponse:
    """Swap the one-time code from the Discord redirect for real site tokens."""
    r = get_redis()
    raw = await r.getdel(f"site_oauthx:{payload.code}") if r is not None else None
    if not isinstance(raw, str):
        raise APIError(400, ErrorCode.bad_request, "Invalid or expired exchange code")
    d = json.loads(raw)
    return SiteTokenResponse(
        access_token=d["a"],
        refresh_token=d["r"],
        expires_in=settings.access_token_expire_minutes * 60,
    )


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


async def _find_or_create(discord_id: int, email: str, me: dict) -> SiteUser:
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
    # Link to an existing same-email account (safe: Discord only reports a
    # VERIFIED email, so the person completing the flow controls this address).
    user = await SiteUser.find_one(SiteUser.email == email)
    if user is not None:
        user.discord_id = discord_id
        user.is_verified = True
        user.discord_avatar = avatar
        user.discord_handle = handle or user.discord_handle or user.username
        await user.save()
        return user
    user = SiteUser(
        # The frozen Trove username INHERITS the Discord handle at signup (sanitized
        # to a url-safe, unique handle); it diverges only via an approved change.
        username=await _unique_username(handle),
        discord_handle=handle,
        email=email,
        display_name=me.get("global_name") or me.get("username"),
        is_verified=True,
        discord_id=discord_id,
        discord_avatar=avatar,
    )
    await user.insert()
    return user
