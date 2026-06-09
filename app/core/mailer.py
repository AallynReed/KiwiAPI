import logging
from email.message import EmailMessage
from email.utils import formatdate, make_msgid

import aiosmtplib

from app.core.config import settings

logger = logging.getLogger("kiwi.mailer")


class EmailDeliveryError(Exception):
    """Raised by :func:`deliver_email`. ``permanent`` distinguishes a hard bounce
    (5xx - don't retry) from a transient failure (4xx / connection - retry)."""

    def __init__(self, message: str, *, permanent: bool, code: int | None = None) -> None:
        self.permanent = permanent
        self.code = code
        super().__init__(message)


def _build_message(to: str, subject: str, text_body: str, html_body: str | None) -> EmailMessage:
    message = EmailMessage()
    message["From"] = f"{settings.mail_from_name} <{settings.mail_from}>"
    message["To"] = to
    message["Subject"] = subject
    # Date + Message-ID are required by RFC 5322 and expected by every real mail
    # client. aiosmtplib won't add them and Postfix isn't either, so without these
    # Gmail synthesizes them (SMTPIN_ADDED_MISSING) and docks the spam score. The
    # Message-ID domain matches the From domain for good alignment.
    message["Date"] = formatdate(localtime=True)
    message["Message-ID"] = make_msgid(domain=settings.mail_from.rsplit("@", 1)[-1])
    message.set_content(text_body)
    if html_body:
        message.add_alternative(html_body, subtype="html")
    return message


async def deliver_email(
    to: str,
    subject: str,
    text_body: str,
    html_body: str | None = None,
) -> None:
    """Send one email now, raising EmailDeliveryError (classified) on failure.

    This is the low-level path used by the outbox worker so it can decide whether
    to retry. Callers that don't care about delivery should queue instead.
    """
    if not settings.email_enabled:
        raise EmailDeliveryError("SMTP is not configured", permanent=True)

    message = _build_message(to, subject, text_body, html_body)
    try:
        await aiosmtplib.send(
            message,
            hostname=settings.smtp_host,
            port=settings.smtp_port,
            username=settings.smtp_username or None,
            password=settings.smtp_password or None,
            start_tls=settings.smtp_starttls,
            use_tls=settings.smtp_ssl,
            timeout=15,
        )
    except aiosmtplib.SMTPRecipientsRefused as exc:
        # Every recipient was refused - treat as a hard bounce.
        raise EmailDeliveryError(f"Recipients refused: {exc}", permanent=True) from exc
    except aiosmtplib.SMTPResponseException as exc:
        permanent = 500 <= (exc.code or 0) < 600
        raise EmailDeliveryError(
            f"SMTP {exc.code}: {exc.message}", permanent=permanent, code=exc.code
        ) from exc
    except (aiosmtplib.SMTPException, OSError) as exc:
        # Connection refused, timeout, disconnect, etc. - transient, retry later.
        raise EmailDeliveryError(f"Transient SMTP failure: {exc}", permanent=False) from exc


async def send_email(
    to: str,
    subject: str,
    text_body: str,
    html_body: str | None = None,
) -> bool:
    """Best-effort direct send. Logs (and returns False) instead of raising.

    If SMTP is not configured, the message is logged so local development still
    surfaces verification/reset links. Returns True only on a confirmed send.
    """
    if not settings.email_enabled:
        logger.warning("SMTP not configured - not sending email to %s. Subject: %s", to, subject)
        logger.info("Email body (would send):\n%s", text_body)
        return False
    try:
        await deliver_email(to, subject, text_body, html_body)
        return True
    except EmailDeliveryError:
        logger.exception("Failed to send email to %s", to)
        return False
