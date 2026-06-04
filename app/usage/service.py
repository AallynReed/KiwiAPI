from datetime import timedelta

from app.core.utils import utcnow
from app.usage.models import UsageEvent
from app.usage.schemas import ActivitySummary, DailyStat, EndpointStat

# Count anything >= 400 as an error in metrics.
ERROR_COND = {"$cond": [{"$gte": ["$status_code", 400]}, 1, 0]}
# Count 429s specifically — rate-limit triggers.
RATE_LIMITED_COND = {"$cond": [{"$eq": ["$status_code", 429]}, 1, 0]}


async def aggregate_activity(base_match: dict, days: int) -> ActivitySummary:
    """Aggregate usage events matching `base_match` over the last `days` days.

    `base_match` scopes the query — e.g. {"user_id": ...} for a whole account or
    {"token_id": ...} for a single token. Shared by the user and admin APIs.
    """
    since = utcnow() - timedelta(days=days)
    match = {**base_match, "created_at": {"$gte": since}}

    totals = await UsageEvent.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": None,
                    "count": {"$sum": 1},
                    "error_count": {"$sum": ERROR_COND},
                    "rate_limited": {"$sum": RATE_LIMITED_COND},
                    "avg_duration_ms": {"$avg": "$duration_ms"},
                }
            },
        ]
    ).to_list()

    by_day = await UsageEvent.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": {
                        "$dateToString": {
                            "format": "%Y-%m-%d",
                            "date": "$created_at",
                            "timezone": "UTC",
                        }
                    },
                    "count": {"$sum": 1},
                    "error_count": {"$sum": ERROR_COND},
                    "avg_duration_ms": {"$avg": "$duration_ms"},
                }
            },
            {"$sort": {"_id": 1}},
        ]
    ).to_list()

    by_endpoint = await UsageEvent.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": {"route": "$route", "method": "$method"},
                    "count": {"$sum": 1},
                    "error_count": {"$sum": ERROR_COND},
                    "avg_duration_ms": {"$avg": "$duration_ms"},
                }
            },
            {"$sort": {"count": -1}},
        ]
    ).to_list()

    t = totals[0] if totals else {"count": 0, "error_count": 0, "rate_limited": 0, "avg_duration_ms": 0}
    return ActivitySummary(
        window_days=days,
        total_requests=t["count"],
        error_count=t["error_count"],
        rate_limited=t.get("rate_limited", 0),
        avg_duration_ms=round(t["avg_duration_ms"] or 0, 2),
        by_day=[
            DailyStat(
                date=r["_id"],
                count=r["count"],
                error_count=r["error_count"],
                avg_duration_ms=round(r["avg_duration_ms"] or 0, 2),
            )
            for r in by_day
        ],
        by_endpoint=[
            EndpointStat(
                route=r["_id"]["route"],
                method=r["_id"]["method"],
                count=r["count"],
                error_count=r["error_count"],
                avg_duration_ms=round(r["avg_duration_ms"] or 0, 2),
            )
            for r in by_endpoint
        ],
    )
