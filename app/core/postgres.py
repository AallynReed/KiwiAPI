"""Async Postgres pool for the leaderboards domain only.

A SECOND datastore alongside Mongo, scoped to the high-volume, append-only,
time-partitioned leaderboards data (entries / boards / players / activity) where
Mongo's per-document overhead + the small cache were the bottleneck. Everything
else stays in Mongo.

No ORM: raw ``asyncpg`` + ``COPY`` for bulk load. The pool is created at startup
(after the schema is ensured) and used per-operation via ``acquire()``. When
``POSTGRES_DSN`` is unset the pool is ``None`` and the PG-backed leaderboards
features are simply disabled (the app still boots - handy for local dev).
"""
import json
import logging

import asyncpg

from app.core.config import settings

logger = logging.getLogger("kiwi.postgres")

_pool: asyncpg.Pool | None = None


async def _init_connection(con: asyncpg.Connection) -> None:
    """Per-connection setup: encode/decode ``jsonb`` as Python objects so the codex
    ``data`` column round-trips as a dict without manual json.dumps/loads."""
    await con.set_type_codec(
        "jsonb", encoder=json.dumps, decoder=json.loads, schema="pg_catalog",
    )


async def init_postgres() -> None:
    """Create the pool + ensure the schema. No-op (with a warning) if unset."""
    global _pool
    if not settings.postgres_enabled:
        logger.warning("POSTGRES_DSN unset - leaderboards Postgres backend disabled")
        return
    _pool = await asyncpg.create_pool(
        dsn=settings.postgres_dsn,
        min_size=settings.postgres_pool_min,
        max_size=settings.postgres_pool_max,
        command_timeout=120,
        init=_init_connection,
    )
    from app.trove.codexes import pg_schema as codex_pg_schema
    from app.trove.leaderboards import pg_schema
    from app.trove.market import pg_schema as market_pg_schema
    async with _pool.acquire() as con:
        await pg_schema.init(con)
        await market_pg_schema.init(con)
        await codex_pg_schema.init(con)
    logger.info("Postgres pool ready (leaderboards + market + codexes backends)")


async def close_postgres() -> None:
    global _pool
    if _pool is not None:
        await _pool.close()
        _pool = None


def acquire():
    """Acquire a pooled connection (async context manager). Raises if the pool
    isn't configured - leaderboards callers run only when ``postgres_enabled``."""
    if _pool is None:
        raise RuntimeError("Postgres pool not initialized (POSTGRES_DSN unset?)")
    return _pool.acquire()
