"""Master-only readout of the design-feedback tallies (Dev Portal, Site Analytics)."""
from fastapi import APIRouter, Depends, Query

from app.core.dependencies import get_current_superuser
from app.design_feedback import service

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


@router.get("/design-feedback")
async def design_feedback_summary(days: int = Query(default=30, ge=1, le=365)) -> dict:
    return await service.summary(days)
