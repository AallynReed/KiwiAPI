import hashlib
import logging

import httpx

from app.core.config import settings
from app.core.errors import APIError, ErrorCode

logger = logging.getLogger("kiwi.passwords")

_HIBP_RANGE = "https://api.pwnedpasswords.com/range/"


async def password_breach_count(password: str) -> int:
    """How many known breaches this password appears in (0 if none).

    Uses HaveIBeenPwned k-anonymity: only the first 5 chars of the SHA-1 hash
    leave this server. Fails OPEN (returns 0) if HIBP is unreachable, so an
    outage can't block signups.
    """
    sha1 = hashlib.sha1(password.encode("utf-8")).hexdigest().upper()  # noqa: S324
    prefix, suffix = sha1[:5], sha1[5:]
    try:
        async with httpx.AsyncClient(timeout=5.0) as client:
            # Add-Padding hides the real result-set size from the network.
            resp = await client.get(_HIBP_RANGE + prefix, headers={"Add-Padding": "true"})
            resp.raise_for_status()
            text = resp.text
    except httpx.HTTPError:
        logger.warning("HIBP lookup failed - allowing the password")
        return 0

    for line in text.splitlines():
        h, _, count = line.partition(":")
        if h.strip() == suffix:
            try:
                return int(count.strip())
            except ValueError:
                return 1
    return 0


async def ensure_password_not_breached(password: str) -> None:
    """Raise 400 password_breached if the password is in a known breach."""
    if not settings.password_breach_check:
        return
    count = await password_breach_count(password)
    if count > 0:
        raise APIError(
            status_code=400,
            code=ErrorCode.password_breached,
            message="This password has appeared in known data breaches - please choose a different one.",
            details={"breach_count": count},
        )
