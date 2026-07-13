from datetime import timedelta

from app.core.utils import utcnow
from app.pageviews.models import PageView
from app.pageviews.schemas import PageStat, PageviewSummary, SeriesPoint


async def _daily_series(match: dict, granularity: str) -> dict[str, tuple[int, int]]:
    """Bucket the matched page views by UTC hour or day, returning
    ``{bucket_label: (views, unique_visitors)}``. Buckets with no views are absent
    (the caller zero-fills a continuous range).

    Unique visitors per bucket = distinct ``visitor_hash`` in that bucket; the salt
    rotates daily so a hash is stable within a UTC day, making per-bucket distinct a
    faithful daily-active count.
    """
    fmt = "%Y-%m-%dT%H:00" if granularity == "hour" else "%Y-%m-%d"
    label = {"$dateToString": {"date": "$created_at", "format": fmt, "timezone": "UTC"}}

    views_rows = await PageView.aggregate(
        [{"$match": match}, {"$group": {"_id": label, "views": {"$sum": 1}}}]
    ).to_list()

    # Two-stage distinct: collapse (bucket, hash) pairs first so no $group holds a
    # large in-memory set, then count the survivors per bucket.
    uniq_rows = await PageView.aggregate(
        [
            {"$match": match},
            {"$group": {"_id": {"b": label, "vh": "$visitor_hash"}}},
            {"$group": {"_id": "$_id.b", "u": {"$sum": 1}}},
        ]
    ).to_list()

    views = {r["_id"]: r["views"] for r in views_rows}
    uniq = {r["_id"]: r["u"] for r in uniq_rows}
    return {b: (v, uniq.get(b, 0)) for b, v in views.items()}


async def aggregate_pageviews(days: int, top: int = 100) -> PageviewSummary:
    """Roll up showcase-site page views over the last ``days`` days for the admin
    "Site Analytics" dashboard (per-page list is the ``top`` pages by views; totals +
    unique visitors stay global).

    Because the visitor-hash salt rotates daily (see ``pageviews.middleware``), a
    returning visitor produces a new hash each day - so a multi-day "unique visitors"
    figure is the sum of daily uniques (daily-active visitors), not distinct humans
    across the whole window. Standard cookieless trade-off, fine at this scale.
    """
    now = utcnow()
    since = now - timedelta(days=days)
    match = {"created_at": {"$gte": since}}

    total_views = await PageView.find(match).count()

    # Site-wide unique visitors: distinct visitor_hash. A two-stage group (distinct
    # the hashes, then count) avoids building one huge in-memory array in $group.
    uniq = await PageView.aggregate(
        [
            {"$match": match},
            {"$group": {"_id": "$visitor_hash"}},
            {"$count": "n"},
        ]
    ).to_list()
    unique_visitors = uniq[0]["n"] if uniq else 0

    # Per-page views + uniques. $addToSet of visitor_hash is bounded per-page, so
    # the array stays small even when the site-wide distinct set is large.
    by_page = await PageView.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": "$path",
                    "views": {"$sum": 1},
                    "visitors": {"$addToSet": "$visitor_hash"},
                }
            },
            {"$project": {"views": 1, "unique_visitors": {"$size": "$visitors"}}},
            {"$sort": {"views": -1}},
            {"$limit": top},
        ]
    ).to_list()

    start_today = now.replace(hour=0, minute=0, second=0, microsecond=0)
    views_today = await PageView.find({"created_at": {"$gte": start_today}}).count()

    # Trend chart: hourly buckets for the 24h window, daily otherwise. Build a
    # continuous label range so the chart shows zero-view buckets as gaps, not skips.
    granularity = "hour" if days <= 1 else "day"
    buckets = await _daily_series(match, granularity)
    if granularity == "hour":
        step = timedelta(hours=1)
        cursor = (now - timedelta(hours=23)).replace(minute=0, second=0, microsecond=0)
        end = now.replace(minute=0, second=0, microsecond=0)
        fmt = "%Y-%m-%dT%H:00"
    else:
        step = timedelta(days=1)
        cursor = start_today - timedelta(days=days - 1)
        end = start_today
        fmt = "%Y-%m-%d"
    series: list[SeriesPoint] = []
    while cursor <= end:
        b = cursor.strftime(fmt)
        v, u = buckets.get(b, (0, 0))
        series.append(SeriesPoint(bucket=b, views=v, unique_visitors=u))
        cursor += step

    return PageviewSummary(
        window_days=days,
        total_views=total_views,
        unique_visitors=unique_visitors,
        views_today=views_today,
        granularity=granularity,
        series=series,
        pages=[
            PageStat(
                path=r["_id"],
                views=r["views"],
                unique_visitors=r["unique_visitors"],
            )
            for r in by_page
        ],
    )
