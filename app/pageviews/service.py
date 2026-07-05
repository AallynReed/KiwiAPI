from datetime import timedelta

from app.core.utils import utcnow
from app.pageviews.models import PageView
from app.pageviews.schemas import PageStat, PageviewSummary


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

    return PageviewSummary(
        window_days=days,
        total_views=total_views,
        unique_visitors=unique_visitors,
        views_today=views_today,
        pages=[
            PageStat(
                path=r["_id"],
                views=r["views"],
                unique_visitors=r["unique_visitors"],
            )
            for r in by_page
        ],
    )
