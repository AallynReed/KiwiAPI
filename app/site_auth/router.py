"""Site-side accounts router - public-facing user system.

Mounted at ``/v1/site-auth/*`` on the API host. Sign-in is Discord-only
(see ``app/site_auth/oauth.py``); this router owns what happens AFTER a
session exists: JWT access tokens + rotated refresh tokens, the profile /
session-management surface, and the Trove-name claim + verification flow.

There is no password login here - accounts are created and identified
solely through "Sign in with Discord". The dev portal (``app/auth/*``) is
a separate system and keeps its own email/password + GitHub flows.
"""
from beanie import PydanticObjectId
from fastapi import APIRouter, Depends, Request, status
from pydantic import EmailStr, TypeAdapter

from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, utcnow
from app.site_auth.dependencies import (
    get_current_site_user,
    get_current_verified_site_user,
)
from app.site_auth.models import SiteSession, SiteUser
from app.site_auth.schemas import (
    SiteClaimTroveNameRequest,
    SiteLogoutRequest,
    SiteRefreshRequest,
    SiteTokenResponse,
    SiteUpdateProfileRequest,
    SiteUsernameRequestBody,
    SiteUserPublic,
    SiteVerifyTroveClaimResponse,
)
from app.site_auth.sessions import (
    revoke_all_sessions,
    revoke_by_refresh_token,
    rotate,
)
from app.site_auth.usernames import (
    cancel_pending,
    latest_request,
    request_change,
    username_request_dto,
)

router = APIRouter(prefix="/v1/site-auth", tags=["site-auth"])


_DISCORD_CDN = "https://cdn.discordapp.com"

# Validate the opt-in notification email with the same engine as EmailStr, without
# forcing the model field to be EmailStr (so it can be cleared/omitted cleanly).
_EMAIL_ADAPTER = TypeAdapter(EmailStr)


def _validate_email(value: str) -> str:
    """Return the normalized address, or raise ValueError if it isn't a valid email."""
    return str(_EMAIL_ADAPTER.validate_python(value)).lower()


def _discord_avatar_url(user: SiteUser) -> str | None:
    """Build a Discord CDN avatar URL from the stored hash. Falls back to the
    account's default Discord embed avatar when there's no custom one, so the
    UI always has a picture to show. ``None`` only if no Discord id is linked."""
    if user.discord_id is None:
        return None
    if user.discord_avatar:
        ext = "gif" if user.discord_avatar.startswith("a_") else "png"
        return f"{_DISCORD_CDN}/avatars/{user.discord_id}/{user.discord_avatar}.{ext}?size=128"
    # Default avatar - the new (pomelo) username scheme indexes by (id >> 22) % 6.
    return f"{_DISCORD_CDN}/embed/avatars/{(user.discord_id >> 22) % 6}.png"


def _to_public(user: SiteUser) -> SiteUserPublic:
    return SiteUserPublic(
        id=str(user.id),
        username=user.username,
        discord_handle=user.discord_handle or user.username,
        notify_email=user.notify_email,
        display_name=user.display_name,
        avatar_url=_discord_avatar_url(user),
        is_active=user.is_active,
        is_verified=user.is_verified,
        claimed_trove_name=user.claimed_trove_name,
        claimed_trove_display=user.claimed_trove_display,
        claimed_at=user.claimed_at,
        claim_verified=user.claim_verified,
        claim_verified_at=user.claim_verified_at,
        claim_baseline_board_count=len(user.claim_baseline or {}),
        created_at=user.created_at,
        last_login_at=user.last_login_at,
    )


async def _build_claim_baseline(trove_name: str) -> dict[str, float]:
    """Snapshot the best (board, score) currently captured for this player name.

    Returns ``{board_uuid_str: score}``; keys are stringified because Mongo's
    BSON disallows numeric document keys. Since verification became manual, only
    the length is read (dashboard board count).
    """
    from app.trove.leaderboards import service as lb_service
    rows = await lb_service.player_history(trove_name, limit=500)
    out: dict[str, float] = {}
    for r in rows:
        uuid = r.get("leaderboard")
        score = r.get("score")
        if uuid is None or score is None:
            continue
        key = str(uuid)
        # Keep the best (highest) score per board, not the most-recent anchor.
        prev = out.get(key)
        if prev is None or score > prev:
            out[key] = float(score)
    return out


# --- Tokens / session lifecycle --------------------------------------------

@router.post("/refresh", response_model=SiteTokenResponse)
async def refresh(payload: SiteRefreshRequest, request: Request) -> SiteTokenResponse:
    """Rotate a refresh token. Single-use - the old token is dead after
    this returns."""
    tokens = await rotate(payload.refresh_token, request)
    if tokens is None:
        raise APIError(
            status_code=401, code=ErrorCode.not_authenticated,
            message="Invalid or expired refresh token",
        )
    return tokens


@router.post("/logout", status_code=status.HTTP_204_NO_CONTENT)
async def logout(payload: SiteLogoutRequest) -> None:
    """End a session by its refresh token (idempotent)."""
    if payload.refresh_token:
        await revoke_by_refresh_token(payload.refresh_token)


@router.post("/logout-all", status_code=status.HTTP_204_NO_CONTENT)
async def logout_all(user: SiteUser = Depends(get_current_site_user)) -> None:
    """End every session for the current account and invalidate every
    outstanding access token by bumping ``token_version``."""
    await revoke_all_sessions(user)


# --- Profile + account management ------------------------------------------

@router.get("/me", response_model=SiteUserPublic)
async def me(user: SiteUser = Depends(get_current_site_user)) -> SiteUserPublic:
    return _to_public(user)


@router.patch("/me", response_model=SiteUserPublic)
async def update_profile(
    payload: SiteUpdateProfileRequest,
    user: SiteUser = Depends(get_current_site_user),
) -> SiteUserPublic:
    """Edit the display name and the opt-in notification email. Username is
    immutable here (changed only via the admin-approved request flow)."""
    dirty = False
    if payload.display_name is not None:
        user.display_name = payload.display_name.strip() or None
        dirty = True
    if payload.notify_email is not None:
        val = payload.notify_email.strip()
        if not val:
            user.notify_email = None            # explicit clear
        else:
            try:
                user.notify_email = _validate_email(val)
            except ValueError as exc:
                raise APIError(
                    400, ErrorCode.bad_request,
                    "That doesn't look like a valid email address.",
                ) from exc
        dirty = True
    if dirty:
        user.updated_at = utcnow()
        await user.save()
    return _to_public(user)


@router.post("/me/claim-trove-name", response_model=SiteUserPublic)
async def claim_trove_name(
    payload: SiteClaimTroveNameRequest,
    user: SiteUser = Depends(get_current_verified_site_user),
) -> SiteUserPublic:
    """Self-claim an in-game Trove player name (self-attest; any unclaimed
    name). Email verification is required so a throwaway address can't squat
    popular names; a partial unique index stops two concurrent claims."""
    name = payload.trove_name.strip()
    if not name:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="Trove name can't be empty",
        )
    lowered = name.lower()

    # Is this name already taken by ANOTHER account?
    existing = await SiteUser.find_one(SiteUser.claimed_trove_name == lowered)
    if existing is not None and existing.id != user.id:
        raise APIError(
            status_code=409, code=ErrorCode.bad_request,
            message="That Trove name is already claimed.",
        )

    user.claimed_trove_name = lowered
    user.claimed_trove_display = name
    user.claimed_at = utcnow()
    # Reset verification - a fresh claim starts unverified, with a new
    # baseline anchored on the current leaderboard snapshot.
    user.claim_verified = False
    user.claim_verified_at = None
    user.claim_baseline = await _build_claim_baseline(name)
    user.updated_at = utcnow()
    await user.save()
    return _to_public(user)


@router.delete("/me/claim-trove-name", response_model=SiteUserPublic)
async def unclaim_trove_name(
    user: SiteUser = Depends(get_current_site_user),
) -> SiteUserPublic:
    """Release the claimed Trove name (frees it for another user)."""
    user.claimed_trove_name = None
    user.claimed_trove_display = None
    user.claimed_at = None
    user.claim_verified = False
    user.claim_verified_at = None
    user.claim_baseline = {}
    user.updated_at = utcnow()
    await user.save()
    return _to_public(user)


@router.get("/me/username-request")
async def my_username_request(user: SiteUser = Depends(get_current_site_user)) -> dict:
    """The user's latest Trove-username change request (pending → 'awaiting review';
    rejected → carries the denial reason). ``None`` if they've never requested one."""
    req = await latest_request(user)
    return {"request": username_request_dto(req) if req else None}


@router.post("/me/username-request")
async def request_username(
    payload: SiteUsernameRequestBody, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Request to change your frozen Trove username (the handle used for your mods).
    A moderator reviews it; nothing changes until they approve."""
    req = await request_change(user, payload.username)
    return {"request": username_request_dto(req)}


@router.delete("/me/username-request")
async def cancel_username_request(user: SiteUser = Depends(get_current_site_user)) -> dict:
    """Withdraw a pending username-change request."""
    await cancel_pending(user)
    return {"ok": True}


@router.post(
    "/me/verify-trove-claim",
    response_model=SiteVerifyTroveClaimResponse,
)
async def verify_trove_claim(
    user: SiteUser = Depends(get_current_verified_site_user),
) -> SiteVerifyTroveClaimResponse:
    """Report claim status. Verification is a MANUAL master approval
    (dev-portal admin panel, POST /admin/site-claims/{id}/approve); the old
    score-progression self-check was retired, so this only ever echoes
    "pending review" until a master approves."""
    if not user.claimed_trove_name:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="No Trove name claimed - claim one first.",
        )
    if user.claim_verified:
        return SiteVerifyTroveClaimResponse(
            verified=True, detail="Already verified.", user=_to_public(user),
        )

    return SiteVerifyTroveClaimResponse(
        verified=False,
        detail="Your claim is pending manual review by an admin.",
        user=_to_public(user),
    )


@router.get("/me/trove-stats")
async def my_trove_stats(
    user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Current leaderboard appearances + score-vs-time chart for the
    user's claimed Trove name. Returns an empty payload when no name
    is claimed so the dashboard can render a clean 'claim a name'
    prompt."""
    if not user.claimed_trove_name:
        return {"claimed": False, "items": [], "series": None}
    from app.trove.leaderboards import service as lb_service
    history = await lb_service.player_history(
        user.claimed_trove_display or user.claimed_trove_name, limit=50,
    )
    chart = await lb_service.player_history_series(
        user.claimed_trove_display or user.claimed_trove_name, days=7,
    )
    return {
        "claimed": True,
        "player_name": user.claimed_trove_display or user.claimed_trove_name,
        "items": history,
        "series": chart,
    }


# --- Session listing -------------------------------------------------------

@router.get("/sessions")
async def list_sessions(
    user: SiteUser = Depends(get_current_site_user),
) -> list[dict]:
    """The user's active sessions, so they can spot and revoke a stale device."""
    sessions = await SiteSession.find(
        SiteSession.site_user_id == user.id,
        SiteSession.revoked == False,  # noqa: E712
        SiteSession.expires_at > utcnow(),
    ).to_list()
    return [
        {
            "id": str(s.id),
            "device": s.device,
            "created_at": iso(s.created_at),
            "last_used_at": iso(s.last_used_at),
            "expires_at": iso(s.expires_at),
        }
        for s in sessions
    ]


@router.delete("/sessions/{session_id}", status_code=status.HTTP_204_NO_CONTENT)
async def revoke_session(
    session_id: PydanticObjectId,
    user: SiteUser = Depends(get_current_site_user),
) -> None:
    session = await SiteSession.get(session_id)
    if session is None or session.site_user_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Session not found")
    if not session.revoked:
        session.revoked = True
        await session.save()
