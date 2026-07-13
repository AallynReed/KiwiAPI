from pydantic import BaseModel


class PageStat(BaseModel):
    path: str            # concrete page URL, e.g. /player/Alice
    views: int
    unique_visitors: int


class SeriesPoint(BaseModel):
    bucket: str           # UTC bucket label: "2026-07-07" (day) or "2026-07-07T14:00" (hour)
    views: int
    unique_visitors: int


class PageviewSummary(BaseModel):
    window_days: int
    total_views: int
    unique_visitors: int  # distinct visitors, counted once per UTC day
    views_today: int
    granularity: str      # "hour" (<=1-day window) | "day" - how `series` is bucketed
    series: list[SeriesPoint]  # continuous time series (zero-filled) for the trend chart
    pages: list[PageStat]
