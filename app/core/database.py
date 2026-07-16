from typing import Any, TypeVar

from beanie import Document, init_beanie
from pymongo import AsyncMongoClient
from pymongo.asynchronous.database import AsyncDatabase
from pymongo.errors import OperationFailure

from app.admin.ingest_log import IngestLogEntry
from app.admin.runtime_config import RuntimeConfig
from app.auth.models import Session, User
from app.bot.models import Club, GuildConfig, TrackedAnnouncement
from app.core.config import settings
from app.core.email_outbox import OutboxEmail
from app.core.utils import utcnow
from app.dm_subs.models import DmSubscription
from app.giveaways.models import Giveaway, GiveawayEntry, PrizeCode, VaultItem
from app.images.models import ImageDesign
from app.pageviews.models import PageView
from app.site_auth.models import SiteSession, SiteUser, UsernameChangeRequest
from app.supporters.models import Supporter
from app.tokens.models import ApiToken

# NOTE: the leaderboards domain (boards/entries/players/activity/cheaters/deltas)
# AND the codexes (codex_entry) live in PostgreSQL now (app/trove/*/pg_store.py),
# NOT Beanie - there are deliberately no Documents for them to register here.
from app.trove.market.models import MarketInterestItem, MarketItemCategory
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
from app.trove.modpacks.models import ModpackProject, ModpackStar
from app.trove.mods_hub.models import (
    ModClaimRequest,
    ModDownloadEvent,
    ModGitToken,
    ModImageAsset,
    ModProfile,
    ModProject,
    ModRelease,
    ModReport,
    ModStar,
    StrayImportState,
)
from app.trove.updates.models import (
    UpdateBranch,
    UpdateChange,
    UpdateManifestEntry,
    UpdateState,
    UpdateVersion,
)
from app.usage.models import UsageEvent
from app.webhooks.models import SiteWebhook

# Every Beanie Document must be registered here so init_beanie can bind it.
# Models live in their feature packages; this is the one place that aggregates them.
DOCUMENT_MODELS = [
    User, Session, SiteUser, SiteSession, UsernameChangeRequest,
    ApiToken, UsageEvent, PageView, OutboxEmail, TroveNews, FeedCache, TroveEvent,
    DelveRotation, BttRelease, BttChangelog,
    UpdateBranch, UpdateVersion, UpdateChange, UpdateState, UpdateManifestEntry,
    ModProject, ModRelease, ModImageAsset, ModReport, ModGitToken, ModStar,  # mods hub (git store holds commits)
    ModDownloadEvent,                    # mods hub: 7-day download signal (TTL-pruned)
    ModProfile,                          # mods hub: modder profile pages
    ModClaimRequest, StrayImportState,   # mods hub: stray (imported) mod claims + import job state
    ModpackProject, ModpackStar,         # modpacks: user-curated bundles of mods (refs only) + likes
    MarketInterestItem,                  # MarketListing + CodexEntry moved to Postgres (pg_store)
    MarketItemCategory,                  # admin-defined /market sidebar groupings (name-keyed)
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
    SiteWebhook,                         # outbound (Discord) webhooks
    DmSubscription,                      # inbound (Discord) DM alert subscriptions
    ImageDesign,                         # user-designed images (image studio)
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
    # The mod_projects (source, source_id) unique index moved from SPARSE to PARTIAL.
    # Sparse still indexed source=null (Beanie writes null, not absent), so the unique
    # index collided on (null,null) and 500'd every normal mod create after the first.
    # The new index has the SAME name, so the old one must be dropped BEFORE Beanie
    # recreates it (a same-name create with different options errors).
    await _drop_index_if_exists(_db["mod_projects"], "source_1_source_id_1")
    await init_beanie(database=_db, document_models=DOCUMENT_MODELS)

    # TTL index: Mongo auto-removes a bucket once its window has elapsed.
    await _db[RATE_LIMIT_COLLECTION].create_index("expires_at", expireAfterSeconds=0)

    # Usage events expire after the configured retention window. Managed here
    # (not on the model) so the window can be changed without an index conflict.
    await _ensure_ttl_index(
        _db["usage_events"], "created_at", settings.usage_retention_days * 86400
    )

    # Site page-view analytics events expire after their own retention window.
    await _ensure_ttl_index(
        _db["page_views"], "created_at", settings.pageview_retention_days * 86400
    )

    # Outbox records (sent/bounced/failed/abandoned) are a transient log - expire
    # them after the retention window so the collection stays small.
    await _ensure_ttl_index(
        _db["email_outbox"], "created_at", settings.email_outbox_retention_days * 86400
    )

    # Mods Hub release tags moved from project-wide unique to per-(project, branch)
    # unique (each branch/variant has its own version timeline). Drop the old
    # project-wide index if a prior deploy created it; the new one is built by Beanie.
    await _drop_index_if_exists(_db["mod_releases"], "project_id_1_tag_1")

    # Mod slugs moved from globally-unique to unique per owner (addressed as
    # /mods/<owner_handle>/<slug>). Drop the old global slug_1 unique index; the
    # new (owner_id, slug) unique index is built by Beanie.
    await _drop_index_if_exists(_db["mod_projects"], "slug_1")

    # Mods Hub download events feed the trailing-7-day "popular" metric; keep only
    # ~8 days so the collection stays tiny (lifetime totals live on download_count).
    await _ensure_ttl_index(_db["mod_download_events"], "created_at", 8 * 86400)


async def _drop_index_if_exists(collection, name: str) -> None:
    """Drop a named index if it's present; a no-op if the index or collection
    doesn't exist yet (fresh database)."""
    try:
        await collection.drop_index(name)
    except OperationFailure:
        pass  # index (or namespace) not found - nothing to drop


async def _ensure_ttl_index(collection, field: str, seconds: int) -> None:
    """Ensure a single-field TTL index on ``field`` with the given expiry.

    If a *conflicting* single-field index already exists - a plain ``{field: 1}``
    declared on a model, or our TTL index with a different expiry - drop it and
    recreate. We deliberately avoid ``collMod`` (used previously to adjust the TTL
    in place) because the app's Mongo user is granted ``readWrite`` but not
    ``dbAdmin``, so ``collMod`` returns "not authorized"; drop+create only needs
    ``readWrite``.
    """
    name = f"{field}_ttl"
    try:
        await collection.create_index(field, name=name, expireAfterSeconds=seconds)
        return
    except OperationFailure:
        pass  # an index on this key already exists with different options
    try:
        info = await collection.index_information()
    except OperationFailure:
        info = {}
    for ix_name, spec in info.items():
        if ix_name == "_id_":
            continue
        key = list(spec.get("key", []))
        if len(key) == 1 and key[0][0] == field:   # any single-field index on `field`
            try:
                await collection.drop_index(ix_name)
            except OperationFailure:
                pass
    await collection.create_index(field, name=name, expireAfterSeconds=seconds)


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


_DocT = TypeVar("_DocT", bound=Document)


async def upsert_by(  # noqa: UP047 - keep classic TypeVar (PEP 695 is 3.12+; matches UP046 policy)
    model: type[_DocT],
    key_field: str,
    value: Any,
    fields: dict[str, Any],
    stamp: str | None = None,
) -> _DocT:
    """Single-key upsert for the relay refreshers: fetch ``model`` where
    ``key_field == value``; insert it (with ``key_field`` + ``fields``) if
    missing, otherwise ``setattr`` each field onto the existing doc; then save.

    ``stamp`` names an optional timestamp attribute set to ``utcnow()`` on the
    UPDATE path only. Callers that also want the stamp written on INSERT put it
    directly in ``fields`` (and typically leave ``stamp`` unset so it isn't
    re-written with a fresh instant on update). Returns the saved doc.
    """
    existing = await model.find_one(getattr(model, key_field) == value)
    if existing is None:
        return await model(**{key_field: value}, **fields).insert()
    for name, field_value in fields.items():
        setattr(existing, name, field_value)
    if stamp is not None:
        setattr(existing, stamp, utcnow())
    await existing.save()
    return existing
