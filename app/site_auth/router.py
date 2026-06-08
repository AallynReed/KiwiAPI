"""Site-side accounts router — public-facing user system.

Mounted at ``/v1/site-auth/*`` on the API host and (via thin proxies in
``app/site/router.py``) at ``/site/auth/*`` for same-origin calls from
trove.aallyn.net. Mirrors ``app/auth/router.py`` patterns: argon2
hashing, rate-limit + per-account lockout on login, JWT access tokens
+ rotated refresh tokens, email verification before privileged actions.

What's deliberately NOT here (yet):
  - captcha gates — added if abuse surfaces
  - GitHub OAuth — the dev portal owns that flow; site users get
    email/password only for v1
  - forgot/reset password — follow-up turn; signup + verify is the
    minimum surface area to ship usefully
"""
import jwt
from beanie import PydanticObjectId
from fastapi import APIRouter, BackgroundTasks, Depends, Request, status
from fastapi.responses import HTMLResponse

from app.auth.disposable import is_disposable_email
from app.core import lockout
from app.core.captcha import verify_captcha
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.passwords import ensure_password_not_breached
from app.core.ratelimit import check_rate_limit
from app.core.security import (
    decode_email_token,
    hash_password,
    password_fingerprint,
    verify_password,
)
from app.core.utils import client_ip, utcnow

from app.site_auth.emails import (
    SITE_RESET_PURPOSE,
    SITE_VERIFY_PURPOSE,
    send_password_changed_email,
    send_password_reset_email,
    send_verification_email,
)
from app.site_auth.dependencies import (
    get_current_site_user,
    get_current_verified_site_user,
)
from app.site_auth.models import SiteSession, SiteUser
from app.site_auth.schemas import (
    SiteChangePasswordRequest,
    SiteClaimTroveNameRequest,
    SiteForgotPasswordRequest,
    SiteLoginRequest,
    SiteLogoutRequest,
    SiteMessageResponse,
    SiteRefreshRequest,
    SiteResendVerificationRequest,
    SiteResetPasswordRequest,
    SiteSignupRequest,
    SiteTokenResponse,
    SiteUpdateProfileRequest,
    SiteUserPublic,
    SiteVerifyTroveClaimResponse,
    _validate_username,
)
from app.site_auth.sessions import (
    issue_tokens,
    revoke_all_sessions,
    revoke_by_refresh_token,
    rotate,
)


router = APIRouter(prefix="/v1/site-auth", tags=["site-auth"])


# Pinned at the top so a quick scan finds it — every "we sent you a
# link" message ends with this so a user can rescue the mail from spam.
EMAIL_SPAM_NOTICE = (
    "Don't see it? Check your spam folder and mark it 'Not spam' so "
    "future emails reach your inbox."
)


def _to_public(user: SiteUser) -> SiteUserPublic:
    return SiteUserPublic(
        id=str(user.id),
        username=user.username,
        email=user.email,
        display_name=user.display_name,
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
    """Snapshot every (board, score) currently captured for this
    player name. Used at claim time to anchor the "did any score go
    up?" check the user passes to verify ownership.

    Pulls from the standard ``player_history`` helper so the lookup is
    case-insensitive — same convention the public ``/v1/leaderboards``
    endpoints use. Returns ``{board_uuid_str: score}``; keys are
    stringified because Mongo's BSON disallows numeric document keys.
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
        # Keep the BEST score per board (highest seen during the
        # baseline scan) — if we kept the most-recent we'd capture
        # whatever the latest anchor stored, which can be lower than
        # the player's actual peak. Verification compares against the
        # baseline; a higher current value than the baseline implies
        # progress, so anchoring on the peak is the strictest gate.
        prev = out.get(key)
        if prev is None or score > prev:
            out[key] = float(score)
    return out


def _html_page(title: str, body: str, status_code: int = 200) -> HTMLResponse:
    """Tiny inline HTML page used for verify-email landings. Same style
    as the dev portal's so the brand reads consistently across both
    flows."""
    return HTMLResponse(
        f"<!doctype html><html><head><meta charset='utf-8'><title>{title}</title>"
        "<style>body{font-family:system-ui,sans-serif;background:#0a0e14;color:#e8ecf3;"
        "display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}"
        ".card{max-width:480px;padding:32px;border:1px solid rgba(255,255,255,.10);"
        "border-radius:14px;background:rgba(255,255,255,.03)}"
        "h1{font-size:1.5rem;margin:0 0 12px;font-family:'Space Grotesk',sans-serif}"
        "a{color:#4cc9f0}p{line-height:1.55}</style></head><body>"
        f"<div class='card'><h1>{title}</h1>{body}</div></body></html>",
        status_code=status_code,
    )


# --- Signup / login --------------------------------------------------------

@router.post("/signup", response_model=SiteUserPublic, status_code=status.HTTP_201_CREATED)
async def signup(
    payload: SiteSignupRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> SiteUserPublic:
    """Create a new site account. Open signup — anyone can register.
    Email verification is required before privileged actions
    (currently: claiming a Trove name)."""
    ip = client_ip(request) or "unknown"

    # Reuse the dev-portal signup-rate-limit tunable so a single knob
    # governs both flows. Easier than carrying two parallel sets of
    # values in the runtime config.
    from app.admin import runtime_config
    sup_max, sup_window = await runtime_config.get_rate_limit("signup_rate_limit")
    await check_rate_limit(f"site-signup:{ip}", sup_max, sup_window)

    # Captcha gate — ``verify_captcha`` is a no-op when the captcha
    # keys aren't configured, so dev environments still work; once
    # ``CAPTCHA_SECRET`` + ``CAPTCHA_SITEKEY`` are set, a missing or
    # wrong token kills the signup with a clean 400.
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(
            status_code=400, code=ErrorCode.captcha_failed,
            message="Captcha verification failed",
        )

    email = payload.email.lower()
    if settings.disposable_email_check and is_disposable_email(email):
        raise APIError(
            status_code=400, code=ErrorCode.disposable_email,
            message="Disposable email addresses aren't allowed",
        )
    # Username is already lowercased + shape-validated by the schema.
    if await SiteUser.find_one(SiteUser.username == payload.username) is not None:
        raise APIError(
            status_code=409, code=ErrorCode.bad_request,
            message="That username is taken",
        )
    if await SiteUser.find_one(SiteUser.email == email) is not None:
        raise APIError(
            status_code=409, code=ErrorCode.email_taken,
            message="An account with this email already exists",
        )
    await ensure_password_not_breached(payload.password)

    user = SiteUser(
        username=payload.username,
        email=email,
        hashed_password=hash_password(payload.password),
        display_name=payload.display_name,
    )
    await user.insert()
    background_tasks.add_task(send_verification_email, user)
    return _to_public(user)


@router.post("/login", response_model=SiteTokenResponse)
async def login(
    payload: SiteLoginRequest, request: Request,
) -> SiteTokenResponse:
    """Login by username OR email + password. The discriminator on the
    identifier is whether it contains an ``@`` — same heuristic most
    UIs use."""
    ip = client_ip(request) or "unknown"
    from app.admin import runtime_config
    log_max, log_window = await runtime_config.get_rate_limit("login_rate_limit")
    await check_rate_limit(f"site-login:{ip}", log_max, log_window)

    # Captcha gate. Same disable-when-unconfigured rule as signup —
    # captcha widget on the page passes its token through and we
    # validate it before touching the password hash so a bot can't
    # use this endpoint as a free password-breach oracle.
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(
            status_code=400, code=ErrorCode.captcha_failed,
            message="Captcha verification failed",
        )

    raw_id = (payload.identifier or "").strip()
    lookup_key: str
    if "@" in raw_id:
        lookup_key = raw_id.lower()
        # Per-account lockout keyed on the email so a brute-force on a
        # known account doesn't get reset by changing the identifier
        # shape between requests.
        locked = await lockout.lock_ttl(f"site:{lookup_key}")
        if locked:
            raise APIError(
                status_code=429, code=ErrorCode.account_locked,
                message="Too many failed attempts. Try again later.",
                headers={"Retry-After": str(locked)},
            )
        user = await SiteUser.find_one(SiteUser.email == lookup_key)
    else:
        # Validate the shape so we can short-circuit obviously-invalid
        # IDs without hitting the DB. Re-uses the same regex as signup.
        try:
            lookup_key = _validate_username(raw_id)
        except ValueError:
            raise APIError(
                status_code=401, code=ErrorCode.invalid_credentials,
                message="Incorrect username or password",
            )
        locked = await lockout.lock_ttl(f"site:{lookup_key}")
        if locked:
            raise APIError(
                status_code=429, code=ErrorCode.account_locked,
                message="Too many failed attempts. Try again later.",
                headers={"Retry-After": str(locked)},
            )
        user = await SiteUser.find_one(SiteUser.username == lookup_key)

    # Constant-time-ish: verify even on missing user so a timing attacker
    # can't enumerate usernames by response latency.
    valid = user is not None and verify_password(payload.password, user.hashed_password)
    if user is None or not valid or not user.is_active:
        await lockout.record_failure(f"site:{lookup_key}")
        raise APIError(
            status_code=401, code=ErrorCode.invalid_credentials,
            message="Incorrect username or password",
        )

    await lockout.clear(f"site:{lookup_key}")
    user.last_login_at = utcnow()
    await user.save()
    return await issue_tokens(user, request)


# --- Tokens / session lifecycle --------------------------------------------

@router.post("/refresh", response_model=SiteTokenResponse)
async def refresh(payload: SiteRefreshRequest, request: Request) -> SiteTokenResponse:
    """Rotate a refresh token. Single-use — the old token is dead after
    this returns. Same rotation pattern as the dev portal."""
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
    """v1 profile editor — just the display name. Username + email are
    immutable in this turn; change-email lives in a follow-up."""
    if payload.display_name is not None:
        user.display_name = payload.display_name.strip() or None
        user.updated_at = utcnow()
        await user.save()
    return _to_public(user)


@router.post("/me/claim-trove-name", response_model=SiteUserPublic)
async def claim_trove_name(
    payload: SiteClaimTroveNameRequest,
    user: SiteUser = Depends(get_current_verified_site_user),
) -> SiteUserPublic:
    """Self-claim an in-game Trove player name. Email verification is
    required so a single throwaway address can't squat on every popular
    name in the database. v1 is self-attest — anybody can claim any
    name not already taken. Uniqueness enforced by a partial index on
    the model so two concurrent claims can't both succeed."""
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
    # Reset verification — a fresh claim starts unverified, with a new
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
    """Release the previously-claimed Trove name (frees it up for
    someone else if the user changes character or moves on)."""
    user.claimed_trove_name = None
    user.claimed_trove_display = None
    user.claimed_at = None
    user.claim_verified = False
    user.claim_verified_at = None
    user.claim_baseline = {}
    user.updated_at = utcnow()
    await user.save()
    return _to_public(user)


@router.post(
    "/me/verify-trove-claim",
    response_model=SiteVerifyTroveClaimResponse,
)
async def verify_trove_claim(
    user: SiteUser = Depends(get_current_verified_site_user),
) -> SiteVerifyTroveClaimResponse:
    """On-demand verification check. Re-fetches the user's current
    leaderboard appearances by the claimed name and compares to the
    baseline captured at claim time. If ANY board's current score is
    higher than the baseline, the claim is marked verified and a
    detail string explains which board provided the proof.

    The check is intentionally simple in v1: any forward progress on
    any board proves "this account belongs to a player who has
    control over the Trove account using this name." Future tightening
    (require N points of progress, require it on a specific board,
    require activity within a time window) can layer on top without
    changing the storage shape.
    """
    if not user.claimed_trove_name:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message="No Trove name claimed — claim one first.",
        )
    if user.claim_verified:
        return SiteVerifyTroveClaimResponse(
            verified=True, detail="Already verified.", user=_to_public(user),
        )

    baseline = user.claim_baseline or {}
    if not baseline:
        # No baseline rows means the player wasn't on any board at
        # claim time. Re-snapshot and ask them to come back after
        # they've appeared on a board.
        new_baseline = await _build_claim_baseline(user.claimed_trove_display or user.claimed_trove_name)
        user.claim_baseline = new_baseline
        user.updated_at = utcnow()
        await user.save()
        return SiteVerifyTroveClaimResponse(
            verified=False,
            detail=(
                "We didn't have any leaderboard data for that name at "
                "claim time. We've taken a fresh baseline; come back "
                "after you've played a bit and we'll re-check."
            ),
            user=_to_public(user),
        )

    from app.trove.leaderboards import service as lb_service
    rows = await lb_service.player_history(
        user.claimed_trove_display or user.claimed_trove_name, limit=500,
    )
    # current[board_uuid_str] = highest current score across captured anchors
    current: dict[str, float] = {}
    for r in rows:
        uuid = r.get("leaderboard")
        score = r.get("score")
        if uuid is None or score is None:
            continue
        key = str(uuid)
        prev = current.get(key)
        if prev is None or score > prev:
            current[key] = float(score)

    # Look for any board where the current peak strictly exceeds the
    # baseline. We don't require a minimum delta — a single score
    # increment is enough to prove control of the in-game character.
    proof_board: str | None = None
    proof_delta: float = 0.0
    for key, cur in current.items():
        base = baseline.get(key)
        if base is None:
            # New board appearance entirely — counts as forward progress.
            proof_board = key
            proof_delta = cur
            break
        if cur > base:
            proof_board = key
            proof_delta = cur - base
            break

    if proof_board is None:
        return SiteVerifyTroveClaimResponse(
            verified=False,
            detail=(
                "No score progression detected yet. Play a bit and "
                "score on any leaderboard you appeared on at claim "
                "time, then try again."
            ),
            user=_to_public(user),
        )

    user.claim_verified = True
    user.claim_verified_at = utcnow()
    user.updated_at = utcnow()
    await user.save()
    return SiteVerifyTroveClaimResponse(
        verified=True,
        detail=(
            f"Verified — score went up on board #{proof_board} "
            f"(delta ~{proof_delta:g})."
        ),
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


@router.post("/change-password", response_model=SiteTokenResponse)
async def change_password(
    payload: SiteChangePasswordRequest,
    request: Request,
    background_tasks: BackgroundTasks,
    user: SiteUser = Depends(get_current_site_user),
) -> SiteTokenResponse:
    if not verify_password(payload.current_password, user.hashed_password):
        raise APIError(
            status_code=401, code=ErrorCode.invalid_credentials,
            message="Current password is incorrect",
        )
    await ensure_password_not_breached(payload.new_password)
    user.hashed_password = hash_password(payload.new_password)
    # Log out everywhere, then hand the current device a fresh session.
    await revoke_all_sessions(user)
    if settings.security_email_notifications:
        background_tasks.add_task(send_password_changed_email, user)
    return await issue_tokens(user, request)


# --- Email verification ----------------------------------------------------

@router.get("/verify-email", response_class=HTMLResponse, include_in_schema=False)
async def verify_email(token: str) -> HTMLResponse:
    """Land here when the user clicks the link in their verification
    email. Marks ``is_verified=True`` and shows a small landing page
    that points back to /dashboard on the showcase site."""
    try:
        payload = decode_email_token(token, SITE_VERIFY_PURPOSE)
        user = await SiteUser.get(PydanticObjectId(payload["sub"]))
    except (jwt.PyJWTError, KeyError):
        user = None

    if user is None:
        return _html_page(
            "Verification failed",
            "<p>This link is invalid or has expired. Request a new one "
            "from the sign-in page.</p>",
            status_code=400,
        )

    if not user.is_verified:
        user.is_verified = True
        user.updated_at = utcnow()
        await user.save()

    return _html_page(
        "Email verified",
        f"<p>Your email is confirmed. You can now sign in and claim "
        f"your Trove player name.</p>"
        f"<p><a href='https://trove.aallyn.net/dashboard'>Open the dashboard →</a></p>",
    )


@router.post("/resend-verification", response_model=SiteMessageResponse)
async def resend_verification(
    payload: SiteResendVerificationRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> SiteMessageResponse:
    """Enumeration-safe resend. Tight per-IP cap so a spammer can't
    flood the outbox by submitting random addresses."""
    await check_rate_limit(
        f"site-resend:{client_ip(request) or 'unknown'}", 5, 3600,
    )
    user = await SiteUser.find_one(SiteUser.email == payload.email.lower())
    if user is not None and user.is_active and not user.is_verified:
        background_tasks.add_task(send_verification_email, user)
    return SiteMessageResponse(
        message=(
            f"If that account exists and isn't verified, a new link is "
            f"on its way. {EMAIL_SPAM_NOTICE}"
        ),
    )


# --- Password reset --------------------------------------------------------


@router.post("/forgot-password", response_model=SiteMessageResponse)
async def forgot_password(
    payload: SiteForgotPasswordRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> SiteMessageResponse:
    """Send a reset link to the address — IF an account with that email
    exists. The response is always the same shape so a caller can't
    enumerate registered emails by probing for status differences."""
    from app.admin import runtime_config
    ip = client_ip(request) or "unknown"
    fp_max, fp_window = await runtime_config.get_rate_limit("forgot_password_rate_limit")
    await check_rate_limit(f"site-forgot:{ip}", fp_max, fp_window)

    # Captcha gate — same disable-when-unconfigured rule. Keeps a
    # spammer from using the forgot-password endpoint to flood the
    # outbox with reset emails to random addresses (the response is
    # enumeration-safe but the mail-send itself is real I/O).
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(
            status_code=400, code=ErrorCode.captcha_failed,
            message="Captcha verification failed",
        )

    user = await SiteUser.find_one(SiteUser.email == payload.email.lower())
    if user is not None and user.is_active:
        background_tasks.add_task(send_password_reset_email, user)
    # Constant response regardless of whether the lookup hit.
    return SiteMessageResponse(
        message=(
            f"If that email is registered, a reset link is on its way. "
            f"{EMAIL_SPAM_NOTICE}"
        ),
    )


@router.post("/reset-password", response_model=SiteMessageResponse)
async def reset_password(payload: SiteResetPasswordRequest) -> SiteMessageResponse:
    """Validate the reset token from the email link and set the new
    password. Single-use: the token embeds a fingerprint of the
    CURRENT password hash, so once the password changes (via this
    very call) any other outstanding reset links for the same account
    immediately stop working."""
    invalid = APIError(
        status_code=400, code=ErrorCode.bad_request,
        message="This reset link is invalid or has expired.",
    )
    try:
        claims = decode_email_token(payload.token, SITE_RESET_PURPOSE)
        user = await SiteUser.get(PydanticObjectId(claims["sub"]))
    except (jwt.PyJWTError, KeyError):
        raise invalid

    # Fingerprint mismatch ⇒ already used OR password changed via another
    # path since the link was issued. Either way, refuse.
    if user is None or claims.get("fp") != password_fingerprint(user.hashed_password):
        raise invalid

    await ensure_password_not_breached(payload.new_password)
    user.hashed_password = hash_password(payload.new_password)
    # End every outstanding session — a reset implies the old password
    # may be compromised, so any device still holding tokens minted on
    # it shouldn't be trusted.
    await revoke_all_sessions(user)
    return SiteMessageResponse(
        message="Your password has been reset. You can now sign in.",
    )


# --- Session listing -------------------------------------------------------

@router.get("/sessions")
async def list_sessions(
    user: SiteUser = Depends(get_current_site_user),
) -> list[dict]:
    """The user's active sessions. Lets them spot a stale device and
    revoke it from the dashboard."""
    sessions = await SiteSession.find(
        SiteSession.site_user_id == user.id,
        SiteSession.revoked == False,  # noqa: E712
        SiteSession.expires_at > utcnow(),
    ).to_list()
    return [
        {
            "id": str(s.id),
            "ip": s.ip,
            "user_agent": s.user_agent,
            "created_at": s.created_at.isoformat(),
            "last_used_at": s.last_used_at.isoformat(),
            "expires_at": s.expires_at.isoformat(),
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
