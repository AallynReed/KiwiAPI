from fastapi import APIRouter, Depends, Query

from app.core.dependencies import TokenContext, require_scope
from app.trove import server_time
from app.trove.models import TroveNews
from app.trove.schemas import CalendarResponse, TroveNewsItem, TroveNewsList

router = APIRouter(prefix="/v1/trove", tags=["trove"])


@router.get("/calendar", response_model=CalendarResponse)
async def get_calendar(
    ctx: TokenContext = Depends(require_scope("trove:read")),
) -> CalendarResponse:
    """Current Trove server time, today's daily + weekly bonuses, and the live
    merchant timers (Corruxion, Fluxion) — the home-page snapshot.

    Timer timestamps are real-UTC unix seconds; `seconds_remaining` counts down to
    the merchant leaving (when `active`) or arriving (when not).
    """
    return CalendarResponse(**server_time.calendar_snapshot())


@router.get("/news", response_model=TroveNewsList)
async def list_news(
    ctx: TokenContext = Depends(require_scope("trove:read")),
    limit: int = Query(default=20, ge=1, le=50),
) -> TroveNewsList:
    """Latest Trove news, relayed from trovegame.com and cached server-side
    (refreshed periodically). Newest first by publish date."""
    docs = await TroveNews.find().sort("-published_at").limit(limit).to_list()
    items = [
        TroveNewsItem(
            title=d.title,
            url=d.url,
            author=d.author,
            summary=d.summary,
            category=d.category,
            categories=d.categories,
            image=d.image,
            published_at=d.published_at,
        )
        for d in docs
    ]
    return TroveNewsList(items=items, count=len(items))
