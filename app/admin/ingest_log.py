"""History log for master-only ingest endpoints.

Lightweight audit trail for the four ``insert``-style endpoints (the bot
or master uses these to push captures into the database). Each row
records:

- which endpoint was hit
- when (server time, tz-aware)
- which user submitted it (id + cached email so the UI can render
  without joining against ``User``)
- a small, endpoint-specific summary dict (boards count, entries count,
  anchor, …) that the portal's Ingest tab surfaces verbatim
- whether the call succeeded; on failure, a short error message

Rows are written from the route handlers themselves - wrapping them
inside a helper keeps the route bodies clean. Kept for 30 days via a
TTL index so the collection doesn't grow unbounded.
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

    endpoint: str             # canonical route path, e.g. "/v1/leaderboards/insert"
    timestamp: datetime       # tz-aware UTC; written via ``utcnow()`` in record()
    user_id: PydanticObjectId
    user_email: str           # cached at write time to avoid join on read
    success: bool
    summary: dict[str, Any]   # endpoint-specific (renders verbatim in UI)
    error: str | None = None  # short human-readable error on failure
    # Auth context: "token" when the bot submitted with its API token,
    # "session" when the master submitted via the portal Ingest tab.
    # Old rows default to "session" for back-compat with pre-migration
    # data.
    auth_via: Literal["token", "session"] = "session"
    # Token name (the human-readable label set at mint time) - only set
    # when auth_via == "token". Lets the UI display "By: bot@aallyn.net
    # (token: trove-bot)" vs the master submitting from the portal.
    token_name: str | None = None

    class Settings:
        name = "ingest_log"
        indexes = [
            # Recent-first listing - the only read pattern.
            IndexModel([("timestamp", DESCENDING)]),
            # Per-endpoint filtering for future "show me all leaderboards
            # ingests" tab variants.
            IndexModel([("endpoint", ASCENDING), ("timestamp", DESCENDING)]),
            # Auto-cleanup after 30 days (30 * 86400 = 2_592_000 s).
            IndexModel([("timestamp", ASCENDING)], expireAfterSeconds=2_592_000),
        ]


async def record(
    *, endpoint: str, user: User, token: ApiToken | None = None,
    summary: dict[str, Any] | None = None,
    success: bool = True, error: str | None = None,
) -> None:
    """Write one log row. Failure to write is logged but never re-raised
    - the caller's primary work (the actual ingest) already succeeded
    and we don't want a logging hiccup to surface as a 500.

    ``token`` distinguishes bot (API token) from master (portal session
    JWT) submissions. Pass ``None`` for session auth; pass the resolved
    ``ApiToken`` for token auth (the human-readable name is cached on
    the log row)."""
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
    """Most-recent rows first. Optional ``endpoint`` filter for per-route
    drill-down."""
    q: dict[str, Any] = {}
    if endpoint:
        q["endpoint"] = endpoint
    return await (
        IngestLogEntry.find(q)
        .sort("-timestamp")
        .limit(max(1, min(limit, 200)))
        .to_list()
    )
