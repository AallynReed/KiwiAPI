from datetime import timedelta

from beanie import PydanticObjectId
from beanie.operators import Set
from fastapi import APIRouter, Depends, Request, status

from app.auth.models import Session, User
from app.auth.schemas import (
    LogoutRequest,
    RefreshRequest,
    SessionPublic,
    TokenResponse,
)
from app.core.config import settings
from app.core.dependencies import AuthContext, get_auth_context, get_current_user
from app.core.errors import APIError, ErrorCode
from app.core.security import create_access_token, generate_refresh_token, hash_token
from app.core.utils import client_ip, utcnow

router = APIRouter(prefix="/auth", tags=["auth"])


def _user_agent(request: Request) -> str | None:
    ua = request.headers.get("user-agent")
    return ua[:300] if ua else None


async def issue_tokens(user: User, request: Request) -> TokenResponse:
    """Open a new session (refresh token) and return access + refresh tokens."""
    assert user.id is not None  # always set on a persisted user
    refresh_token, refresh_hash = generate_refresh_token()
    session = Session(
        user_id=user.id,
        refresh_token_hash=refresh_hash,
        ip=client_ip(request),
        user_agent=_user_agent(request),
        expires_at=utcnow() + timedelta(days=settings.refresh_token_expire_days),
    )
    await session.insert()
    access = create_access_token(str(user.id), user.token_version, session_id=str(session.id))
    return TokenResponse(
        access_token=access,
        refresh_token=refresh_token,
        expires_in=settings.access_token_expire_minutes * 60,
    )


async def revoke_all_sessions(user: User) -> None:
    """Invalidate every access token (bump token_version) and revoke all sessions.

    Use on password change, email change, and logout-all.
    """
    user.token_version += 1
    user.updated_at = utcnow()
    await user.save()
    active = Session.find(Session.user_id == user.id, Session.revoked == False)  # noqa: E712
    await active.update(Set({Session.revoked: True}))  # pyright: ignore[reportGeneralTypeIssues]


def _to_public(session: Session, current_id: str | None) -> SessionPublic:
    return SessionPublic(
        id=str(session.id),
        ip=session.ip,
        user_agent=session.user_agent,
        created_at=session.created_at,
        last_used_at=session.last_used_at,
        expires_at=session.expires_at,
        current=str(session.id) == current_id,
    )


@router.post("/refresh", response_model=TokenResponse)
async def refresh(payload: RefreshRequest, request: Request) -> TokenResponse:
    """Exchange a refresh token for a new access token, rotating the refresh token."""
    session = await Session.find_one(
        Session.refresh_token_hash == hash_token(payload.refresh_token)
    )
    now = utcnow()
    if session is None or session.revoked or session.expires_at < now:
        raise APIError(401, ErrorCode.not_authenticated, "Invalid or expired refresh token")

    user = await User.get(session.user_id)
    if user is None or not user.is_active:
        raise APIError(401, ErrorCode.not_authenticated, "Account is inactive")

    # Rotate: a refresh token is single-use.
    new_refresh, new_hash = generate_refresh_token()
    session.refresh_token_hash = new_hash
    session.last_used_at = now
    session.ip = client_ip(request)
    session.user_agent = _user_agent(request)
    await session.save()

    access = create_access_token(str(user.id), user.token_version, session_id=str(session.id))
    return TokenResponse(
        access_token=access,
        refresh_token=new_refresh,
        expires_in=settings.access_token_expire_minutes * 60,
    )


@router.post("/logout", status_code=status.HTTP_204_NO_CONTENT)
async def logout(payload: LogoutRequest) -> None:
    """End a session by its refresh token (idempotent)."""
    if not payload.refresh_token:
        return
    session = await Session.find_one(
        Session.refresh_token_hash == hash_token(payload.refresh_token)
    )
    if session is not None and not session.revoked:
        session.revoked = True
        await session.save()


@router.post("/logout-all", status_code=status.HTTP_204_NO_CONTENT)
async def logout_all(user: User = Depends(get_current_user)) -> None:
    """End every session for the current account and invalidate all access tokens."""
    await revoke_all_sessions(user)


@router.get("/sessions", response_model=list[SessionPublic])
async def list_sessions(ctx: AuthContext = Depends(get_auth_context)) -> list[SessionPublic]:
    sessions = await Session.find(
        Session.user_id == ctx.user.id,
        Session.revoked == False,  # noqa: E712
        Session.expires_at > utcnow(),
    ).to_list()
    return [_to_public(s, ctx.session_id) for s in sessions]


@router.delete("/sessions/{session_id}", status_code=status.HTTP_204_NO_CONTENT)
async def revoke_session(
    session_id: PydanticObjectId, user: User = Depends(get_current_user)
) -> None:
    session = await Session.get(session_id)
    if session is None or session.user_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Session not found")
    if not session.revoked:
        session.revoked = True
        await session.save()
