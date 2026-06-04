from datetime import timedelta

from app.auth.models import User
from app.core.config import settings
from app.core.email_outbox import queue_email
from app.core.email_render import render_email
from app.core.security import create_email_token, password_fingerprint

VERIFY_PURPOSE = "email_verify"
RESET_PURPOSE = "password_reset"
EMAIL_CHANGE_PURPOSE = "email_change"


def verification_link(user: User) -> str:
    token = create_email_token(
        str(user.id),
        VERIFY_PURPOSE,
        timedelta(hours=settings.email_verification_expire_hours),
    )
    return f"{settings.api_url}/auth/verify-email?token={token}"


def reset_link(user: User) -> str:
    token = create_email_token(
        str(user.id),
        RESET_PURPOSE,
        timedelta(hours=settings.password_reset_expire_hours),
        # Fingerprint makes the link single-use (invalid once the password changes).
        extra={"fp": password_fingerprint(user.hashed_password)},
    )
    return f"{settings.api_url}/auth/reset-password?token={token}"


async def _queue(to: str, subject: str, heading: str, paragraphs: list[str],
                 button: dict | None = None, note: str | None = None) -> None:
    """Build the text + branded HTML parts and hand them to the outbox."""
    text = "\n\n".join(paragraphs)
    if button:
        text += f"\n\n{button['label']}: {button['url']}"
    if note:
        text += f"\n\n{note}"
    html = render_email(heading, paragraphs, button=button, note=note)
    await queue_email(to, subject, text, html)


# --- Verification / reset / email-change -----------------------------------

async def send_verification_email(user: User) -> None:
    hours = settings.email_verification_expire_hours
    await _queue(
        user.email,
        f"Verify your {settings.app_name} email",
        "Confirm your email",
        [
            f"Welcome to {settings.app_name}.",
            "Confirm your email address to start creating API tokens.",
        ],
        button={"label": "Verify my email", "url": verification_link(user)},
        note=f"This link expires in {hours} hours. If you didn't sign up, ignore this email.",
    )


async def send_password_reset_email(user: User) -> None:
    hours = settings.password_reset_expire_hours
    await _queue(
        user.email,
        f"Reset your {settings.app_name} password",
        "Reset your password",
        [f"A password reset was requested for your {settings.app_name} account."],
        button={"label": "Set a new password", "url": reset_link(user)},
        note=f"This link expires in {hours} hour(s). If you didn't request this, ignore this email.",
    )


async def send_email_change_verification(user: User, new_email: str) -> None:
    # Token carries the new address; fingerprint of the *current* email makes it
    # single-use (invalid once the email actually changes).
    token = create_email_token(
        str(user.id),
        EMAIL_CHANGE_PURPOSE,
        timedelta(hours=settings.email_verification_expire_hours),
        extra={"new": new_email, "fp": password_fingerprint(user.email)},
    )
    link = f"{settings.api_url}/auth/verify-email-change?token={token}"
    hours = settings.email_verification_expire_hours
    # Sent TO the new address — that's what we're verifying.
    await _queue(
        new_email,
        f"Confirm your new {settings.app_name} email",
        "Confirm your new email",
        [f"Confirm your new email address for {settings.app_name}."],
        button={"label": "Confirm new email", "url": link},
        note=f"This link expires in {hours} hours. If you didn't request this, ignore it.",
    )


# --- Security notices -------------------------------------------------------

async def _security_notice(to: str, subject: str, heading: str, paragraphs: list[str]) -> None:
    await _queue(
        to,
        subject,
        heading,
        paragraphs,
        note=(
            f"If this wasn't you, secure your {settings.app_name} account immediately — "
            "change your password and review your active sessions."
        ),
    )


async def send_new_login_email(user: User, ip: str | None, user_agent: str | None) -> None:
    await _security_notice(
        user.email,
        f"New sign-in to your {settings.app_name} account",
        "New sign-in detected",
        [
            "A new sign-in to your account was detected from a device we hadn't seen before.",
            f"IP address: {ip or 'unknown'}",
            f"Device: {user_agent or 'unknown'}",
        ],
    )


async def send_password_changed_email(user: User) -> None:
    await _security_notice(
        user.email,
        f"Your {settings.app_name} password was changed",
        "Password changed",
        ["Your account password was just changed, and all other sessions were signed out."],
    )


async def send_token_created_email(user: User, token_name: str, prefix: str) -> None:
    await _security_notice(
        user.email,
        f"A new API token was created on your {settings.app_name} account",
        "New API token created",
        [f'A new API token "{token_name}" ({prefix}…) was just created.'],
    )


async def send_token_expiring_email(
    user: User, token_name: str, prefix: str, days_left: int
) -> None:
    when = "today" if days_left <= 0 else f"in {days_left} day(s)"
    await _queue(
        user.email,
        f"Your {settings.app_name} API token expires {when}",
        "API token expiring",
        [
            f'Your {settings.app_name} API token "{token_name}" ({prefix}…) expires {when}.',
            "Once it expires it will stop authenticating requests. Rotate or replace it "
            "from the developer portal.",
        ],
        button={"label": "Open the portal", "url": settings.dev_url},
        note="If you no longer need this token, you can ignore this message.",
    )


async def send_github_linked_email(user: User) -> None:
    await _security_notice(
        user.email,
        f"GitHub was linked to your {settings.app_name} account",
        "GitHub account linked",
        [
            "A GitHub account was just linked to your account and can now be used to sign in.",
        ],
    )


async def send_token_compromised_email(user: User, token_name: str, prefix: str) -> None:
    await _security_notice(
        user.email,
        f"An API token was auto-revoked on your {settings.app_name} account",
        "API token exposed — auto-revoked",
        [
            f'Your API token "{token_name}" ({prefix}…) was found exposed publicly '
            "(detected by secret scanning) and has been automatically revoked.",
            "Create a replacement token and update wherever the old one was used. "
            "Never commit a token to source control.",
        ],
    )


async def send_email_changed_notice_to_old(old_email: str, new_email: str) -> None:
    await _security_notice(
        old_email,
        f"Your {settings.app_name} email address was changed",
        "Email address changed",
        [
            "The email address on your account was changed to a new one.",
            f"New address: {new_email}",
        ],
    )
