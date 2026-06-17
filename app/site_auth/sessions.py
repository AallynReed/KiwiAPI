"""Session lifecycle for site users - issue / refresh / revoke.

Cribs the dev-portal pattern (see ``app/auth/sessions.py``) but:

  - persists into the ``site_sessions`` collection
  - mints access tokens with a ``"kind": "site"`` claim so a token meant
    for trove.aallyn.net can NEVER be presented to ``/v1/*`` and pass
    ``get_current_user``. Dev-portal tokens lack the claim and are
    rejected by ``get_current_site_user`` for the symmetric reason.
"""
from datetime import timedelta

import jwt
from beanie.operators import Set
from fastapi import Request

from app.core.config import settings
from app.core.security import generate_refresh_token, hash_token
from app.core.utils import client_ip, utcnow
from app.site_auth.models import SiteSession, SiteUser
from app.site_auth.schemas import SiteTokenResponse

# Distinguishes site-user access tokens from dev-portal ones. Read by the
# ``get_current_site_user`` dependency; missing or wrong = 401.
TOKEN_KIND = "site"


def _user_agent(request: Request) -> str | None:
    ua = request.headers.get("user-agent")
    return ua[:300] if ua else None


def create_site_access_token(
    subject: str, token_version: int, session_id: str | None = None,
) -> str:
    """Mint an access token for a SiteUser. Identical shape to the
    dev-portal token EXCEPT for the ``kind=site`` claim - same secret,
    same algorithm, same exp window. The kind discriminator is what
    keeps the two surfaces from cross-contaminating."""
    now = utcnow()
    expire = now + timedelta(minutes=settings.access_token_expire_minutes)
    payload = {
        "sub": str(subject),
        "ver": token_version,
        "iat": now,
        "exp": expire,
        "type": "access",
        "kind": TOKEN_KIND,
    }
    if session_id is not None:
        payload["sid"] = session_id
    return jwt.encode(payload, settings.secret_key, algorithm=settings.jwt_algorithm)


async def issue_tokens(user: SiteUser, request: Request) -> SiteTokenResponse:
    """Open a new session row and return access + refresh tokens."""
    assert user.id is not None
    refresh_token, refresh_hash = generate_refresh_token()
    session = SiteSession(
        site_user_id=user.id,
        refresh_token_hash=refresh_hash,
        ip=client_ip(request),
        user_agent=_user_agent(request),
        expires_at=utcnow() + timedelta(days=settings.refresh_token_expire_days),
    )
    await session.insert()
    access = create_site_access_token(str(user.id), user.token_version, session_id=str(session.id))
    return SiteTokenResponse(
        access_token=access,
        refresh_token=refresh_token,
        expires_in=settings.access_token_expire_minutes * 60,
    )


async def revoke_all_sessions(user: SiteUser) -> None:
    """Bump ``token_version`` (instantly invalidates every outstanding
    access token) and mark every active session row as revoked. Use on
    password change, email change, and explicit logout-all."""
    user.token_version += 1
    user.updated_at = utcnow()
    await user.save()
    active = SiteSession.find(
        SiteSession.site_user_id == user.id,
        SiteSession.revoked == False,  # noqa: E712
    )
    await active.update(Set({SiteSession.revoked: True}))  # pyright: ignore[reportGeneralTypeIssues]


async def rotate(refresh_token: str, request: Request) -> SiteTokenResponse | None:
    """Single-use rotate of a refresh token. Returns None on any
    invalidity (expired / revoked / wrong-kind user / inactive)."""
    session = await SiteSession.find_one(
        SiteSession.refresh_token_hash == hash_token(refresh_token),
    )
    now = utcnow()
    if session is None or session.revoked or session.expires_at < now:
        return None
    user = await SiteUser.get(session.site_user_id)
    if user is None or not user.is_active:
        return None

    new_refresh, new_hash = generate_refresh_token()
    session.refresh_token_hash = new_hash
    session.last_used_at = now
    session.ip = client_ip(request)
    session.user_agent = _user_agent(request)
    await session.save()

    access = create_site_access_token(str(user.id), user.token_version, session_id=str(session.id))
    return SiteTokenResponse(
        access_token=access,
        refresh_token=new_refresh,
        expires_in=settings.access_token_expire_minutes * 60,
    )


async def revoke_by_refresh_token(refresh_token: str) -> None:
    """End one session by its refresh token (idempotent - no-op if
    already revoked or unknown)."""
    if not refresh_token:
        return
    session = await SiteSession.find_one(
        SiteSession.refresh_token_hash == hash_token(refresh_token),
    )
    if session is not None and not session.revoked:
        session.revoked = True
        await session.save()
