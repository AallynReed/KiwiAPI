import logging

import httpx

from app.core.config import settings

logger = logging.getLogger("kiwi.captcha")

# Both providers accept POST form {secret, response, remoteip} and reply
# {"success": bool, ...} — so a single implementation covers both.
_VERIFY_URLS = {
    "hcaptcha": "https://api.hcaptcha.com/siteverify",
    "turnstile": "https://challenges.cloudflare.com/turnstile/v0/siteverify",
}


async def verify_captcha(token: str | None, remote_ip: str | None = None) -> bool:
    """Verify a captcha response token server-side (hCaptcha or Turnstile).

    If no secret is configured the check is skipped (development convenience) —
    a warning is logged so this is never silently relied upon in production.
    """
    # Enforce only when BOTH keys are configured; a half-config (one key set,
    # the other blank) means captcha is simply off, never a guaranteed failure.
    if not (settings.captcha_secret and settings.captcha_sitekey):
        logger.info("Captcha not fully configured (need both keys) — skipping")
        return True

    url = _VERIFY_URLS.get(settings.captcha_provider)
    if url is None:
        logger.error("Unknown captcha provider: %s", settings.captcha_provider)
        return False

    if not token:
        return False

    data = {"secret": settings.captcha_secret, "response": token}
    if remote_ip:
        data["remoteip"] = remote_ip

    try:
        async with httpx.AsyncClient(timeout=10.0) as client:
            response = await client.post(url, data=data)
            result = response.json()
    except (httpx.HTTPError, ValueError):
        logger.exception("Captcha verification request failed")
        return False

    return bool(result.get("success"))
