from beanie import init_beanie
from pymongo import AsyncMongoClient
from pymongo.asynchronous.database import AsyncDatabase
from pymongo.errors import OperationFailure

from app.auth.models import Session, User
from app.core.config import settings
from app.core.email_outbox import OutboxEmail
from app.tokens.models import ApiToken
from app.trove.models import TroveNews
from app.usage.models import UsageEvent

# Every Beanie Document must be registered here so init_beanie can bind it.
# Models live in their feature packages; this is the one place that aggregates them.
DOCUMENT_MODELS = [User, Session, ApiToken, UsageEvent, OutboxEmail, TroveNews]

# Beanie 2.x uses PyMongo's native async client (Motor is no longer used).
_client: AsyncMongoClient | None = None
_db: AsyncDatabase | None = None

# Collection backing the rate limiter — managed directly (not a Beanie Document)
# so we can do atomic upserts with $inc.
RATE_LIMIT_COLLECTION = "rate_limit_buckets"


async def init_db() -> None:
    """Open the Mongo connection, bind Beanie models, ensure extra indexes."""
    global _client, _db
    _client = AsyncMongoClient(settings.mongo_uri)
    _db = _client[settings.mongo_db]
    await init_beanie(database=_db, document_models=DOCUMENT_MODELS)

    # TTL index: Mongo auto-removes a bucket once its window has elapsed.
    await _db[RATE_LIMIT_COLLECTION].create_index("expires_at", expireAfterSeconds=0)

    # Usage events expire after the configured retention window. Managed here
    # (not on the model) so the window can be changed without an index conflict.
    await _ensure_ttl_index(
        _db["usage_events"], "created_at", settings.usage_retention_days * 86400
    )

    # Outbox records (sent/bounced/failed/abandoned) are a transient log — expire
    # them after the retention window so the collection stays small.
    await _ensure_ttl_index(
        _db["email_outbox"], "created_at", settings.email_outbox_retention_days * 86400
    )


async def _ensure_ttl_index(collection, field: str, seconds: int) -> None:
    """Create a named TTL index, updating expireAfterSeconds if it already exists."""
    name = f"{field}_ttl"
    try:
        await collection.create_index(field, name=name, expireAfterSeconds=seconds)
    except OperationFailure:
        # Index exists with a different TTL — adjust it in place.
        await collection.database.command(
            {
                "collMod": collection.name,
                "index": {"name": name, "expireAfterSeconds": seconds},
            }
        )


async def close_db() -> None:
    """Close the Mongo connection on shutdown."""
    global _client, _db
    if _client is not None:
        await _client.close()
        _client = None
        _db = None


def get_db() -> AsyncDatabase:
    if _db is None:
        raise RuntimeError("Database not initialized")
    return _db
