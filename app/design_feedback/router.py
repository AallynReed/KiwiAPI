"""Public vote endpoint for the "Do you like the new design?" prompt."""
from typing import Literal

from fastapi import APIRouter, Depends, Request
from pydantic import BaseModel

from app.admin import runtime_config
from app.core.ratelimit import check_rate_limit
from app.core.utils import client_ip
from app.design_feedback import service

router = APIRouter(tags=["site"])


class DesignVote(BaseModel):
    design: Literal["voxel", "classic"]
    theme: Literal["auto", "light", "dark"] = "auto"
    vote: Literal["up", "down"]


async def _throttle(request: Request) -> None:
    max_, window = await runtime_config.get_rate_limit("design_feedback_rate_limit")
    await check_rate_limit(f"designfeedback:{client_ip(request) or 'unknown'}", max_, window)


@router.post("/site/design-feedback", status_code=202)
async def site_design_feedback(vote: DesignVote, _limit: None = Depends(_throttle)) -> dict:
    """Count one anonymous thumbs up/down. Stores a tally only, never the voter."""
    await service.record_vote(vote.design, vote.theme, vote.vote)
    return {"ok": True}
