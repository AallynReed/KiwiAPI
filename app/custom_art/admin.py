"""Master-only custom art review. Powers the portal's Modules -> Custom art page."""
from fastapi import APIRouter, Depends
from fastapi.responses import Response
from pydantic import BaseModel, Field

from app.core.dependencies import get_current_superuser
from app.custom_art import service

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


class ApproveBody(BaseModel):
    name: str | None = Field(default=None, max_length=200)


class DenyBody(BaseModel):
    reason: str = Field(min_length=3, max_length=2000)


@router.get("/custom-art")
async def custom_art_list() -> dict:
    return {"items": await service.list_requests()}


@router.get("/custom-art/{request_id}/image/{slot}")
async def custom_art_image(request_id: str, slot: str) -> Response:
    data, content_type = await service.image(request_id, slot)
    return Response(data, media_type=content_type, headers={"Cache-Control": "no-store"})


@router.post("/custom-art/release")
async def custom_art_release() -> dict:
    """Queue every approved request for the worker to build and publish silently."""
    return {"queued": await service.release()}


@router.post("/custom-art/steam-done")
async def custom_art_steam_done() -> dict:
    return {"cleared": await service.steam_done()}


@router.post("/custom-art/{request_id}/approve")
async def custom_art_approve(request_id: str, body: ApproveBody) -> dict:
    return await service.approve(request_id, body.name)


@router.post("/custom-art/{request_id}/deny")
async def custom_art_deny(request_id: str, body: DenyBody) -> dict:
    """Deny with a reason; the submitter is emailed it."""
    return await service.deny(request_id, body.reason)


@router.post("/custom-art/{request_id}/retry")
async def custom_art_retry(request_id: str) -> dict:
    return {"queued": await service.retry(request_id)}
