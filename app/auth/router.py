import jwt
from beanie import PydanticObjectId
from fastapi import APIRouter, BackgroundTasks, Depends, Request, status
from fastapi.responses import HTMLResponse

from app.auth.disposable import is_disposable_email
from app.auth.emails import (
    EMAIL_CHANGE_PURPOSE,
    RESET_PURPOSE,
    VERIFY_PURPOSE,
    send_email_change_verification,
    send_email_changed_notice_to_old,
    send_new_login_email,
    send_password_changed_email,
    send_password_reset_email,
    send_verification_email,
)
from app.auth.models import Session, User
from app.auth.schemas import (
    ChangeEmailRequest,
    ChangePasswordRequest,
    ForgotPasswordRequest,
    LoginRequest,
    MessageResponse,
    ResendVerificationRequest,
    ResetPasswordRequest,
    SignupRequest,
    TokenResponse,
    UpdateProfileRequest,
    UserPublic,
)
from app.auth.sessions import _user_agent, issue_tokens, revoke_all_sessions
from app.core import lockout
from app.core.captcha import verify_captcha
from app.core.config import settings
from app.core.dependencies import get_current_user
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

router = APIRouter(prefix="/auth", tags=["auth"])


def _html_page(title: str, body: str, status_code: int = 200) -> HTMLResponse:
    return HTMLResponse(
        f"<!doctype html><html><head><meta charset='utf-8'><title>{title}</title>"
        "<style>body{font-family:system-ui,sans-serif;background:#0d1117;color:#e6edf3;"
        "display:flex;min-height:100vh;align-items:center;justify-content:center;margin:0}"
        ".card{max-width:420px;padding:32px;border:1px solid #30363d;border-radius:12px;"
        "background:#161b22}h1{font-size:1.3rem;margin:0 0 12px}a,button{color:#58a6ff}"
        "input,button{font-size:1rem;padding:10px;width:100%;box-sizing:border-box;margin-top:8px;"
        "border-radius:8px;border:1px solid #30363d;background:#0d1117;color:#e6edf3}"
        "button{cursor:pointer;background:#238636;color:#fff;border:0;margin-top:16px}"
        f"</style></head><body><div class='card'><h1>{title}</h1>{body}</div></body></html>",
        status_code=status_code,
    )


def _to_public(user: User) -> UserPublic:
    return UserPublic(
        id=str(user.id),
        email=user.email,
        display_name=user.display_name,
        is_active=user.is_active,
        is_verified=user.is_verified,
        is_superuser=user.is_superuser,
        roles=user.roles,
        created_at=user.created_at,
        last_login_at=user.last_login_at,
    )


@router.post("/signup", response_model=UserPublic, status_code=status.HTTP_201_CREATED)
async def signup(
    payload: SignupRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> UserPublic:
    ip = client_ip(request) or "unknown"

    # Anti-spam: cap signups per IP, then require a valid captcha solution.
    await check_rate_limit(
        f"signup:{ip}",
        settings.signup_rate_limit_max,
        settings.signup_rate_limit_window_seconds,
    )
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(
            status_code=400,
            code=ErrorCode.captcha_failed,
            message="Captcha verification failed",
        )

    email = payload.email.lower()
    if settings.disposable_email_check and is_disposable_email(email):
        raise APIError(
            status_code=400,
            code=ErrorCode.disposable_email,
            message="Disposable email addresses aren't allowed",
        )
    if await User.find_one(User.email == email) is not None:
        raise APIError(
            status_code=409,
            code=ErrorCode.email_taken,
            message="An account with this email already exists",
        )
    await ensure_password_not_breached(payload.password)

    user = User(
        email=email,
        hashed_password=hash_password(payload.password),
        display_name=payload.display_name,
    )
    await user.insert()
    # Send the verification email after the response is returned.
    background_tasks.add_task(send_verification_email, user)
    return _to_public(user)


@router.post("/login", response_model=TokenResponse)
async def login(
    payload: LoginRequest, request: Request, background_tasks: BackgroundTasks
) -> TokenResponse:
    ip = client_ip(request) or "unknown"
    email = payload.email.lower()
    await check_rate_limit(
        f"login:{ip}",
        settings.login_rate_limit_max,
        settings.login_rate_limit_window_seconds,
    )
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(
            status_code=400,
            code=ErrorCode.captcha_failed,
            message="Captcha verification failed",
        )

    # Per-account lockout after repeated failures.
    locked = await lockout.lock_ttl(email)
    if locked:
        raise APIError(
            status_code=429,
            code=ErrorCode.account_locked,
            message="Too many failed attempts. Try again later.",
            headers={"Retry-After": str(locked)},
        )

    user = await User.find_one(User.email == email)
    # Verify even when the user is missing to keep timing roughly constant.
    valid = user is not None and verify_password(payload.password, user.hashed_password)
    if user is None or not valid or not user.is_active:
        await lockout.record_failure(email)
        raise APIError(
            status_code=401,
            code=ErrorCode.invalid_credentials,
            message="Incorrect email or password",
        )

    await lockout.clear(email)

    if settings.require_verified_for_login and not user.is_verified:
        raise APIError(
            status_code=403,
            code=ErrorCode.email_unverified,
            message="Verify your email address before logging in.",
        )

    # Notify on a sign-in from an IP we haven't seen on this account.
    seen_ip = ip and await Session.find_one(Session.user_id == user.id, Session.ip == ip)
    user.last_login_at = utcnow()
    await user.save()

    tokens = await issue_tokens(user, request)
    if settings.security_email_notifications and not seen_ip:
        background_tasks.add_task(send_new_login_email, user, ip, _user_agent(request))
    return tokens


@router.get("/me", response_model=UserPublic)
async def me(user: User = Depends(get_current_user)) -> UserPublic:
    return _to_public(user)


@router.patch("/me", response_model=UserPublic)
async def update_profile(
    payload: UpdateProfileRequest, user: User = Depends(get_current_user)
) -> UserPublic:
    """Update editable profile fields (currently just the display name)."""
    if payload.display_name is not None:
        user.display_name = payload.display_name.strip() or None
        user.updated_at = utcnow()
        await user.save()
    return _to_public(user)


# --- Email verification ----------------------------------------------------

@router.get("/verify-email", response_class=HTMLResponse, include_in_schema=False)
async def verify_email(token: str) -> HTMLResponse:
    try:
        payload = decode_email_token(token, VERIFY_PURPOSE)
        user = await User.get(PydanticObjectId(payload["sub"]))
    except (jwt.PyJWTError, KeyError):
        user = None

    if user is None:
        return _html_page(
            "Verification failed",
            "<p>This link is invalid or has expired. Request a new one from the portal.</p>",
            status_code=400,
        )

    if not user.is_verified:
        user.is_verified = True
        user.updated_at = utcnow()
        await user.save()

    return _html_page(
        "Email verified ✓",
        "<p>Your email is confirmed. You can now create API tokens.</p>",
    )


@router.post("/resend-verification", response_model=MessageResponse)
async def resend_verification(
    payload: ResendVerificationRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> MessageResponse:
    # Unauthenticated + enumeration-safe, so it works from the login screen when
    # verified-email-before-login blocks the user from signing in.
    await check_rate_limit(f"resend:{client_ip(request) or 'unknown'}", 5, 3600)  # 5 / hour / IP
    user = await User.find_one(User.email == payload.email.lower())
    if user is not None and user.is_active and not user.is_verified:
        background_tasks.add_task(send_verification_email, user)
    return MessageResponse(message="If that account exists and isn't verified, a new link is on its way.")


# --- Account management (authenticated) ------------------------------------

@router.post("/change-password", response_model=TokenResponse)
async def change_password(
    payload: ChangePasswordRequest,
    request: Request,
    background_tasks: BackgroundTasks,
    user: User = Depends(get_current_user),
) -> TokenResponse:
    if not verify_password(payload.current_password, user.hashed_password):
        raise APIError(
            status_code=401,
            code=ErrorCode.invalid_credentials,
            message="Current password is incorrect",
        )
    await ensure_password_not_breached(payload.new_password)
    user.hashed_password = hash_password(payload.new_password)
    # Log out everywhere (bumps token_version + revokes sessions), then hand the
    # current device a fresh session so it stays logged in.
    await revoke_all_sessions(user)
    if settings.security_email_notifications:
        background_tasks.add_task(send_password_changed_email, user)
    return await issue_tokens(user, request)


@router.post("/change-email", response_model=MessageResponse)
async def change_email(
    payload: ChangeEmailRequest,
    background_tasks: BackgroundTasks,
    user: User = Depends(get_current_user),
) -> MessageResponse:
    if not verify_password(payload.password, user.hashed_password):
        raise APIError(
            status_code=401,
            code=ErrorCode.invalid_credentials,
            message="Password is incorrect",
        )
    new_email = payload.new_email.lower()
    if new_email == user.email:
        raise APIError(
            status_code=400,
            code=ErrorCode.bad_request,
            message="That is already your email address",
        )
    if await User.find_one(User.email == new_email) is not None:
        raise APIError(
            status_code=409,
            code=ErrorCode.email_taken,
            message="That email is already in use",
        )
    # The change only takes effect once the new address is confirmed.
    background_tasks.add_task(send_email_change_verification, user, new_email)
    return MessageResponse(message=f"A confirmation link was sent to {new_email}.")


@router.get("/verify-email-change", response_class=HTMLResponse, include_in_schema=False)
async def verify_email_change(token: str, background_tasks: BackgroundTasks) -> HTMLResponse:
    claims: dict = {}
    try:
        claims = decode_email_token(token, EMAIL_CHANGE_PURPOSE)
        user = await User.get(PydanticObjectId(claims["sub"]))
    except (jwt.PyJWTError, KeyError):
        user = None

    if user is None or claims.get("fp") != password_fingerprint(user.email):
        return _html_page(
            "Link expired",
            "<p>This confirmation link is invalid or has expired.</p>",
            status_code=400,
        )

    new_email = claims.get("new", "").lower()
    existing = await User.find_one(User.email == new_email) if new_email else None
    if not new_email or (existing is not None and existing.id != user.id):
        return _html_page(
            "Email unavailable",
            "<p>That email address is no longer available.</p>",
            status_code=409,
        )

    old_email = user.email
    user.email = new_email
    user.is_verified = True
    user.email_bounced = False  # fresh address — resume delivery
    # Email change is security-sensitive — end all sessions.
    await revoke_all_sessions(user)
    # Tell the OLD address the change happened (in case it wasn't them).
    if settings.security_email_notifications:
        background_tasks.add_task(send_email_changed_notice_to_old, old_email, new_email)
    return _html_page(
        "Email updated ✓",
        "<p>Your email address has been changed and verified. Please log in again.</p>",
    )


# --- Password reset --------------------------------------------------------

@router.post("/forgot-password", response_model=MessageResponse)
async def forgot_password(
    payload: ForgotPasswordRequest,
    request: Request,
    background_tasks: BackgroundTasks,
) -> MessageResponse:
    ip = client_ip(request) or "unknown"
    await check_rate_limit(
        f"forgot:{ip}",
        settings.forgot_password_rate_limit_max,
        settings.forgot_password_rate_limit_window_seconds,
    )
    if not await verify_captcha(payload.captcha_token, remote_ip=ip):
        raise APIError(status_code=400, code=ErrorCode.captcha_failed, message="Captcha verification failed")
    user = await User.find_one(User.email == payload.email.lower())
    if user is not None and user.is_active:
        background_tasks.add_task(send_password_reset_email, user)
    # Always identical response — never reveal whether an account exists.
    return MessageResponse(message="If that email is registered, a reset link is on its way.")


@router.post("/reset-password", response_model=MessageResponse)
async def reset_password(payload: ResetPasswordRequest) -> MessageResponse:
    invalid = APIError(
        status_code=400,
        code=ErrorCode.bad_request,
        message="This reset link is invalid or has expired.",
    )
    try:
        claims = decode_email_token(payload.token, RESET_PURPOSE)
        user = await User.get(PydanticObjectId(claims["sub"]))
    except (jwt.PyJWTError, KeyError):
        raise invalid

    # Fingerprint mismatch => already used or password since changed.
    if user is None or claims.get("fp") != password_fingerprint(user.hashed_password):
        raise invalid

    await ensure_password_not_breached(payload.new_password)
    user.hashed_password = hash_password(payload.new_password)
    # End every session — a reset implies the old password may be compromised.
    await revoke_all_sessions(user)
    return MessageResponse(message="Your password has been reset. You can now log in.")


@router.get("/reset-password", response_class=HTMLResponse, include_in_schema=False)
async def reset_password_form(token: str) -> HTMLResponse:
    # Minimal self-contained page so a reset link works straight from the email,
    # with no separate frontend: it POSTs JSON to /auth/reset-password.
    safe_token = token.replace("'", "").replace('"', "").replace("<", "")
    return _html_page(
        "Set a new password",
        f"""
        <input id="pw" type="password" placeholder="New password (min 8 chars)" autocomplete="new-password">
        <button onclick="submitReset()">Reset password</button>
        <p id="msg" style="margin-top:12px"></p>
        <script>
        const token = '{safe_token}';
        async function submitReset() {{
          const pw = document.getElementById('pw').value;
          const msg = document.getElementById('msg');
          const r = await fetch('/auth/reset-password', {{
            method: 'POST', headers: {{'Content-Type': 'application/json'}},
            body: JSON.stringify({{token: token, new_password: pw}})
          }});
          const data = await r.json().catch(() => ({{}}));
          msg.textContent = r.ok ? (data.message || 'Password reset.')
            : ((data.error && data.error.message) || 'Reset failed.');
        }}
        </script>
        """,
    )
