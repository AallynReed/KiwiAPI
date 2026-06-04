from pydantic import BaseModel


class EndpointStat(BaseModel):
    route: str
    method: str
    count: int
    error_count: int
    avg_duration_ms: float


class DailyStat(BaseModel):
    date: str  # YYYY-MM-DD (UTC)
    count: int
    error_count: int
    avg_duration_ms: float


class ActivitySummary(BaseModel):
    window_days: int
    total_requests: int
    error_count: int
    rate_limited: int  # count of 429s (rate-limit triggers)
    avg_duration_ms: float
    by_day: list[DailyStat]
    by_endpoint: list[EndpointStat]
