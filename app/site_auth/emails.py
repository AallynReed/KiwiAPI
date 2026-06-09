"""Email sends for the site-side accounts.

Reuses the dev-portal outbox + render helpers but routes the verify /
reset links to the site-auth surface (``/v1/site-auth/verify-email``,
``/v1/site-auth/reset-password``) so a click on a verification mail
lands on the right account namespace.
"""
from datetime import timedelta

from app.core.config import settings
from app.core.email_outbox import queue_email
from app.core.email_render import render_email
from app.core.security import create_email_token, password_fingerprint
from app.site_auth.models import SiteUser


SITE_VERIFY_PURPOSE = "site_email_verify"
SITE_RESET_PURPOSE = "site_password_reset"


def verification_link(user: SiteUser) -> str:
    token = create_email_token(
        str(user.id),
        SITE_VERIFY_PURPOSE,
        timedelta(hours=settings.email_verification_expire_hours),
    )
    # The verify-email endpoint lives on the API host (it's a small HTML
    # landing the FastAPI app serves). Keep linking there directly so a
    # click in the email immediately marks the account verified without
    # an extra hop through the showcase site.
    return f"{settings.api_url}/v1/site-auth/verify-email?token={token}"


def reset_link(user: SiteUser) -> str:
    token = create_email_token(
        str(user.id),
        SITE_RESET_PURPOSE,
        timedelta(hours=settings.password_reset_expire_hours),
        extra={"fp": password_fingerprint(user.hashed_password)},
    )
    # Point at the public site's /reset-password page (not the API host)
    # so the user lands on a properly-styled form on the same domain
    # they signed up on. The page POSTs the token + new password back
    # to /v1/site-auth/reset-password.
    return f"{settings.app_url}/reset-password?token={token}"


async def _queue(to: str, subject: str, heading: str, paragraphs: list[str],
                 button: dict | None = None, note: str | None = None) -> None:
    text = "\n\n".join(paragraphs)
    if button:
        text += f"\n\n{button['label']}: {button['url']}"
    if note:
        text += f"\n\n{note}"
    html = render_email(heading, paragraphs, button=button, note=note)
    await queue_email(to, subject, text, html)


async def send_verification_email(user: SiteUser) -> None:
    hours = settings.email_verification_expire_hours
    await _queue(
        user.email,
        f"Verify your {settings.app_name} email",
        "Confirm your email",
        [
            f"Welcome to {settings.app_name}, {user.display_name or user.username}.",
            "Confirm your email address to unlock the dashboard and claim your "
            "in-game Trove player name.",
        ],
        button={"label": "Verify my email", "url": verification_link(user)},
        note=f"This link expires in {hours} hours. If you didn't sign up, ignore this email.",
    )


async def send_password_reset_email(user: SiteUser) -> None:
    hours = settings.password_reset_expire_hours
    await _queue(
        user.email,
        f"Reset your {settings.app_name} password",
        "Reset your password",
        [f"A password reset was requested for your {settings.app_name} account."],
        button={"label": "Set a new password", "url": reset_link(user)},
        note=f"This link expires in {hours} hour(s). If you didn't request this, ignore this email.",
    )


async def send_password_changed_email(user: SiteUser) -> None:
    await _queue(
        user.email,
        f"Your {settings.app_name} password was changed",
        "Password changed",
        ["Your site account password was just changed, and all other sessions "
         "were signed out."],
        note=(
            f"If this wasn't you, secure your {settings.app_name} account "
            "immediately - change your password and review your active sessions."
        ),
    )
