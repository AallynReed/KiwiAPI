from datetime import datetime
from typing import Any

from pydantic import BaseModel


class MerchantTimer(BaseModel):
    active: bool
    state: str | None = None          # fluxion only: "voting" | "selling" | "away"
    starts_at: int                    # unix seconds (real UTC)
    ends_at: int                      # unix seconds (real UTC)
    seconds_remaining: int            # until end (if active) else until start


class ServerTimeInfo(BaseModel):
    now_unix: int
    trove_day: str                    # e.g. "Friday" (the current in-game day)
    daily_reset_at: int               # next 11:00 UTC, unix seconds
    weekly_reset_at: int              # next weekly-buff rotation, unix seconds


class CalendarResponse(BaseModel):
    server_time: ServerTimeInfo
    daily: dict[str, Any]             # today's daily buff (raw game data)
    weekly: dict[str, Any]            # this week's weekly buff (raw game data)
    merchants: dict[str, MerchantTimer]  # keys: corruxion, fluxion


class TroveNewsItem(BaseModel):
    title: str
    url: str
    author: str
    summary: str
    category: str
    categories: list[str]
    image: str | None = None
    published_at: datetime | None = None


class TroveNewsList(BaseModel):
    items: list[TroveNewsItem]
    count: int
