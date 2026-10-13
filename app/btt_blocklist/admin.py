"""Master-only editing of the BTT mod blocklist (dev portal -> Mods hub tab)."""
from fastapi import APIRouter, Depends, status
from pydantic import BaseModel, Field

from app.btt_blocklist import service
from app.btt_blocklist.models import BlockKind
from app.core.dependencies import get_current_superuser

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


class BlockAdd(BaseModel):
    kind: BlockKind
    values: str = Field(max_length=20000, description="One or more values, separated by commas or whitespace.")
    note: str = Field(default="", max_length=500)


@router.get("/btt/mod-blocklist")
async def blocklist_list() -> dict:
    return {"items": await service.list_entries()}


@router.post("/btt/mod-blocklist", status_code=status.HTTP_201_CREATED)
async def blocklist_add(req: BlockAdd) -> dict:
    return {"added": await service.add(req.kind, req.values, req.note)}


@router.delete("/btt/mod-blocklist/{entry_id}", status_code=status.HTTP_204_NO_CONTENT)
async def blocklist_remove(entry_id: str) -> None:
    await service.remove(entry_id)
