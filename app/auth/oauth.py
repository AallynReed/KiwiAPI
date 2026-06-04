import json
import logging
import secrets
from urllib.parse import urlencode

import httpx
from fastapi import APIRouter, Request
from fastapi.responses import RedirectResponse

from app.auth.emails import send_github_linked_email
from app.auth.models import User
from app.auth.schemas import OAuthExchangeRequest, TokenResponse
from app.auth.sessions import issue_tokens
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.redis import get_redis
from app.core.security import hash_password
from app.core.utils import utcnow

logger = logging.getLogger("kiwi.oauth")

router = APIRouter(prefix="/auth/oauth", tags=["auth"])

_AUTHORIZE = "https://github.com/login/oauth/authorize"
_TOKEN = "https://github.com/login/oauth/access_token"
_API = "https://api.github.com"


def _require_enabled() -> None:
    if not settings.github_oauth_enabled:
        raise APIError(404, ErrorCode.not_found, "GitHub sign-in is not configured")


def _redirect_uri() -> str:
    return f"{settings.api_url}/auth/oauth/github/callback"


def _back(fragment: str) -> RedirectResponse:
    # Always land the browser back on the SPA, passing the result in the URL
    # fragment (never sent to a server, so the one-time code isn't logged).
    return RedirectResponse(f"{settings.dev_url}/#{fragment}", status_code=307)


@router.get("/github/start", include_in_schema=False)
async def github_start():
    _require_enabled()
    r = get_redis()
    if r is None:
        # OAuth needs Redis for the CSRF state + one-time code exchange. Without
        # it we cannot validate state, so refuse rather than degrade open.
        return _back("oauth_error=unavailable")
    state = secrets.token_urlsafe(24)
    await r.set(f"oauthstate:{state}", "1", ex=600)
    params = {
        "client_id": settings.github_client_id,
        "redirect_uri": _redirect_uri(),
        "scope": "read:user user:email",
        "state": state,
        "allow_signup": "true",
    }
    return RedirectResponse(f"{_AUTHORIZE}?{urlencode(params)}", status_code=307)


@router.get("/github/callback", include_in_schema=False)
async def github_callback(request: Request, code: str | None = None, state: str | None = None):
    _require_enabled()
    if not code or not state:
        return _back("oauth_error=missing")

    r = get_redis()
    if r is None:
        return _back("oauth_error=unavailable")
    # Single-use state: delete returns 0 if it was never set / already used.
    if not await r.delete(f"oauthstate:{state}"):
        return _back("oauth_error=state")

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            tok = await client.post(
                _TOKEN,
                headers={"Accept": "application/json"},
                data={
                    "client_id": settings.github_client_id,
                    "client_secret": settings.github_client_secret,
                    "code": code,
                    "redirect_uri": _redirect_uri(),
                },
            )
            access = tok.json().get("access_token")
            if not access:
                return _back("oauth_error=exchange")
            gh_headers = {"Authorization": f"Bearer {access}", "Accept": "application/vnd.github+json"}
            gh_user = (await client.get(f"{_API}/user", headers=gh_headers)).json()
            gh_emails = (await client.get(f"{_API}/user/emails", headers=gh_headers)).json()
    except httpx.HTTPError:
        logger.exception("GitHub OAuth request failed")
        return _back("oauth_error=github")

    github_id = gh_user.get("id")
    # Only ever trust a *verified* email for sign-in/linking — never the public
    # profile email (which may be unverified). No verified email -> refuse.
    email = _pick_email(gh_emails)
    if github_id is None or not email:
        return _back("oauth_error=noemail")
    email = email.lower()

    user = await _find_or_create(github_id, email, gh_user)
    if not user.is_active:
        return _back("oauth_error=inactive")

    user.last_login_at = utcnow()
    await user.save()
    tokens = await issue_tokens(user, request)

    # Hand the tokens to the SPA via a short-lived one-time exchange code.
    xcode = secrets.token_urlsafe(24)
    await r.set(
        f"oauthx:{xcode}",
        json.dumps({"a": tokens.access_token, "r": tokens.refresh_token}),
        ex=120,
    )
    return _back(f"oauth={xcode}")


def _pick_email(emails) -> str | None:
    if not isinstance(emails, list):
        return None
    for e in emails:
        if e.get("primary") and e.get("verified"):
            return e.get("email")
    for e in emails:
        if e.get("verified"):
            return e.get("email")
    return None


async def _find_or_create(github_id: int, email: str, gh_user: dict) -> User:
    user = await User.find_one(User.github_id == github_id)
    if user is not None:
        return user
    # Link to an existing same-email account, or create a fresh one. Linking is
    # safe because GitHub only reports *verified* emails (see _pick_email), so the
    # person completing the flow controls this address. We still notify the owner.
    user = await User.find_one(User.email == email)
    if user is not None:
        user.github_id = github_id
        user.is_verified = True
        user.email_bounced = False  # GitHub-verified address — resume delivery
        await user.save()
        if settings.security_email_notifications:
            await send_github_linked_email(user)
        return user
    user = User(
        email=email,
        hashed_password=hash_password(secrets.token_urlsafe(32)),  # unusable until reset
        display_name=gh_user.get("name") or gh_user.get("login"),
        is_verified=True,
        github_id=github_id,
    )
    await user.insert()
    return user


@router.post("/exchange", response_model=TokenResponse)
async def oauth_exchange(payload: OAuthExchangeRequest) -> TokenResponse:
    """Swap the one-time code from the OAuth redirect for real tokens."""
    r = get_redis()
    raw = await r.getdel(f"oauthx:{payload.code}") if r is not None else None
    if raw is None:
        raise APIError(400, ErrorCode.bad_request, "Invalid or expired exchange code")
    if not isinstance(raw, str):  # defensive: Redis returns str with decode_responses
        raise APIError(400, ErrorCode.bad_request, "Invalid or expired exchange code")
    d = json.loads(raw)
    return TokenResponse(
        access_token=d["a"],
        refresh_token=d["r"],
        expires_in=settings.access_token_expire_minutes * 60,
    )
