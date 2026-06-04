from datetime import datetime, timezone

from app.core import captcha
from app.core.config import settings
from app.core.ratelimit import RateLimitInfo, rate_limit_headers


def test_rate_limit_headers():
    info = RateLimitInfo(limit=120, remaining=119, reset=datetime(2030, 1, 1, tzinfo=timezone.utc))
    h = rate_limit_headers(info)
    assert h["X-RateLimit-Limit"] == "120"
    assert h["X-RateLimit-Remaining"] == "119"
    assert h["X-RateLimit-Reset"] == str(int(info.reset.timestamp()))


def test_captcha_provider_urls():
    assert set(captcha._VERIFY_URLS) == {"hcaptcha", "turnstile"}


async def test_captcha_skips_when_unconfigured(monkeypatch):
    monkeypatch.setattr(settings, "captcha_secret", None)
    monkeypatch.setattr(settings, "captcha_sitekey", None)
    assert await captcha.verify_captcha("anything") is True
