"""Audit trail for the master-only ``insert``-style ingest endpoints.

Email/token name are cached on each row so the Ingest tab renders without
joining ``User``/``ApiToken``. Rows expire after 30 days (TTL index).
"""
from __future__ import annotations

import logging
from datetime import UTC, datetime
from typing import Any, Literal

from beanie import Document, PydanticObjectId
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.auth.models import User
from app.tokens.models import ApiToken

logger = logging.getLogger(__name__)


class IngestLogEntry(Document):
    """One row per call to a master-only ingest endpoint."""

    endpoint: str
    timestamp: datetime
    user_id: PydanticObjectId
    user_email: str
    success: bool
    summary: dict[str, Any]   # endpoint-specific; renders verbatim in the UI
    error: str | None = None
    # "token" = bot submitted with its API token; "session" = master via the
    # portal Ingest tab. Old (pre-migration) rows default to "session".
    auth_via: Literal["token", "session"] = "session"
    # Only set when auth_via == "token" (the token's mint-time label).
    token_name: str | None = None

    class Settings:
        name = "ingest_log"
        indexes = [
            IndexModel([("timestamp", DESCENDING)]),  # recent-first listing
            IndexModel([("endpoint", ASCENDING), ("timestamp", DESCENDING)]),  # per-endpoint filter
            IndexModel([("timestamp", ASCENDING)], expireAfterSeconds=2_592_000),  # 30-day TTL
        ]


async def record(
    *, endpoint: str, user: User, token: ApiToken | None = None,
    summary: dict[str, Any] | None = None,
    success: bool = True, error: str | None = None,
) -> None:
    """Write one log row. Never re-raises: the ingest already succeeded, so a
    logging hiccup must not surface as a 500. Pass ``token`` for token auth
    (its name is cached on the row), ``None`` for portal-session auth."""
    try:
        await IngestLogEntry(
            endpoint=endpoint,
            timestamp=datetime.now(UTC),
            user_id=user.id,
            user_email=user.email,
            success=success,
            summary=summary or {},
            error=error,
            auth_via="token" if token is not None else "session",
            token_name=token.name if token is not None else None,
        ).insert()
    except Exception:
        logger.exception("ingest_log.record: failed to write entry for %s", endpoint)


async def recent(limit: int = 20, endpoint: str | None = None) -> list[IngestLogEntry]:
    """Most-recent rows first, optionally filtered to one ``endpoint``."""
    q: dict[str, Any] = {}
    if endpoint:
        q["endpoint"] = endpoint
    return await (
        IngestLogEntry.find(q)
        .sort("-timestamp")
        .limit(max(1, min(limit, 200)))
        .to_list()
    )
