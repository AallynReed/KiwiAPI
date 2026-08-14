"""Postgres data-access for the market scope (the ``market_listing`` table).

Raw asyncpg, mirroring the leaderboards ``pg_store`` style. The service layer
(``app/trove/market/service.py``) owns the cutoffs / interest-list filtering and
calls these for the actual DB work. Reads run only in the API process (which has
Postgres); the bot reaches market data over HTTP instead (see service.item_summary).
"""
from __future__ import annotations

from app.core.postgres import acquire

# Columns the listings page is allowed to sort by. Mapping user token -> the
# literal SQL identifier we emit, so the value spliced into the query is ALWAYS
# one of these constants, never the caller's string (injection-safe by
# construction - the user input only ever selects a key, it is never emitted).
_SORT_COLUMNS = {
    "last_seen": "last_seen",
    "price_each": "price_each",
    "created_at": "created_at",
    "price": "price",
    "stack": "stack",
    "name": "name",
}


def order_by(sort: str | None) -> str:
    """Translate a Beanie-style sort string ('+price_each' / '-last_seen') to a
    safe SQL ``ORDER BY`` expression. Unknown fields fall back to ``last_seen DESC``."""
    s = (sort or "-last_seen").strip()
    desc = s.startswith("-")
    col = _SORT_COLUMNS.get(s.lstrip("+-"))
    if col is None:
        return "last_seen DESC"
    return f"{col} {'DESC' if desc else 'ASC'}"


async def upsert_listings(rows: list[tuple]) -> int:
    """Bulk upsert ``(id, name, type, stack, price, price_each, created_at, last_seen)``.

    On conflict bump ONLY ``last_seen`` - the other fields are immutable per UUID
    (the game issues a new UUID if a real listing's price/stack changes), matching
    the old Mongo ``$set last_seen`` + ``$setOnInsert`` upsert."""
    if not rows:
        return 0
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO market_listing "
            "(id, name, type, stack, price, price_each, created_at, last_seen) "
            "VALUES ($1, $2, $3, $4, $5, $6, $7, $8) "
            "ON CONFLICT (id) DO UPDATE SET last_seen = EXCLUDED.last_seen",
            rows,
        )
    return len(rows)


async def list_listings(
    *, name: str | None, price_min: float | None, price_max: float | None,
    last_seen_floor: int | None, created_at_floor: int | None,
    sort: str, limit: int, offset: int,
) -> tuple[list[dict], int]:
    """Filtered + paged listings, and the matching total count. The service
    computes ``last_seen_floor`` / ``created_at_floor`` from the active-listing
    cutoffs (or leaves them None for an archival read)."""
    conds: list[str] = []
    args: list = []

    def add(expr: str, val) -> None:
        args.append(val)
        conds.append(expr.format(n=len(args)))

    if name is not None:
        add("name = ${n}", name)
    if price_min is not None:
        add("price_each >= ${n}", price_min)
    if price_max is not None:
        add("price_each <= ${n}", price_max)
    if last_seen_floor is not None:
        add("last_seen >= ${n}", last_seen_floor)
    if created_at_floor is not None:
        add("created_at >= ${n}", created_at_floor)

    where = (" WHERE " + " AND ".join(conds)) if conds else ""
    async with acquire() as con:
        total = await con.fetchval(f"SELECT count(*) FROM market_listing{where}", *args)
        rows = await con.fetch(
            "SELECT id, name, type, stack, price, price_each, last_seen, created_at "
            f"FROM market_listing{where} ORDER BY {order_by(sort)} "
            f"LIMIT ${len(args) + 1} OFFSET ${len(args) + 2}",
            *args, limit, offset,
        )
    return [dict(r) for r in rows], int(total or 0)


async def distinct_items() -> list[str]:
    """Item names that currently have at least one stored listing (sorted)."""
    async with acquire() as con:
        rows = await con.fetch("SELECT DISTINCT name FROM market_listing ORDER BY name")
    return [r["name"] for r in rows]


async def item_summary(name: str, last_seen_floor: int, created_at_floor: int) -> dict | None:
    """Cheapest / median (exact, via ``percentile_cont``) / count for one item
    across active listings. ``None`` when no active listings exist for ``name``."""
    async with acquire() as con:
        r = await con.fetchrow(
            "SELECT count(*) AS count, sum(price) AS total_price, sum(stack) AS total_stack, "
            "min(price_each) AS min_each, max(price_each) AS max_each, avg(price_each) AS avg_each, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each "
            "FROM market_listing WHERE name = $1 AND last_seen >= $2 AND created_at >= $3",
            name, last_seen_floor, created_at_floor,
        )
    if r is None or not r["count"]:
        return None
    return {
        "name": name,
        "count": int(r["count"]),
        "total_price": int(r["total_price"] or 0),
        "total_stack": int(r["total_stack"] or 0),
        "min_each": float(r["min_each"]),
        "max_each": float(r["max_each"]),
        "avg_each": round(float(r["avg_each"]), 3) if r["avg_each"] is not None else 0.0,
        "median_each": float(r["median_each"]) if r["median_each"] is not None else 0.0,
    }


async def medians_for_names(
    names: list[str], last_seen_floor: int, created_at_floor: int,
) -> dict[str, dict]:
    """Batch median price-each (exact, ``percentile_cont``) for many item names in
    ONE query - the price join behind the crafting cost calculator. Only names
    with at least one active listing appear in the result; callers treat a missing
    key as "no market data" rather than a zero price."""
    if not names:
        return {}
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT name, count(*) AS count, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each "
            "FROM market_listing "
            "WHERE name = ANY($1) AND last_seen >= $2 AND created_at >= $3 "
            "GROUP BY name",
            names, last_seen_floor, created_at_floor,
        )
    return {
        r["name"]: {"median_each": float(r["median_each"]), "count": int(r["count"])}
        for r in rows if r["median_each"] is not None
    }


async def price_history_rows(
    name: str, *, created_at_floor: int, last_seen_floor: int | None, limit: int,
) -> list[dict]:
    """``(created_at, price_each, last_seen, stack, price)`` rows for one item in a
    window, oldest-first, capped at ``limit`` - the price-evolution scatter source."""
    conds = ["name = $1", "created_at >= $2"]
    args: list = [name, created_at_floor]
    if last_seen_floor is not None:
        args.append(last_seen_floor)
        conds.append(f"last_seen >= ${len(args)}")
    args.append(limit)
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT created_at, price_each, last_seen, stack, price FROM market_listing "
            f"WHERE {' AND '.join(conds)} ORDER BY created_at ASC LIMIT ${len(args)}",
            *args,
        )
    return [dict(r) for r in rows]


# --- analytics aggregations (the /market Analytics tab) ---------------------
# All read over the immutable `market_listing` history; no new tables. Medians
# are exact (`percentile_cont`); "active" = re-seen within 3h AND created within
# the 7-day in-game lifetime (same cutoffs as item_summary).

async def price_volume_timeline(
    name: str, *, created_at_floor: int, bucket_seconds: int,
) -> list[dict]:
    """Per-bucket price band + supply for one item over a window: median / p25 / p75
    price-each, new-listing count, and total stack. Buckets are floored
    ``created_at`` (so ``bucket_seconds=86400`` = daily), oldest-first."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT (created_at / $3) * $3 AS bucket, count(*) AS listings, "
            "sum(stack) AS stack, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS p50, "
            "percentile_cont(0.25) WITHIN GROUP (ORDER BY price_each) AS p25, "
            "percentile_cont(0.75) WITHIN GROUP (ORDER BY price_each) AS p75 "
            "FROM market_listing WHERE name = $1 AND created_at >= $2 "
            "GROUP BY bucket ORDER BY bucket",
            name, created_at_floor, bucket_seconds,
        )
    return [
        {"bucket": int(r["bucket"]), "listings": int(r["listings"]),
         "stack": int(r["stack"] or 0), "p50": float(r["p50"]),
         "p25": float(r["p25"]), "p75": float(r["p75"])}
        for r in rows
    ]


async def daily_series_all(
    *, created_at_floor: int, bucket_seconds: int = 86400,
) -> list[dict]:
    """Daily median / new-listing count / total stack for EVERY item in one pass.

    The anomaly pass needs each item's whole recent history to know what normal
    looks like for it, and it needs every item at once to tell an item-specific
    move apart from the entire market drifting. Per-item queries would be ~300
    round trips for the same rows, so this is one grouped scan instead.
    """
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT name, (created_at / $2) * $2 AS bucket, "
            "count(*) AS listings, sum(stack) AS stack, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS p50, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY stack) AS stack_med, "
            "max(stack) AS stack_max "
            "FROM market_listing WHERE created_at >= $1 "
            "GROUP BY name, bucket ORDER BY name, bucket",
            created_at_floor, bucket_seconds,
        )
    return [
        {"name": r["name"], "bucket": int(r["bucket"]),
         "listings": int(r["listings"]), "stack": int(r["stack"] or 0),
         "p50": float(r["p50"]), "stack_med": float(r["stack_med"] or 0),
         "stack_max": int(r["stack_max"] or 0)}
        for r in rows
    ]


async def underpriced_deals(
    *, last_seen_floor: int, created_at_floor: int,
    min_discount: float, min_samples: int, limit: int,
) -> list[dict]:
    """Active listings priced at least ``min_discount`` (0-1 fraction) below their
    item's median-each, biggest discount first. ``min_samples`` guards against a
    "median" computed from one or two listings."""
    async with acquire() as con:
        rows = await con.fetch(
            "WITH med AS ("
            "  SELECT name, count(*) AS n, "
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each "
            "  FROM market_listing WHERE last_seen >= $1 AND created_at >= $2 "
            "  GROUP BY name HAVING count(*) >= $3) "
            "SELECT l.id, l.name, l.stack, l.price, l.price_each, l.created_at, "
            "  l.last_seen, m.median_each, m.n AS sample_size, "
            "  (1 - l.price_each / m.median_each) AS discount "
            "FROM market_listing l JOIN med m USING (name) "
            "WHERE l.last_seen >= $1 AND l.created_at >= $2 "
            "  AND m.median_each > 0 "
            "  AND l.price_each <= m.median_each * (1 - $4) "
            "ORDER BY discount DESC LIMIT $5",
            last_seen_floor, created_at_floor, min_samples, min_discount, limit,
        )
    return [
        {"id": str(r["id"]), "name": r["name"], "stack": int(r["stack"]),
         "price": int(r["price"]), "price_each": float(r["price_each"]),
         "median_each": float(r["median_each"]), "sample_size": int(r["sample_size"]),
         "discount": round(float(r["discount"]), 4),
         "created_at": int(r["created_at"]), "last_seen": int(r["last_seen"])}
        for r in rows
    ]


# Ranking for ``market_movers``. NOT user input - an internal enum, since the
# value is interpolated straight into the SQL.
#   "abs"  - biggest move either way (the single top mover on the pulse strip)
#   "up"   - risers only, biggest gain first
#   "down" - fallers only, biggest drop first
#
# Risers and fallers MUST be queried separately. A rise is unbounded (+850% is
# routine on a thin Trove item) while a fall is floored at -100%, so ranking a
# single result set by abs(change) lets any item that merely doubled outrank
# EVERY possible faller. Take the top N of that combined ranking and the fallers
# list comes back empty whenever N items rose past +100% - which is most days.
_MOVER_ORDER = {
    "abs": ("", "abs((r.med - p.med) / p.med) DESC"),
    "up": ("AND r.med > p.med", "(r.med - p.med) / p.med DESC"),
    "down": ("AND r.med < p.med", "(r.med - p.med) / p.med ASC"),
}


async def market_movers(
    *, recent_start: int, prior_start: int, now: int,
    min_samples: int, limit: int, direction: str = "abs",
) -> list[dict]:
    """Per-item median-each in the recent window vs the prior window.

    ``direction`` picks the ranking (see ``_MOVER_ORDER``): ``"up"`` / ``"down"``
    return one side only, already ordered biggest-move-first, so each list gets
    its own LIMIT. Both windows require ``min_samples`` listings so a thin item
    can't fake a swing."""
    where_extra, order_by = _MOVER_ORDER[direction]
    async with acquire() as con:
        rows = await con.fetch(
            "WITH recent AS ("
            "  SELECT name, count(*) AS n, "
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS med "
            "  FROM market_listing WHERE created_at >= $1 AND created_at < $3 "
            "  GROUP BY name HAVING count(*) >= $4), "
            "prior AS ("
            "  SELECT name, count(*) AS n, "
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS med "
            "  FROM market_listing WHERE created_at >= $2 AND created_at < $1 "
            "  GROUP BY name HAVING count(*) >= $4) "
            "SELECT r.name, r.med AS recent_med, p.med AS prior_med, r.n AS recent_n, "
            "  (r.med - p.med) / p.med AS change "
            f"FROM recent r JOIN prior p USING (name) WHERE p.med > 0 {where_extra} "
            f"ORDER BY {order_by} LIMIT $5",
            recent_start, prior_start, now, min_samples, limit,
        )
    return [
        {"name": r["name"], "recent_med": float(r["recent_med"]),
         "prior_med": float(r["prior_med"]), "recent_n": int(r["recent_n"]),
         "change": round(float(r["change"]), 4)}
        for r in rows
    ]


async def market_overview(*, stale_floor: int, lifetime_floor: int) -> dict:
    """Snapshot of the live market right now: active-listing count, distinct
    active items, total flux value posted (sum of stack prices), and total units.
    "Active" uses the same cutoffs as ``item_summary`` (re-seen within 3h AND
    created within the 7-day TTL)."""
    async with acquire() as con:
        r = await con.fetchrow(
            "SELECT count(*) AS listings, count(DISTINCT name) AS items, "
            "COALESCE(sum(price), 0) AS total_value, COALESCE(sum(stack), 0) AS total_units "
            "FROM market_listing WHERE last_seen >= $1 AND created_at >= $2",
            stale_floor, lifetime_floor,
        )
    return {
        "active_listings": int(r["listings"] or 0),
        "active_items": int(r["items"] or 0),
        "total_value": int(r["total_value"] or 0),
        "total_units": int(r["total_units"] or 0),
    }


async def volume_leaders(*, window_start: int, limit: int) -> list[dict]:
    """Items with the most NEW listings created in the window (a supply/throughput
    signal), with total units posted, total flux value, and the median price-each.
    Ordered by listing count desc."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT name, count(*) AS listings, "
            "COALESCE(sum(stack), 0) AS units, COALESCE(sum(price), 0) AS total_value, "
            "percentile_cont(0.5) WITHIN GROUP (ORDER BY price_each) AS median_each "
            "FROM market_listing WHERE created_at >= $1 "
            "GROUP BY name ORDER BY listings DESC, units DESC LIMIT $2",
            window_start, limit,
        )
    return [
        {"name": r["name"], "listings": int(r["listings"]),
         "units": int(r["units"] or 0), "total_value": int(r["total_value"] or 0),
         "median_each": float(r["median_each"]) if r["median_each"] is not None else 0.0}
        for r in rows
    ]


async def market_liquidity(
    *, concluded_before: int, window_start: int, expire_lifespan: int,
    min_samples: int, limit: int,
) -> list[dict]:
    """Per-item sell-through, using each listing's lifespan (``last_seen -
    created_at``) as an outcome proxy. A listing is *concluded* once we stop seeing
    it (``last_seen`` older than the stale cutoff = ``concluded_before``). Among
    listings that concluded inside the window (``last_seen >= window_start``):

      - lifespan ``< expire_lifespan`` -> left the market early = **likely sold**
        (or pulled). ``expire_lifespan`` is the 7-day TTL minus a capture-jitter
        margin, since we only scrape hourly.
      - lifespan ``>= expire_lifespan`` -> survived to ~the TTL = **expired unsold**.

    Returns sold / expired / concluded counts, sell-through fraction, and the median
    time-to-sell (seconds) over the sold bucket. ``min_samples`` guards thin items;
    ordered by sell-through desc then volume desc.
    """
    async with acquire() as con:
        rows = await con.fetch(
            "WITH concluded AS ("
            "  SELECT name, (last_seen - created_at) AS lifespan "
            "  FROM market_listing WHERE last_seen < $1 AND last_seen >= $2) "
            "SELECT name, count(*) AS concluded, "
            "  count(*) FILTER (WHERE lifespan < $3) AS sold, "
            "  count(*) FILTER (WHERE lifespan >= $3) AS expired, "
            "  percentile_cont(0.5) WITHIN GROUP (ORDER BY lifespan) "
            "    FILTER (WHERE lifespan < $3) AS median_tts "
            "FROM concluded GROUP BY name HAVING count(*) >= $4 "
            "ORDER BY (count(*) FILTER (WHERE lifespan < $3))::float / count(*) DESC, "
            "         count(*) DESC LIMIT $5",
            concluded_before, window_start, expire_lifespan, min_samples, limit,
        )
    out: list[dict] = []
    for r in rows:
        concluded = int(r["concluded"])
        sold = int(r["sold"])
        out.append({
            "name": r["name"], "concluded": concluded, "sold": sold,
            "expired": int(r["expired"]),
            "sell_through": round(sold / concluded, 4) if concluded else 0.0,
            "median_time_to_sell": (
                int(r["median_tts"]) if r["median_tts"] is not None else None),
        })
    return out


async def reset() -> int:
    """Wipe every listing (returns the prior row count). Used for the cutover from
    the disposable Mongo data, and reusable as a maintenance reset."""
    async with acquire() as con:
        n = await con.fetchval("SELECT count(*) FROM market_listing")
        await con.execute("TRUNCATE market_listing")
    return int(n or 0)
