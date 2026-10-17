"""Site Analytics reports over ``page_views``.

Visits are built at query time: a visitor's views (same daily visitor hash) belong
to one visit until a 30-minute gap. Nothing about a visit is stored, so there is no
per-visitor record to keep, export or delete.

Unique visitors are distinct visitor hashes. The hash salt rotates every UTC day, so
over a multi-day window this is the sum of daily uniques (the cookieless standard),
not distinct people.
"""
from __future__ import annotations

import asyncio
import time
from collections.abc import Awaitable, Callable
from datetime import datetime, timedelta
from typing import Any

from app.core.utils import utcnow
from app.pageviews.models import PageView

VISIT_GAP_MS = 30 * 60 * 1000
BOUNCE_ENGAGED_S = 10
# Only views reported by the stats script. Older server-counted rows include bots.
TRACKED = {"pid": {"$type": "string"}}
VISIT_FILTERS = ("channel", "source", "device", "browser", "os", "lang", "entry")

_memo: dict[tuple, tuple[float, Any]] = {}
_heavy = asyncio.Semaphore(3)


async def _cached(key: tuple, ttl: float, compute: Callable[[], Awaitable[Any]]) -> Any:
    hit = _memo.get(key)
    if hit and hit[0] > time.monotonic():
        return hit[1]
    value = await compute()
    _memo[key] = (time.monotonic() + ttl, value)
    if len(_memo) > 256:
        now = time.monotonic()
        for k in [k for k, (exp, _) in _memo.items() if exp <= now]:
            del _memo[k]
    return value


async def _aggregate(pipeline: list[dict]) -> list[dict]:
    async with _heavy:
        cursor = await PageView.get_pymongo_collection().aggregate(pipeline, allowDiskUse=True)
        return await cursor.to_list(None)


async def tracking_since() -> datetime | None:
    """When the stats script started reporting (the oldest tracked view)."""
    async def first() -> datetime | None:
        doc = await PageView.get_pymongo_collection().find_one(
            TRACKED, sort=[("created_at", 1)], projection={"created_at": 1})
        return doc["created_at"] if doc else None
    return await _cached(("since",), 300, first)


# --- windows ------------------------------------------------------------------

def window(days: int, now: datetime | None = None) -> tuple[datetime, datetime, str, list[str]]:
    """``(since, until, granularity, bucket labels)`` for the last ``days`` days.

    One day = the last 24 hours, hourly. Longer windows are whole UTC days ending
    today (today partial), daily."""
    now = now or utcnow()
    if days <= 1:
        until = now
        since = (now - timedelta(hours=23)).replace(minute=0, second=0, microsecond=0)
        labels = [(since + timedelta(hours=i)).strftime("%Y-%m-%dT%H:00") for i in range(24)]
        return since, until, "hour", labels
    today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    since = today - timedelta(days=days - 1)
    labels = [(since + timedelta(days=i)).strftime("%Y-%m-%d") for i in range(days)]
    return since, now, "day", labels


def _bucket(field: str, granularity: str) -> dict:
    fmt = "%Y-%m-%dT%H:00" if granularity == "hour" else "%Y-%m-%d"
    return {"$dateToString": {"date": f"${field}", "format": fmt, "timezone": "UTC"}}


# --- visits pipeline ------------------------------------------------------------

def visits_pipeline(since: datetime, until: datetime) -> list[dict]:
    """Tracked views in ``[since, until)`` grouped into visits, one doc per visit."""
    by_time = {"partitionBy": "$visitor_hash", "sortBy": {"created_at": 1}}
    return [
        {"$match": {**TRACKED, "created_at": {"$gte": since, "$lt": until}}},
        {"$setWindowFields": {**by_time, "output": {
            "_prev": {"$shift": {"output": "$created_at", "by": -1}}}}},
        {"$set": {"_new": {"$cond": [{"$or": [
            {"$eq": ["$_prev", None]},
            {"$gt": [{"$subtract": ["$created_at", "$_prev"]}, VISIT_GAP_MS]},
        ]}, 1, 0]}}},
        {"$setWindowFields": {**by_time, "output": {
            "_visit": {"$sum": "$_new", "window": {"documents": ["unbounded", "current"]}}}}},
        {"$sort": {"visitor_hash": 1, "created_at": 1}},
        {"$group": {
            "_id": {"v": "$visitor_hash", "n": "$_visit"},
            "start": {"$first": "$created_at"},
            "pages": {"$sum": 1},
            "engaged": {"$sum": "$engaged_s"},
            "entry": {"$first": "$path"},
            "exit": {"$last": "$path"},
            "channel": {"$first": "$channel"},
            "source": {"$first": "$source"},
            "device": {"$first": "$device"},
            "browser": {"$first": "$browser"},
            "os": {"$first": "$os"},
            "lang": {"$first": "$lang"},
            "hits": {"$push": {"p": "$path", "r": "$route", "e": "$engaged_s"}},
        }},
        # A visit that starts on an internal link is a tab left open past the gap.
        {"$set": {
            "channel": {"$cond": [{"$eq": ["$channel", "internal"]}, "direct", "$channel"]},
            "bounce": {"$cond": [{"$and": [
                {"$lte": ["$pages", 1]}, {"$lt": ["$engaged", BOUNCE_ENGAGED_S]}]}, 1, 0]},
        }},
    ]


def _visit_dim(key, limit: int | None = 30) -> list[dict]:
    """Visits grouped by ``key``: visitors, visits, page views, bounces, seconds."""
    stages = [
        {"$group": {"_id": {"k": key, "v": "$_id.v"}, "visits": {"$sum": 1},
                    "pages": {"$sum": "$pages"}, "bounces": {"$sum": "$bounce"},
                    "engaged": {"$sum": "$engaged"}}},
        {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "visits": {"$sum": "$visits"},
                    "pages": {"$sum": "$pages"}, "bounces": {"$sum": "$bounces"},
                    "engaged": {"$sum": "$engaged"}}},
        {"$sort": {"visits": -1, "_id": 1}},
    ]
    return stages + ([{"$limit": limit}] if limit else [])


def _hit_dim(field: str, limit: int) -> list[dict]:
    """Page views grouped by ``hits.<field>`` (path or route)."""
    return [
        {"$unwind": "$hits"},
        {"$group": {"_id": {"k": f"$hits.{field}", "v": "$_id.v"}, "views": {"$sum": 1},
                    "engaged": {"$sum": "$hits.e"}, "route": {"$first": "$hits.r"}}},
        {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "views": {"$sum": "$views"},
                    "engaged": {"$sum": "$engaged"}, "route": {"$first": "$route"}}},
        {"$sort": {"views": -1, "_id": 1}},
        {"$limit": limit},
    ]


def _totals_facets(granularity: str) -> dict:
    return {
        "totals": _visit_dim({"$literal": "all"}, None),
        "median": [{"$group": {"_id": None, "m": {
            "$median": {"input": "$engaged", "method": "approximate"}}}}],
        "series": _visit_dim(_bucket("start", granularity), None),
    }


def _breakdown_facets() -> dict:
    return {
        "channels": _visit_dim("$channel"),
        "sources": [{"$match": {"source": {"$ne": None}}}, *_visit_dim("$source")],
        "entries": _visit_dim("$entry"),
        "exits": _visit_dim("$exit"),
        "devices": _visit_dim("$device"),
        "browsers": _visit_dim("$browser"),
        "os": _visit_dim("$os"),
        "langs": _visit_dim("$lang"),
        "pages": _hit_dim("p", 100),
        "sections": _hit_dim("r", 60),
    }


def _row(r: dict, name: str = "key") -> dict:
    out = {name: r["_id"]}
    out.update({k: v for k, v in r.items() if k != "_id"})
    return out


def _totals(facet: dict) -> dict:
    t = (facet.get("totals") or [{}])[0]
    med = (facet.get("median") or [{}])[0].get("m")
    visits = t.get("visits", 0)
    return {
        "visitors": t.get("visitors", 0),
        "visits": visits,
        "pageviews": t.get("pages", 0),
        "bounces": t.get("bounces", 0),
        "engaged": t.get("engaged", 0),
        "median_engaged": round(med) if med is not None else None,
    }


def _series(facet: dict, labels: list[str]) -> list[dict]:
    by = {r["_id"]: r for r in facet.get("series", [])}
    return [{"bucket": b, "visitors": by.get(b, {}).get("visitors", 0),
             "visits": by.get(b, {}).get("visits", 0),
             "pageviews": by.get(b, {}).get("pages", 0),
             "bounces": by.get(b, {}).get("bounces", 0),
             "engaged": by.get(b, {}).get("engaged", 0)} for b in labels]


def _filter_stage(filters: dict[str, str]) -> list[dict]:
    match = {k: v for k, v in filters.items() if k in VISIT_FILTERS and v}
    return [{"$match": match}] if match else []


async def site_report(days: int, filters: dict[str, str | None] | None = None) -> dict:
    """Everything the Site Analytics tab shows for the last ``days`` days."""
    chosen = {k: v for k, v in (filters or {}).items() if k in VISIT_FILTERS and v}
    key = ("site", days, tuple(sorted(chosen.items())))
    return await _cached(key, 60 if days <= 31 else 600, lambda: _site_report(days, chosen))


async def _site_report(days: int, filters: dict[str, str]) -> dict:
    since, until, gran, labels = window(days)
    span = until - since
    shift = timedelta(days=days) if gran == "day" else timedelta(hours=24)
    prev_since, prev_until = since - shift, since - shift + span
    prev_labels = window(days, until - shift)[3]
    started = await tracking_since()

    flt = _filter_stage(filters)
    current = _aggregate([*visits_pipeline(since, until), *flt,
                          {"$facet": {**_totals_facets(gran), **_breakdown_facets()}}])
    has_prev = started is not None and prev_since >= started
    previous = _aggregate([*visits_pipeline(prev_since, prev_until), *flt,
                           {"$facet": _totals_facets(gran)}]) if has_prev else None
    cur_rows = await current
    prev_rows = await previous if previous is not None else None

    facet = cur_rows[0] if cur_rows else {}
    out = {
        "window_days": days,
        "granularity": gran,
        "since": since,
        "until": until,
        "tracking_since": started,
        "filters": filters,
        "totals": _totals(facet),
        "series": _series(facet, labels),
        "previous": None,
    }
    if prev_rows is not None:
        pf = prev_rows[0] if prev_rows else {}
        out["previous"] = {"totals": _totals(pf), "series": _series(pf, prev_labels)}
    for name in ("channels", "sources", "entries", "exits", "devices", "browsers", "os",
                 "langs", "pages", "sections"):
        out[name] = [_row(r) for r in facet.get(name, [])]
    return out


async def live() -> dict:
    """Visitors with a page open in the last 5 minutes, and what they're reading."""
    async def compute() -> dict:
        now = utcnow()
        recent = {**TRACKED, "created_at": {"$gte": now - timedelta(hours=6)},
                  "last_at": {"$gte": now - timedelta(minutes=5)}}
        rows = await _aggregate([
            {"$match": recent},
            {"$facet": {
                "visitors": [{"$group": {"_id": "$visitor_hash"}}, {"$count": "n"}],
                "pages": [{"$group": {"_id": {"p": "$path", "v": "$visitor_hash"}}},
                          {"$group": {"_id": "$_id.p", "visitors": {"$sum": 1}}},
                          {"$sort": {"visitors": -1, "_id": 1}}, {"$limit": 5}],
            }},
        ])
        facet = rows[0] if rows else {}
        n = (facet.get("visitors") or [{}])[0].get("n", 0)
        return {"visitors": n, "pages": [_row(r, "path") for r in facet.get("pages", [])]}
    return await _cached(("live",), 15, compute)


# --- creator stats ----------------------------------------------------------------

async def paths_report(paths: list[str], days: int) -> dict:
    """Views of a fixed set of pages (a creator's mods): totals vs the previous
    window, a daily series, and per-page, source and device breakdowns."""
    paths = sorted(set(paths))[:1000]
    key = ("paths", days, hash(tuple(paths)))
    return await _cached(key, 120, lambda: _paths_report(paths, days))


def _path_dim(key, limit: int | None = None) -> list[dict]:
    stages = [
        {"$group": {"_id": {"k": key, "v": "$visitor_hash"}, "views": {"$sum": 1},
                    "engaged": {"$sum": "$engaged_s"}}},
        {"$group": {"_id": "$_id.k", "visitors": {"$sum": 1}, "views": {"$sum": "$views"},
                    "engaged": {"$sum": "$engaged"}}},
        {"$sort": {"views": -1, "_id": 1}},
    ]
    return stages + ([{"$limit": limit}] if limit else [])


async def _paths_report(paths: list[str], days: int) -> dict:
    since, until, gran, labels = window(max(days, 2))
    shift = timedelta(days=days)
    started = await tracking_since()
    has_prev = started is not None and since - shift >= started

    def match(lo, hi):
        return {"$match": {**TRACKED, "path": {"$in": paths}, "created_at": {"$gte": lo, "$lt": hi}}}

    totals = _path_dim({"$literal": "all"})
    current = _aggregate([match(since, until), {"$facet": {
        "totals": totals,
        "series": _path_dim(_bucket("created_at", gran)),
        "pages": _path_dim("$path"),
        "channels": _path_dim("$channel"),
        "sources": [{"$match": {"source": {"$ne": None}}}, *_path_dim("$source", 15)],
        "devices": _path_dim("$device"),
    }}])
    previous = _aggregate([match(since - shift, until - shift), {"$facet": {"totals": totals}}]) \
        if has_prev else None
    rows = await current
    prev = await previous if previous is not None else None

    facet = rows[0] if rows else {}

    def tot(f):
        t = (f.get("totals") or [{}])[0]
        return {"views": t.get("views", 0), "visitors": t.get("visitors", 0),
                "engaged": t.get("engaged", 0)}

    by = {r["_id"]: r for r in facet.get("series", [])}
    return {
        "window_days": days,
        "since": since,
        "tracking_since": started,
        "totals": tot(facet),
        "previous": tot(prev[0] if prev else {}) if prev is not None else None,
        "series": [{"bucket": b, "views": by.get(b, {}).get("views", 0),
                    "visitors": by.get(b, {}).get("visitors", 0)} for b in labels],
        "pages": [_row(r, "path") for r in facet.get("pages", [])],
        "channels": [_row(r) for r in facet.get("channels", [])],
        "sources": [_row(r) for r in facet.get("sources", [])],
        "devices": [_row(r) for r in facet.get("devices", [])],
    }
