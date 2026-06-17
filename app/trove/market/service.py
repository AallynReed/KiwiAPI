"""Write- and read-side helpers for the market scope.

Insert flow:
1. Parse the cfg dump into ``ParsedListing`` tuples.
2. Filter against the interest-items allow-list.
3. Bulk-upsert by ``_id`` (the listing UUID). On first sighting we set every
   field; on a re-scrape we ONLY bump ``last_seen`` so ``created_at`` /
   ``price`` / ``stack`` stay stable (those don't change on a real listing -
   if they did, the game would issue a new UUID).
4. Old listings (>7 days since first sighting, or stale for >3h) are NOT
   deleted - the read endpoints filter them out by default. Keeping them lets
   us serve price-history aggregations on the cold tail without losing data.

Reads are simple Beanie queries; the indexes (``name``, ``price_each``,
``last_seen``, ``created_at``) cover the common filters.
"""

from __future__ import annotations

import logging
import time as time_mod
from datetime import UTC, datetime
from urllib.parse import quote

from beanie import PydanticObjectId

from app.trove.market import pg_store
from app.trove.market.models import (
    LISTING_LIFETIME_SECONDS,
    LISTING_STALE_SECONDS,
    MarketInterestItem,
    load_interest_items_from_file,
)
from app.trove.market.parser import parse_dump

logger = logging.getLogger(__name__)


def _now() -> int:
    return int(datetime.now(UTC).replace(microsecond=0).timestamp())


# ----- Interest-items cache ------------------------------------------------
# Read every market insert + every /v1/misc/interest-items hit. A short TTL
# means admin edits propagate quickly without an explicit cache bust on every
# write path; writes also invalidate immediately so an admin change is visible
# on the next request, not after the TTL expires.

_INTEREST_TTL_SECONDS = 30.0
_interest_cache_value: frozenset[str] | None = None
_interest_cache_expires_at: float = 0.0


def _invalidate_interest_cache() -> None:
    global _interest_cache_value, _interest_cache_expires_at
    _interest_cache_value = None
    _interest_cache_expires_at = 0.0


async def _interest_items_set() -> frozenset[str]:
    """Set of allow-listed item names. DB-backed with a short TTL cache;
    falls back to the seed JSON file if the collection is empty (which only
    happens before the first boot seed completes, or if an admin wiped it)."""
    global _interest_cache_value, _interest_cache_expires_at
    now = time_mod.monotonic()
    if _interest_cache_value is not None and now < _interest_cache_expires_at:
        return _interest_cache_value
    docs = await MarketInterestItem.find().to_list()
    if docs:
        items = frozenset(d.name for d in docs)
    else:
        # Empty collection - seed hasn't run yet OR an admin wiped without a
        # replacement. Use the bundled list so ingest doesn't drop everything.
        items = frozenset(load_interest_items_from_file())
    _interest_cache_value = items
    _interest_cache_expires_at = now + _INTEREST_TTL_SECONDS
    return items


async def interest_items_list() -> list[str]:
    """Sorted list of names for public consumption."""
    return sorted(await _interest_items_set())


# ----- Admin operations on the interest list -------------------------------


async def seed_interest_items_if_empty() -> int:
    """Idempotent first-boot seeder. If the collection has any documents,
    no-op. Otherwise inserts every name from the bundled JSON file with
    ``added_by=None`` (system-seeded). Returns the count inserted."""
    if await MarketInterestItem.count() > 0:
        return 0
    names = load_interest_items_from_file()
    if not names:
        return 0
    await MarketInterestItem.insert_many([
        MarketInterestItem(name=n) for n in names
    ])
    _invalidate_interest_cache()
    logger.info("market: seeded %d interest items from %s",
                len(names), "gamedata/market_items.json")
    return len(names)


async def admin_list_interest_items() -> list[MarketInterestItem]:
    """All items with full metadata, ``name`` ascending. Used by the admin UI."""
    return await MarketInterestItem.find().sort("+name").to_list()


async def admin_add_interest_item(
    name: str, *, added_by: PydanticObjectId | None,
) -> MarketInterestItem:
    """Insert one item. Raises ``ValueError`` on dup (the caller maps to 409)."""
    name = name.strip()
    if not name:
        raise ValueError("name is empty")
    if await MarketInterestItem.find_one(MarketInterestItem.name == name):
        raise ValueError(f"interest item '{name}' already exists")
    doc = MarketInterestItem(name=name, added_by=added_by)
    await doc.insert()
    _invalidate_interest_cache()
    return doc


async def admin_remove_interest_item(name: str) -> bool:
    """Remove one item by name. Returns True if it existed, False otherwise."""
    doc = await MarketInterestItem.find_one(MarketInterestItem.name == name)
    if doc is None:
        return False
    await doc.delete()
    _invalidate_interest_cache()
    return True


async def admin_replace_interest_items(
    names: list[str], *, added_by: PydanticObjectId | None,
) -> dict:
    """Atomic-ish bulk replace: drop everything currently stored, then insert
    the new list. Used for one-shot mass edits (e.g. paste in a fresh list).

    Returns a small summary dict. Not transactional - Mongo replica-set
    transactions are overkill for an admin-only edit on a small collection;
    if the insert fails, the admin re-runs.
    """
    clean = sorted({n.strip() for n in names if n and n.strip()})
    if not clean:
        raise ValueError("names list is empty (refusing to wipe the collection)")
    before = await MarketInterestItem.count()
    await MarketInterestItem.delete_all()
    await MarketInterestItem.insert_many([
        MarketInterestItem(name=n, added_by=added_by) for n in clean
    ])
    _invalidate_interest_cache()
    return {"removed": before, "added": len(clean)}


# --- insert -----------------------------------------------------------------


async def insert_dump(text: str, *, timestamp: int | None = None) -> dict:
    """Parse + upsert a dump. Returns a small summary dict.

    ``timestamp`` overrides the "as-of" anchor used for ``last_seen`` (for
    back-fills); defaults to ``now()``.
    """
    listings = parse_dump(text)
    if not listings:
        logger.warning("market: parsed 0 listings from %d-char dump", len(text))
        return {"parsed": 0, "imported": 0, "ignored_not_in_list": 0, "last_seen": None}

    last_seen = int(timestamp) if (timestamp is not None and timestamp > 0) else _now()
    interest = await _interest_items_set()

    ignored = 0
    rows: list[tuple] = []
    for entry in listings:
        if entry.name not in interest:
            ignored += 1
            continue
        # (id, name, type, stack, price, price_each, created_at, last_seen). The PG
        # upsert bumps only last_seen on conflict, like the old Mongo $setOnInsert.
        rows.append((entry.id, entry.name, entry.type, entry.stack, entry.price,
                     entry.price_each, entry.created_at, last_seen))
    imported = await pg_store.upsert_listings(rows)

    logger.info(
        "market: ingested last_seen=%d parsed=%d imported=%d ignored=%d",
        last_seen, len(listings), imported, ignored,
    )
    return {
        "parsed": len(listings),
        "imported": imported,
        "ignored_not_in_list": ignored,
        "last_seen": last_seen,
    }


# --- read -------------------------------------------------------------------


async def list_listings(
    *,
    name: str | None = None,
    price_min: float | None = None,
    price_max: float | None = None,
    last_seen_after: int | None = None,
    hide_expired: bool = True,
    limit: int = 100,
    offset: int = 0,
    sort: str = "-last_seen",
) -> tuple[list[dict], int]:
    """Paged listings with the usual marketplace filters.

    ``hide_expired`` drops listings older than 7 days OR stale for >3 hours;
    pass ``False`` to include the historical tail (useful for archival reads).
    """
    last_seen_floor = last_seen_after
    created_at_floor = None
    if hide_expired:
        # Active = re-seen within 3h AND created within the 7-day in-game lifetime.
        stale = _now() - LISTING_STALE_SECONDS
        last_seen_floor = max(last_seen_floor, stale) if last_seen_floor is not None else stale
        created_at_floor = _now() - LISTING_LIFETIME_SECONDS
    rows, total = await pg_store.list_listings(
        name=name, price_min=price_min, price_max=price_max,
        last_seen_floor=last_seen_floor, created_at_floor=created_at_floor,
        sort=sort, limit=limit, offset=offset,
    )
    return [_to_dict(r) for r in rows], total


def _to_dict(d: dict) -> dict:
    now = _now()
    created_at, last_seen = d["created_at"], d["last_seen"]
    return {
        "id": str(d["id"]),
        "name": d["name"],
        "type": d["type"],
        "stack": d["stack"],
        "price": d["price"],
        "price_each": d["price_each"],
        "last_seen": last_seen,
        "created_at": created_at,
        "expires_at": created_at + LISTING_LIFETIME_SECONDS,
        "expired": (
            (now - created_at > LISTING_LIFETIME_SECONDS)
            or (now - last_seen > LISTING_STALE_SECONDS)
        ),
    }


async def list_distinct_items() -> list[str]:
    """Item names that currently have at least one stored listing (sorted).

    Pairs with ``interest_items_list()`` (the bot's full allow-list) for a
    "what's actually on the market" vs "what we track" comparison."""
    return await pg_store.distinct_items()


# NOTE: `interest_items_list()` (the DB-backed async version) lives above
# alongside the cache helpers. The old sync file-only helper has been removed;
# callers should `await interest_items_list()`.


async def item_summary(name: str) -> dict | None:
    """Cheapest / median / average / count for one item across active listings.
    ``None`` when no active listings are stored for ``name``.

    The API process queries Postgres directly (exact median via
    ``percentile_cont``). The gateway bot (no Postgres) fetches it over HTTP from
    the same-origin proxy - same shape, so ``/price`` + ``market_watch`` work
    unchanged."""
    from app.core.config import settings
    if not settings.postgres_enabled:
        from app.core.internal_api import internal_get
        return await internal_get(f"/site/market/items/{quote(name, safe='')}/summary")
    return await pg_store.item_summary(
        name, _now() - LISTING_STALE_SECONDS, _now() - LISTING_LIFETIME_SECONDS,
    )


# Default modified-Z cutoff used by the price-history outlier filter.
# Iglewicz & Hoaglin (1993) suggest |z| > 3.5 as the canonical threshold;
# we keep it here so a future tweak doesn't have to chase three call
# sites. In log-price space this catches a ~10× deviation from the
# median given a typical MAD of 0.3-0.5 - which is exactly the
# "1 listing at 50M when median is 20k" shape we're trying to drop.
_PRICE_OUTLIER_Z_THRESHOLD = 3.5
# Minimum sample size before we attempt outlier filtering. Below this,
# MAD is too noisy to be reliable and we'd drop legitimate data.
_PRICE_OUTLIER_MIN_SAMPLES = 5


def _filter_price_outliers(
    points: list[dict],
    z_threshold: float = _PRICE_OUTLIER_Z_THRESHOLD,
) -> tuple[list[dict], list[dict]]:
    """Modified-Z outlier filter on ``log10(price_each)``.

    Returns ``(kept, dropped)``. Operates in log-space because market
    prices are log-normally distributed - a stray 50,000,000 flux post
    looks like a 4-sigma outlier on the raw scale, but in log-space it's
    a clean ~3.5σ that any test recognises. Uses median + MAD because
    both are themselves robust to the outliers we're trying to flag,
    so the threshold doesn't drift when the outlier is present.

    Returns ``(points, [])`` unchanged when the sample is too small or
    the MAD is degenerate (all prices identical) - better to show the
    raw cloud in that case than to risk dropping every legitimate
    point on a thin item.
    """
    import math

    if len(points) < _PRICE_OUTLIER_MIN_SAMPLES:
        return points, []
    log_prices: list[float] = []
    for p in points:
        v = p.get("price_each")
        if v is None or v <= 0:
            continue
        log_prices.append(math.log10(float(v)))
    if len(log_prices) < _PRICE_OUTLIER_MIN_SAMPLES:
        return points, []

    log_prices.sort()
    median = log_prices[len(log_prices) // 2]
    deviations = sorted(abs(x - median) for x in log_prices)
    mad = deviations[len(deviations) // 2]
    if mad < 1e-9:
        # Every price identical (or near enough) - no outliers possible.
        return points, []

    kept: list[dict] = []
    dropped: list[dict] = []
    for p in points:
        v = p.get("price_each")
        # Keep zero/negative-price entries on the "kept" side; they're
        # unusual enough that the caller should see them.
        if v is None or v <= 0:
            kept.append(p)
            continue
        z = 0.6745 * abs(math.log10(float(v)) - median) / mad
        if z > z_threshold:
            dropped.append(p)
        else:
            kept.append(p)
    return kept, dropped


async def price_history(
    name: str,
    *,
    days: int = 7,
    include_expired: bool = False,
    keep_outliers: bool = False,
    limit: int = 5000,
) -> dict:
    """Per-listing price-vs-time points for the price-evolution chart.

    Each point is a (``created_at``, ``price_each``) pair - one row per
    listing that matches the window. The page draws these as a scatter
    so the user can read the cloud directly; we also surface a small
    set of aggregates the client uses to compute axis ranges and the
    median-trend overlay without a second round-trip.

    ``include_expired=False`` (the default) applies the same active-
    listing predicate that ``list_listings``/``item_summary`` use:
      • re-seen within ``LISTING_STALE_SECONDS`` (3h)
      • created within ``LISTING_LIFETIME_SECONDS`` (7d, the in-game TTL)

    ``include_expired=True`` widens the lookup to the user-supplied
    ``days`` window only - so a 7d chart on an item that just rolled
    over still shows the slope across the last live cycle, including
    posts that have since timed out. Caps the result at ``limit`` rows
    (sorted oldest-first) to bound the payload size.
    """
    days = max(1, min(int(days), 30))
    limit = max(1, min(int(limit), 10_000))
    now = _now()
    window_start = now - days * 86_400

    created_at_floor = window_start
    last_seen_floor = None
    if not include_expired:
        # Mirror the summary's active-listing definition: re-seen recently AND
        # within the 7d TTL. Both bounds are stricter than the user's window so
        # this composes cleanly.
        last_seen_floor = now - LISTING_STALE_SECONDS
        ttl_cutoff = now - LISTING_LIFETIME_SECONDS
        if created_at_floor < ttl_cutoff:
            created_at_floor = ttl_cutoff

    rows = await pg_store.price_history_rows(
        name, created_at_floor=created_at_floor,
        last_seen_floor=last_seen_floor, limit=limit,
    )
    # ``last_seen`` makes the client tooltip useful; ``stack`` + ``price`` let it
    # show "12,000 ×8 = 96,000" without a second lookup.
    points: list[dict] = [
        {"created_at": r["created_at"], "price_each": r["price_each"],
         "last_seen": r["last_seen"], "stack": r["stack"], "price": r["price"]}
        for r in rows
    ]

    truncated = len(points) >= limit

    # ── Outlier filtering ──────────────────────────────────────────
    # Defaults to ON because the cloud is much more readable when a
    # lone "I typed 1,000,000 instead of 1,000" listing isn't dragging
    # the y-axis through the ceiling. Caller can pass ``keep_outliers
    # =True`` to see the raw data - the metadata always reports what
    # we found so the page can surface it either way.
    raw_count = len(points)
    if keep_outliers:
        outliers_excluded = 0
        outliers_min_price: float | None = None
        outliers_max_price: float | None = None
    else:
        kept, dropped = _filter_price_outliers(points)
        points = kept
        outliers_excluded = len(dropped)
        # Surface the *price range* of the excluded outliers so the UI
        # can say "Excluded 3 outliers between 41M and 50M flux" - much
        # more useful than a bare count when the user is deciding
        # whether to re-include them.
        if dropped:
            extremes = [
                d["price_each"] for d in dropped
                if d.get("price_each") is not None
            ]
            outliers_min_price = min(extremes) if extremes else None
            outliers_max_price = max(extremes) if extremes else None
        else:
            outliers_min_price = None
            outliers_max_price = None

    return {
        "name": name,
        "days": days,
        "include_expired": include_expired,
        "keep_outliers": keep_outliers,
        "window_start": window_start,
        "window_end": now,
        "points": points,
        "count": len(points),
        "raw_count": raw_count,
        "outliers_excluded": outliers_excluded,
        "outliers_min_price": outliers_min_price,
        "outliers_max_price": outliers_max_price,
        "outliers_threshold_z": _PRICE_OUTLIER_Z_THRESHOLD,
        "truncated": truncated,
    }
