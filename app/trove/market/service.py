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

from beanie import PydanticObjectId
from beanie.odm.bulk import BulkWriter
from pymongo import UpdateOne

from app.trove.market.models import (
    LISTING_LIFETIME_SECONDS,
    LISTING_STALE_SECONDS,
    MarketInterestItem,
    MarketListing,
    load_interest_items_from_file,
)
from app.trove.market.parser import ParsedListing, parse_dump

logger = logging.getLogger(__name__)

_BULK_BATCH = 1000


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

    imported = 0
    ignored = 0
    batch: list[UpdateOne] = []

    coll = MarketListing.get_motor_collection()  # raw collection for batched upsert

    async def flush(ops: list[UpdateOne]) -> None:
        if ops:
            await coll.bulk_write(ops, ordered=False)

    for entry in listings:
        if entry.name not in interest:
            ignored += 1
            continue
        # Upsert by _id: on first insert, write everything; on re-scrape, only
        # bump last_seen. The two-operator form lets one Mongo op cover both.
        batch.append(UpdateOne(
            {"_id": entry.id},
            {
                "$set": {"last_seen": last_seen},
                "$setOnInsert": {
                    "name": entry.name,
                    "type": entry.type,
                    "stack": entry.stack,
                    "price": entry.price,
                    "price_each": entry.price_each,
                    "created_at": entry.created_at,
                },
            },
            upsert=True,
        ))
        imported += 1
        if len(batch) >= _BULK_BATCH:
            await flush(batch)
            batch = []
    await flush(batch)

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
    query: dict = {}
    if name is not None:
        query["name"] = name
    if price_min is not None or price_max is not None:
        pe: dict = {}
        if price_min is not None:
            pe["$gte"] = price_min
        if price_max is not None:
            pe["$lte"] = price_max
        query["price_each"] = pe
    if last_seen_after is not None:
        query["last_seen"] = {"$gte": last_seen_after}
    if hide_expired:
        cutoff_stale = _now() - LISTING_STALE_SECONDS
        cutoff_lifetime = _now() - LISTING_LIFETIME_SECONDS
        # NB: we need BOTH "still being seen" AND "not past lifetime".
        query.setdefault("last_seen", {})
        # If last_seen already constrained above, take the more restrictive bound.
        if "$gte" in query["last_seen"]:
            query["last_seen"]["$gte"] = max(query["last_seen"]["$gte"], cutoff_stale)
        else:
            query["last_seen"]["$gte"] = cutoff_stale
        query["created_at"] = {"$gte": cutoff_lifetime}

    total = await MarketListing.find(query).count()
    docs = (
        await MarketListing.find(query)
        .sort(sort)
        .skip(offset)
        .limit(limit)
        .to_list()
    )
    items = [_to_dict(d) for d in docs]
    return items, total


def _to_dict(d: MarketListing) -> dict:
    now = _now()
    return {
        "id": str(d.id),
        "name": d.name,
        "type": d.type,
        "stack": d.stack,
        "price": d.price,
        "price_each": d.price_each,
        "last_seen": d.last_seen,
        "created_at": d.created_at,
        "expires_at": d.created_at + LISTING_LIFETIME_SECONDS,
        "expired": (
            (now - d.created_at > LISTING_LIFETIME_SECONDS)
            or (now - d.last_seen > LISTING_STALE_SECONDS)
        ),
    }


async def list_distinct_items() -> list[str]:
    """Item names that currently have at least one stored listing.

    Useful for clients building filter dropdowns - pairs with
    ``interest_items()`` (the bot's full allow-list) for an "what's actually on
    the market" vs "what we track" comparison."""
    items = await MarketListing.distinct("name")
    return sorted(items)


# NOTE: `interest_items_list()` (the DB-backed async version) lives above
# alongside the cache helpers. The old sync file-only helper has been removed;
# callers should `await interest_items_list()`.


async def item_summary(name: str) -> dict | None:
    """Aggregate cheapest / median / count for one item across non-expired
    listings. Returns ``None`` if no active listings are stored for ``name``.

    Median is approximated as the middle ``price_each`` after sort - accurate
    on small N, "close enough" on large N where the Mongo planner can't
    easily compute a true median in one pass."""
    cutoff_stale = _now() - LISTING_STALE_SECONDS
    cutoff_lifetime = _now() - LISTING_LIFETIME_SECONDS
    query = {
        "name": name,
        "last_seen": {"$gte": cutoff_stale},
        "created_at": {"$gte": cutoff_lifetime},
    }
    pipeline = [
        {"$match": query},
        {"$sort": {"price_each": 1}},
        {"$group": {
            "_id": None,
            "count": {"$sum": 1},
            "total_price": {"$sum": "$price"},
            "total_stack": {"$sum": "$stack"},
            "min_each": {"$min": "$price_each"},
            "max_each": {"$max": "$price_each"},
            "avg_each": {"$avg": "$price_each"},
            "all_each": {"$push": "$price_each"},
        }},
    ]
    rows = await MarketListing.aggregate(pipeline).to_list()
    if not rows:
        return None
    r = rows[0]
    all_each = r["all_each"]
    median = all_each[len(all_each) // 2] if all_each else 0.0
    return {
        "name": name,
        "count": r["count"],
        "total_price": r["total_price"],
        "total_stack": r["total_stack"],
        "min_each": r["min_each"],
        "max_each": r["max_each"],
        "avg_each": round(r["avg_each"], 3) if r["avg_each"] is not None else 0.0,
        "median_each": median,
    }


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

    query: dict = {"name": name, "created_at": {"$gte": window_start}}
    if not include_expired:
        # Mirror the summary's active-listing definition: drop rows that
        # haven't been re-seen recently OR whose 7d TTL is already up.
        # Both bounds are stricter than the user's window so this AND
        # composes cleanly.
        query["last_seen"] = {"$gte": now - LISTING_STALE_SECONDS}
        # Re-clamp created_at to the live-lifetime cutoff in case the
        # user-supplied window is wider than 7d.
        ttl_cutoff = now - LISTING_LIFETIME_SECONDS
        if query["created_at"]["$gte"] < ttl_cutoff:
            query["created_at"]["$gte"] = ttl_cutoff

    coll = MarketListing.get_pymongo_collection()
    cursor = (
        coll.find(
            query,
            {"_id": 0, "created_at": 1, "price_each": 1, "last_seen": 1,
             "stack": 1, "price": 1},
        )
        .sort("created_at", 1)
        .limit(limit)
    )
    points: list[dict] = []
    async for d in cursor:
        # ``last_seen`` makes the client-side tooltip useful (the user
        # can spot a listing posted ages ago but only just expired).
        # ``stack`` + ``price`` let it show "12,000 ×8 = 96,000" without
        # a second lookup.
        points.append({
            "created_at": d["created_at"],
            "price_each": d["price_each"],
            "last_seen":  d.get("last_seen"),
            "stack":      d.get("stack"),
            "price":      d.get("price"),
        })

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
