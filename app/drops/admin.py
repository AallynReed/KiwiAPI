"""Master-only file-drop management.

Mounted under ``/admin`` with a router-level superuser gate (same as
``app/admin/router.py``). Powers the portal's Modules -> File drops page: mint a
link, watch what lands on it, download it, delete it.
"""
from fastapi import APIRouter, Depends, status
from fastapi.responses import FileResponse

from app.core.dependencies import get_current_superuser
from app.drops import service
from app.drops.schemas import DropCreate, DropCreated, DropUpdate, DropView

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


@router.get("/drops")
async def drops_list() -> list[DropView]:
    return await service.list_drops()


@router.post("/drops", response_model=DropCreated, status_code=status.HTTP_201_CREATED)
async def drops_create(req: DropCreate) -> DropCreated:
    """Mint a link. The response carries the PIN in the clear once - it is stored
    only as an argon2 hash, so a lost PIN means minting a new drop."""
    return await service.create(req)


@router.patch("/drops/{drop_id}", response_model=DropView)
async def drops_update(drop_id: str, req: DropUpdate) -> DropView:
    return await service.update(drop_id, req)


@router.delete("/drops/{drop_id}", status_code=status.HTTP_204_NO_CONTENT)
async def drops_delete(drop_id: str) -> None:
    """Delete the link AND everything uploaded through it."""
    await service.delete(drop_id)


@router.get("/drops/uploads/{upload_id}/download")
async def drop_upload_download(upload_id: str) -> FileResponse:
    """Download a received file. Master-only: this is the only way the bytes
    come back out of the server, and there is no public read path at all."""
    upload, path = await service.upload_path(upload_id)
    return FileResponse(
        path,
        media_type=upload.content_type or "application/octet-stream",
        filename=upload.filename,
        headers={"Cache-Control": "no-store"},
    )


@router.delete("/drops/uploads/{upload_id}", status_code=status.HTTP_204_NO_CONTENT)
async def drop_upload_delete(upload_id: str) -> None:
    await service.delete_upload(upload_id)
