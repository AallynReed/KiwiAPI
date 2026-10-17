"""Internal service-to-service HTTP (gateway bot -> API).

The bot container has Mongo + Redis but NOT Postgres, so the few Postgres-backed
reads it needs (currently the activity estimate/series for the daily activity
announcement) are fetched from the API over the compose network instead of
querying Postgres directly. Everything else the bot reads in-process from Mongo.

This is the seam to reuse for future bot features that need leaderboard / cheater
data: add an ``internal_get`` call behind a ``settings.postgres_enabled`` guard in
the relevant read-function (see app/trove/leaderboards/activity.py for the pattern).
"""
from __future__ import annotations

import hmac
import logging

import httpx

from app.core.config import settings

logger = logging.getLogger("kiwi.internal_api")


def internal_key_ok(supplied: str) -> bool:
    """Shared-secret check for the ``/internal/*`` endpoints; always False when unset."""
    key = settings.internal_key
    return bool(key) and hmac.compare_digest(supplied, key)


async def internal_get(path: str, params: dict | None = None, *, timeout: float = 8.0,
                       headers: dict | None = None) -> dict | None:
    """GET a JSON endpoint on the internal API (e.g. ``/v1/activity/current``).

    Returns the parsed JSON dict, or ``None`` on any failure - the caller decides
    the fallback (callers here return an empty-but-valid payload so the embed still
    renders). Never raises."""
    url = settings.internal_api_url.rstrip("/") + path
    try:
        async with httpx.AsyncClient(timeout=timeout) as client:
            resp = await client.get(url, params=params, headers=headers)
            resp.raise_for_status()
            return resp.json()
    except Exception:
        logger.warning("internal GET %s failed", path, exc_info=True)
        return None
