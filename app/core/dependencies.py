import ipaddress
from dataclasses import dataclass

import jwt
from beanie import PydanticObjectId
from fastapi import Depends, Request, Response
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

from app.auth.models import User
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.limits import endpoint_limit_for
from app.core.ratelimit import check_rate_limit, rate_limit_headers
from app.core.scopes import mask_grants
from app.core.security import decode_access_token, hash_token, verify_token_checksum
from app.core.utils import utcnow
from app.tokens.models import ApiToken
from app.tokens.usage import record_token_use

# Two distinct bearer schemes so Swagger's "Authorize" dialog shows both.
# auto_error=False so we raise our own consistent 401 envelope when missing.
_jwt_scheme = HTTPBearer(
    scheme_name="SessionJWT", description="JWT from /auth/login", auto_error=False
)
_api_scheme = HTTPBearer(
    scheme_name="APIToken", description="API token from /tokens", auto_error=False
)


def _not_authenticated(message: str = "Authentication required") -> APIError:
    return APIError(
        status_code=401,
        code=ErrorCode.not_authenticated,
        message=message,
        headers={"WWW-Authenticate": "Bearer"},
    )


# --- Session (JWT) auth: account + token management ------------------------

@dataclass
class AuthContext:
    user: User
    session_id: str | None


async def _authenticate(creds: HTTPAuthorizationCredentials | None) -> tuple[User, dict]:
    if creds is None:
        raise _not_authenticated()
    try:
        payload = decode_access_token(creds.credentials)
        user_id = payload["sub"]
    except (jwt.PyJWTError, KeyError):
        raise _not_authenticated("Invalid or expired session token")

    user = await User.get(PydanticObjectId(user_id))
    if user is None or not user.is_active:
        raise _not_authenticated("Account is inactive or no longer exists")
    # Reject tokens minted before a logout-all / password change.
    if payload.get("ver") != user.token_version:
        raise _not_authenticated("Session has been ended; please log in again")
    return user, payload


async def get_current_user(
    creds: HTTPAuthorizationCredentials | None = Depends(_jwt_scheme),
) -> User:
    user, _ = await _authenticate(creds)
    return user


async def get_auth_context(
    creds: HTTPAuthorizationCredentials | None = Depends(_jwt_scheme),
) -> AuthContext:
    user, payload = await _authenticate(creds)
    return AuthContext(user=user, session_id=payload.get("sid"))


async def get_current_superuser(user: User = Depends(get_current_user)) -> User:
    if not user.is_superuser:
        raise APIError(
            status_code=403,
            code=ErrorCode.forbidden,
            message="Administrator privileges required",
        )
    return user


# --- API token auth: the queryable data API --------------------------------

@dataclass
class TokenContext:
    user: User
    token: ApiToken


def _client_ip(request: Request) -> str:
    # uvicorn runs with --proxy-headers, so this reflects the real client.
    return request.client.host if request.client else ""


def _ip_allowed(client_ip: str, allowed: list[str]) -> bool:
    """True if client_ip falls within any allowed exact IP or CIDR."""
    try:
        addr = ipaddress.ip_address(client_ip)
    except ValueError:
        return False
    for entry in allowed:
        try:
            if addr in ipaddress.ip_network(entry, strict=False):
                return True
        except ValueError:
            continue
    return False


async def get_token_context(
    request: Request,
    response: Response,
    creds: HTTPAuthorizationCredentials | None = Depends(_api_scheme),
) -> TokenContext:
    if creds is None:
        raise _not_authenticated("API token required")

    # Reject tokens whose self-validating checksum fails — no DB hit needed.
    # (None = legacy/unknown shape, so fall through to the database lookup.)
    if verify_token_checksum(creds.credentials) is False:
        raise _not_authenticated("Invalid or revoked API token")

    token = await ApiToken.find_one(
        ApiToken.hashed_token == hash_token(creds.credentials)
    )
    if token is None or token.revoked:
        raise _not_authenticated("Invalid or revoked API token")

    now = utcnow()
    if token.expires_at is not None and token.expires_at < now:
        raise _not_authenticated("API token has expired")

    user = await User.get(token.user_id)
    if user is None or not user.is_active:
        raise _not_authenticated("Account is inactive or no longer exists")

    # Identify this request to the usage-recording middleware (set before the
    # rate-limit check so throttled 429s are captured in activity metrics too).
    request.state.usage_user_id = user.id
    request.state.usage_token_id = token.id

    # IP allowlist — enforced whenever the token pins source IPs (always, for
    # tokens minted after this feature; older keys with no IPs are unrestricted).
    if token.allowed_ips and not _ip_allowed(_client_ip(request), token.allowed_ips):
        raise APIError(
            status_code=403,
            code=ErrorCode.ip_not_allowed,
            message="Requests from this IP are not allowed for this token",
        )

    # Per-token throughput cap — protects compute-heavy endpoints from one key.
    info = await check_rate_limit(
        f"token:{token.id}",
        settings.api_rate_limit_max,
        settings.api_rate_limit_window_seconds,
    )
    # Surface limit state on every successful response so clients self-throttle.
    response.headers.update(rate_limit_headers(info))

    # Tighter per-endpoint budget on top of the global cap, where configured.
    route = request.scope.get("route")
    extra = endpoint_limit_for(getattr(route, "path", None))
    if extra is not None:
        await check_rate_limit(f"ep:{getattr(route, 'path', '')}:{token.id}", *extra)

    # Usage accounting — coalesced into ~one write per interval when Redis is up.
    await record_token_use(token, _client_ip(request) or None)

    return TokenContext(user=user, token=token)


def require_scope(scope: str):
    """Dependency factory: guard an endpoint behind a scope.

    Usage: `ctx: TokenContext = Depends(require_scope("items"))`. Passes if the
    token carries the scope or has `all_scopes`; otherwise raises 403.
    """

    async def checker(ctx: TokenContext = Depends(get_token_context)) -> TokenContext:
        if not mask_grants(ctx.token.scopes, scope):
            raise APIError(
                status_code=403,
                code=ErrorCode.insufficient_scope,
                message=f"This token is missing the required scope: {scope}",
            )
        return ctx

    return checker
