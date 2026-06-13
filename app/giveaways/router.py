"""Giveaway endpoints.

Two surfaces:
- ``router`` - signed-in SITE-USER actions (enter a giveaway, list your
  entries). Site-JWT auth; included with ``include_in_schema=False`` (not part
  of the public API).
- ``public_router`` - the TOKENLESS public read (ongoing draws) under the
  ``giveaways:read`` scope, shown in the OpenAPI schema. The showcase page
  itself uses the same-origin ``/site/giveaways`` proxy in ``app/site/router``.
"""
from fastapi import APIRouter, Depends, Query, Response

from app.core.dependencies import AccessContext, public_scope
from app.giveaways import service
from app.giveaways.schemas import EnterResponse, GiveawayPublicView, MyGiveawayView
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser

router = APIRouter(prefix="/v1/giveaways", tags=["giveaways"])

# Public, tokenless read surface - separate router so it stays in the OpenAPI
# schema (the site-user router above is included with include_in_schema=False).
public_router = APIRouter(prefix="/v1/giveaways", tags=["giveaways"])
_GW_PUBLIC = Depends(public_scope("giveaways:read"))


@public_router.get(
    "/ongoing", response_model=list[GiveawayPublicView],
    summary="List ongoing (open) giveaways",
)
async def ongoing_giveaways(
    response: Response, _ctx: AccessContext = _GW_PUBLIC,
) -> list[GiveawayPublicView]:
    """**Tokenless.** Currently-open giveaways (accepting entries), soonest to
    end first. Each carries the prize, the open/close window, and the live
    ``entry_count`` so a client can show the draw and compute odds
    (``1 / entry_count``). ``winner_username`` is always null here (winners
    appear only after a draw); the prize code is NEVER exposed. Cached 30s."""
    response.headers["Cache-Control"] = "public, max-age=30"
    return await service.list_ongoing()


@public_router.get(
    "/upcoming", response_model=list[GiveawayPublicView],
    summary="List upcoming (scheduled) giveaways",
)
async def upcoming_giveaways(
    response: Response, _ctx: AccessContext = _GW_PUBLIC,
) -> list[GiveawayPublicView]:
    """**Tokenless.** Scheduled giveaways not yet open, soonest-starting first.
    `status` is `"scheduled"`; entries open at `starts_at`. Same shape as
    `/ongoing` (prize, window, `entry_count` which is 0 until it opens)."""
    response.headers["Cache-Control"] = "public, max-age=30"
    return await service.list_upcoming()


@public_router.get(
    "/ended", response_model=list[GiveawayPublicView],
    summary="List recently-ended giveaways",
)
async def ended_giveaways(
    response: Response,
    days: int = Query(default=7, ge=1, le=30, description="Look-back window in days."),
    _ctx: AccessContext = _GW_PUBLIC,
) -> list[GiveawayPublicView]:
    """**Tokenless.** Giveaways that ended in the last `days` days (default 7,
    max 30), most-recently-ended first. `status` is `"drawn"` (had a winner -
    see `winner_username`) or `"closed"` (no entrants). Cancelled giveaways are
    excluded; the prize code is NEVER exposed. Cached 30s."""
    response.headers["Cache-Control"] = "public, max-age=30"
    return await service.list_ended(days=days)


@router.get("/mine")
async def my_entries(user: SiteUser = Depends(get_current_site_user)) -> dict:
    """The giveaway ids the current user has entered (the public page uses this
    to flip the Enter button to 'Entered' and compute odds)."""
    return {"giveaway_ids": await service.my_entry_ids(user)}


@router.get("/me", response_model=list[MyGiveawayView])
async def my_giveaways(user: SiteUser = Depends(get_current_site_user)) -> list[MyGiveawayView]:
    """Full participation list for the dashboard - entered giveaways, with the
    prize code attached to any the user won."""
    return await service.my_participations(user)


@router.post("/{giveaway_id}/enter", response_model=EnterResponse)
async def enter(
    giveaway_id: str, user: SiteUser = Depends(get_current_site_user),
) -> EnterResponse:
    return await service.enter(user, giveaway_id)
