"""Postgres data-access for the market scope (the ``market_listing`` table).

Raw asyncpg, mirroring the leaderboards ``pg_store`` style. The service layer
(``app/trove/market/service.py``) owns the cutoffs / interest-list filtering and
calls these for the actual DB work. Reads run only in the API process (which has
Postgres); the bot reaches market data over HTTP instead (see service.item_summary).
"""
from __future__ import annotations

from app.core.postgres import acquire

# Columns the listings page is allowed to sort by (whitelist -> injection-safe).
_SORT_COLUMNS = {"last_seen", "price_each", "created_at", "price", "stack", "name"}


def order_by(sort: str | None) -> str:
    """Translate a Beanie-style sort string ('+price_each' / '-last_seen') to a
    safe SQL ``ORDER BY`` expression. Unknown fields fall back to ``last_seen DESC``."""
    s = (sort or "-last_seen").strip()
    desc = s.startswith("-")
    col = s.lstrip("+-")
    if col not in _SORT_COLUMNS:
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


async def reset() -> int:
    """Wipe every listing (returns the prior row count). Used for the cutover from
    the disposable Mongo data, and reusable as a maintenance reset."""
    async with acquire() as con:
        n = await con.fetchval("SELECT count(*) FROM market_listing")
        await con.execute("TRUNCATE market_listing")
    return int(n or 0)
