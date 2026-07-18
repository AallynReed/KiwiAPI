"""FastAPI dependency that resolves a Bearer token into a ``SiteUser``.

Same shape as ``app.core.dependencies.get_current_user`` but:
  - requires the access token to carry ``kind=site`` (rejects dev-portal
    tokens that happen to be presented to a /site-auth endpoint)
  - loads from the ``site_users`` collection, not ``users``
"""
from dataclasses import dataclass

import jwt
from beanie import PydanticObjectId
from fastapi import Depends
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from app.core.errors import APIError, ErrorCode
from app.core.security import decode_access_token
from app.site_auth.models import SiteUser
from app.site_auth.sessions import TOKEN_KIND

_jwt_scheme = HTTPBearer(
    scheme_name="SiteSessionJWT",
    description="JWT from Discord sign-in (/v1/site-auth/oauth/*)",
    auto_error=False,
)


def _not_authenticated(message: str = "Authentication required") -> APIError:
    return APIError(
        status_code=401,
        code=ErrorCode.not_authenticated,
        message=message,
        headers={"WWW-Authenticate": "Bearer"},
    )


@dataclass
class SiteAuthContext:
    user: SiteUser
    session_id: str | None


async def _authenticate(
    creds: HTTPAuthorizationCredentials | None,
) -> tuple[SiteUser, dict]:
    if creds is None:
        raise _not_authenticated()
    try:
        payload = decode_access_token(creds.credentials)
        user_id = payload["sub"]
    except (jwt.PyJWTError, KeyError):
        raise _not_authenticated("Invalid or expired session token")

    if payload.get("kind") != TOKEN_KIND:
        raise _not_authenticated("Wrong-audience token")

    user = await SiteUser.get(PydanticObjectId(user_id))
    if user is None or not user.is_active:
        raise _not_authenticated("Account is inactive or no longer exists")
    if payload.get("ver") != user.token_version:
        raise _not_authenticated("Session has been ended; please log in again")
    return user, payload


async def get_current_site_user(
    creds: HTTPAuthorizationCredentials | None = Depends(_jwt_scheme),
) -> SiteUser:
    user, _ = await _authenticate(creds)
    return user


async def get_optional_site_user(
    creds: HTTPAuthorizationCredentials | None = Depends(_jwt_scheme),
) -> SiteUser | None:
    """Like ``get_current_site_user`` but returns ``None`` for an anonymous
    caller instead of raising. Used by public, browse-while-maybe-logged-in
    surfaces (e.g. the Mods Hub) so a page can stay public yet reveal the
    owner's own drafts + owner-only controls when a valid session is present.

    A *malformed* credential is still treated as anonymous here (not a 401) -
    the endpoint is public, so a stale token shouldn't break the read; the
    write endpoints that use ``get_current_site_user`` surface the 401."""
    if creds is None:
        return None
    try:
        user, _ = await _authenticate(creds)
    except Exception:  # noqa: BLE001 - public surface: any bad cred is just anonymous
        return None
    return user


async def get_site_auth_context(
    creds: HTTPAuthorizationCredentials | None = Depends(_jwt_scheme),
) -> SiteAuthContext:
    user, payload = await _authenticate(creds)
    return SiteAuthContext(user=user, session_id=payload.get("sid"))


async def get_current_verified_site_user(
    user: SiteUser = Depends(get_current_site_user),
) -> SiteUser:
    """Gates actions that require a verified account (claim a Trove name, etc).
    Sign-in is Discord-only, so every account is inherently identity-verified;
    this stays as a guard for any account explicitly flagged unverified."""
    if not user.is_verified:
        raise APIError(
            status_code=403,
            code=ErrorCode.email_unverified,
            message="Your account isn't verified yet.",
        )
    return user
