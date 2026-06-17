from beanie import init_beanie
from pymongo import AsyncMongoClient
from pymongo.asynchronous.database import AsyncDatabase
from pymongo.errors import OperationFailure

from app.admin.ingest_log import IngestLogEntry
from app.admin.runtime_config import RuntimeConfig
from app.auth.models import Session, User
from app.bot.models import Club, GuildConfig, TrackedAnnouncement
from app.core.config import settings
from app.core.email_outbox import OutboxEmail
from app.giveaways.models import Giveaway, GiveawayEntry, PrizeCode, VaultItem
from app.site_auth.models import SiteSession, SiteUser
from app.supporters.models import Supporter
from app.tokens.models import ApiToken

# NOTE: the leaderboards domain (boards/entries/players/activity/cheaters/deltas)
# AND the codexes (codex_entry) live in PostgreSQL now (app/trove/*/pg_store.py),
# NOT Beanie - there are deliberately no Documents for them to register here.
from app.trove.market.models import MarketInterestItem
from app.trove.models import (
    BttChangelog,
    BttRelease,
    ChallengeCapture,
    ChaosChestCapture,
    DelveRotation,
    FeedbackEntry,
    FeedCache,
    TroveEvent,
    TroveNews,
    TroveStatusEvent,
)
from app.trove.updates.models import (
    UpdateBranch,
    UpdateChange,
    UpdateManifestEntry,
    UpdateState,
    UpdateVersion,
)
from app.usage.models import UsageEvent

# Every Beanie Document must be registered here so init_beanie can bind it.
# Models live in their feature packages; this is the one place that aggregates them.
DOCUMENT_MODELS = [
    User, Session, SiteUser, SiteSession,
    ApiToken, UsageEvent, OutboxEmail, TroveNews, FeedCache, TroveEvent,
    DelveRotation, BttRelease, BttChangelog,
    UpdateBranch, UpdateVersion, UpdateChange, UpdateState, UpdateManifestEntry,
    MarketInterestItem,                  # MarketListing + CodexEntry moved to Postgres (pg_store)
    ChaosChestCapture, ChallengeCapture,
    FeedbackEntry,
    TroveStatusEvent,
    RuntimeConfig,
    IngestLogEntry,
    VaultItem, Giveaway, GiveawayEntry, PrizeCode,
    Supporter,
    GuildConfig,
    TrackedAnnouncement,
    Club,
]

# Beanie 2.x uses PyMongo's native async client (Motor is no longer used).
_client: AsyncMongoClient | None = None
_db: AsyncDatabase | None = None

# Collection backing the rate limiter - managed directly (not a Beanie Document)
# so we can do atomic upserts with $inc.
RATE_LIMIT_COLLECTION = "rate_limit_buckets"


async def init_db() -> None:
    """Open the Mongo connection, bind Beanie models, ensure extra indexes."""
    global _client, _db
    # tz_aware=True is mandatory - without it, PyMongo decodes BSON datetimes as
    # NAIVE UTC, and comparing them against `utcnow()` (tz-aware) anywhere in the
    # codebase raises TypeError. The auth path's `token.expires_at < utcnow()`
    # tripped this and 500'd every token-bearing request that had an expiry set.
    _client = AsyncMongoClient(settings.mongo_uri, tz_aware=True)
    _db = _client[settings.mongo_db]
    await init_beanie(database=_db, document_models=DOCUMENT_MODELS)

    # TTL index: Mongo auto-removes a bucket once its window has elapsed.
    await _db[RATE_LIMIT_COLLECTION].create_index("expires_at", expireAfterSeconds=0)

    # Usage events expire after the configured retention window. Managed here
    # (not on the model) so the window can be changed without an index conflict.
    await _ensure_ttl_index(
        _db["usage_events"], "created_at", settings.usage_retention_days * 86400
    )

    # Outbox records (sent/bounced/failed/abandoned) are a transient log - expire
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
        # Index exists with a different TTL - adjust it in place.
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
