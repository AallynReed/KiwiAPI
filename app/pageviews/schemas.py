from pydantic import BaseModel


class PageStat(BaseModel):
    path: str            # concrete page URL, e.g. /player/Alice
    views: int
    unique_visitors: int


class PageviewSummary(BaseModel):
    window_days: int
    total_views: int
    unique_visitors: int  # distinct visitors, counted once per UTC day
    views_today: int
    pages: list[PageStat]
