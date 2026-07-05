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

Reads delegate to ``pg_store``; the ``market_listing`` indexes (``name``,
``price_each``, ``last_seen``, ``created_at``) cover the common filters.
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
# Read on every market insert + /v1/misc/interest-items hit. Short TTL as a
# safety net; write paths also invalidate immediately so admin edits show on the
# next request, not after the TTL.

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

    # DM watchlist: notify subscribers whose price threshold is met by this
    # ingest. Cheapest price-each per item just seen; never let it break ingest.
    try:
        cheapest: dict[str, float] = {}
        for r in rows:
            name, pe = r[1], float(r[5])
            if name not in cheapest or pe < cheapest[name]:
                cheapest[name] = pe
        if cheapest:
            from app.dm_subs import delivery as dm_delivery
            await dm_delivery.check_market(cheapest)
    except Exception:
        logger.warning("market: DM watchlist check failed", exc_info=True)

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


async def medians_for_names(names: list[str]) -> dict[str, dict]:
    """``name -> {median_each, count}`` for many items at once, across active
    listings. Empty dict when Postgres isn't configured (dev) - callers must
    degrade to "no market data" rather than assuming a price. Powers the crafting
    calculator's ingredient price join."""
    from app.core.config import settings
    if not settings.postgres_enabled or not names:
        return {}
    return await pg_store.medians_for_names(
        list(names), _now() - LISTING_STALE_SECONDS, _now() - LISTING_LIFETIME_SECONDS,
    )


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


# ----- Analytics (the /market Analytics tab) -------------------------------
# Aggregations over the immutable listing history. Everything degrades to empty
# when Postgres isn't configured (dev) rather than raising.

async def analytics_timeline(name: str, *, days: int = 14, bucket_hours: int = 24) -> dict:
    """Daily (by default) price band + supply for one item, plus the merchant-event
    bands that overlap the window so the chart can shade them."""
    from app.core.config import settings
    now = _now()
    floor = now - days * 86400
    points: list[dict] = []
    if settings.postgres_enabled:
        points = await pg_store.price_volume_timeline(
            name, created_at_floor=floor, bucket_seconds=bucket_hours * 3600)
    return {
        "name": name, "days": days, "bucket_hours": bucket_hours,
        "points": points, "events": event_bands(floor, now), "now": now,
    }


async def analytics_deals(
    *, days: int = 7, min_discount: float = 0.25, min_samples: int = 5, limit: int = 50,
) -> list[dict]:
    """Underpriced active listings - a flip-finder. ``min_discount`` is a 0-1
    fraction below the item median; ``days`` bounds how far back a listing's
    creation can be (on top of the active-listing cutoffs)."""
    from app.core.config import settings
    if not settings.postgres_enabled:
        return []
    now = _now()
    created_floor = max(now - days * 86400, now - LISTING_LIFETIME_SECONDS)
    return await pg_store.underpriced_deals(
        last_seen_floor=now - LISTING_STALE_SECONDS, created_at_floor=created_floor,
        min_discount=min_discount, min_samples=min_samples, limit=limit)


async def analytics_movers(
    *, days: int = 7, min_samples: int = 5, limit: int = 40,
) -> dict:
    """Biggest median-price movers: this window's median vs the previous window's,
    split into risers and fallers."""
    from app.core.config import settings
    if not settings.postgres_enabled:
        return {"risers": [], "fallers": [], "days": days}
    now = _now()
    rows = await pg_store.market_movers(
        recent_start=now - days * 86400, prior_start=now - 2 * days * 86400,
        now=now, min_samples=min_samples, limit=limit)
    risers = [r for r in rows if r["change"] > 0]
    fallers = sorted((r for r in rows if r["change"] < 0), key=lambda r: r["change"])
    return {"risers": risers, "fallers": fallers, "days": days, "now": now}


def _cycle_bands(anchor_ts: int, interval_s: int, duration_s: int,
                 start: int, end: int, name: str, kind: str) -> list[dict]:
    """Every occurrence of a periodic event overlapping ``[start, end]`` (real-UTC
    unix). ``anchor_ts`` is the first occurrence's real-UTC start."""
    if interval_s <= 0:
        return []
    k = max(0, (start - duration_s - anchor_ts) // interval_s)
    bands: list[dict] = []
    while True:
        s = anchor_ts + k * interval_s
        if s > end:
            break
        e = s + duration_s
        if e >= start:
            bands.append({"name": name, "kind": kind, "starts_at": int(s), "ends_at": int(e)})
        k += 1
    return bands


def event_bands(start: int, end: int) -> list[dict]:
    """Merchant-cycle windows (Corruxion, Fluxion) overlapping a time range - the
    overlay bands on the analytics price chart. Derived from the authoritative
    anchors in ``server_time`` so they stay in lock-step with the calendar."""
    from app.trove import server_time as st
    corr_anchor = int((st.FIRST_CORRUXION + st.TROVE_OFFSET).timestamp())
    flux_anchor = int((st.FIRST_FLUXION + st.TROVE_OFFSET).timestamp())
    dur = int(st.DRAGON_DURATION.total_seconds())
    bands = _cycle_bands(corr_anchor, int(st.DRAGON_INTERVAL.total_seconds()), dur,
                         start, end, "Corruxion", "merchant")
    bands += _cycle_bands(flux_anchor, int(st.FLUXION_INTERVAL.total_seconds()), dur,
                          start, end, "Fluxion", "merchant")
    return sorted(bands, key=lambda b: b["starts_at"])


# Modified-Z cutoff for the price-history outlier filter. Iglewicz & Hoaglin
# (1993) give |z| > 3.5 as the canonical threshold. In log-price space this
# catches the "1 listing at 50M when median is 20k" shape (~10× deviation given
# a typical MAD of 0.3-0.5).
_PRICE_OUTLIER_Z_THRESHOLD = 3.5
# Below this sample size MAD is too noisy to trust, so we skip filtering.
_PRICE_OUTLIER_MIN_SAMPLES = 5


def _filter_price_outliers(
    points: list[dict],
    z_threshold: float = _PRICE_OUTLIER_Z_THRESHOLD,
) -> tuple[list[dict], list[dict]]:
    """Modified-Z outlier filter on ``log10(price_each)`` -> ``(kept, dropped)``.

    Log-space because market prices are log-normally distributed: a stray 50M
    flux post is a clean ~3.5σ outlier there where it's a 4σ mess on the raw
    scale. Median + MAD are themselves robust to the outliers being flagged, so
    the threshold doesn't drift when one is present.

    Returns ``(points, [])`` unchanged when the sample is too small or MAD is
    degenerate (all prices identical) - showing the raw cloud beats dropping
    every legitimate point on a thin item.
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
    """Per-listing price-vs-time points (one row per matching listing) for the
    price-evolution scatter, capped at ``limit`` rows oldest-first.

    ``include_expired=False`` (default) applies the same active-listing predicate
    as ``list_listings``/``item_summary``: re-seen within ``LISTING_STALE_SECONDS``
    (3h) AND created within ``LISTING_LIFETIME_SECONDS`` (7d in-game TTL).
    ``include_expired=True`` widens to the ``days`` window only, so a chart on an
    item that just rolled over still shows the slope across its last live cycle.
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

    # Filtering defaults ON so one "typed 1,000,000 not 1,000" listing doesn't
    # drag the y-axis through the ceiling; keep_outliers=True shows the raw data.
    # Metadata always reports what was found so the page can surface it either way.
    raw_count = len(points)
    if keep_outliers:
        outliers_excluded = 0
        outliers_min_price: float | None = None
        outliers_max_price: float | None = None
    else:
        kept, dropped = _filter_price_outliers(points)
        points = kept
        outliers_excluded = len(dropped)
        # Surface the excluded price range so the UI can say "Excluded 3 outliers
        # between 41M and 50M flux", not just a bare count.
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
