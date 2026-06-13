"""Master-only giveaway + vault management endpoints.

Mounted under ``/admin`` with a router-level superuser gate (same as
``app/admin/router.py``). Powers the portal's Modules -> Giveaways page:
  - ``/admin/vault/items``  - drawers: a named prize + its codes (CRUD + bulk add)
  - ``/admin/giveaways``    - create / edit / draw / cancel giveaways
"""
from fastapi import APIRouter, Depends, status

from app.core.dependencies import get_current_superuser
from app.giveaways import service
from app.giveaways.schemas import (
    GiveawayAdminView,
    GiveawayCreate,
    GiveawayUpdate,
    VaultCodesAdd,
    VaultCodeUpdate,
    VaultCodeView,
    VaultItemCreate,
    VaultItemUpdate,
    VaultItemView,
)

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


# ── Vault: drawers (items) + their codes ────────────────────────────────────

@router.get("/vault/items")
async def vault_items() -> list[VaultItemView]:
    return await service.list_items()


@router.post("/vault/items", response_model=VaultItemView, status_code=status.HTTP_201_CREATED)
async def vault_item_create(req: VaultItemCreate) -> VaultItemView:
    return await service.create_item(req)


@router.patch("/vault/items/{item_id}", response_model=VaultItemView)
async def vault_item_update(item_id: str, req: VaultItemUpdate) -> VaultItemView:
    return await service.update_item(item_id, req)


@router.delete("/vault/items/{item_id}", status_code=status.HTTP_204_NO_CONTENT)
async def vault_item_delete(item_id: str) -> None:
    await service.delete_item(item_id)


@router.get("/vault/items/{item_id}/codes")
async def vault_item_codes(item_id: str) -> list[VaultCodeView]:
    return await service.list_codes(item_id)


@router.post("/vault/items/{item_id}/codes")
async def vault_codes_add(item_id: str, req: VaultCodesAdd) -> dict:
    return await service.add_codes(item_id, req)


@router.patch("/vault/codes/{code_id}", response_model=VaultCodeView)
async def vault_code_update(code_id: str, req: VaultCodeUpdate) -> VaultCodeView:
    return await service.update_code(code_id, req)


@router.delete("/vault/codes/{code_id}", status_code=status.HTTP_204_NO_CONTENT)
async def vault_code_delete(code_id: str) -> None:
    await service.delete_code(code_id)


# ── Giveaways ───────────────────────────────────────────────────────────────

@router.get("/giveaways")
async def giveaways_list() -> list[GiveawayAdminView]:
    return await service.list_admin()


@router.post("/giveaways", response_model=GiveawayAdminView, status_code=status.HTTP_201_CREATED)
async def giveaways_create(req: GiveawayCreate) -> GiveawayAdminView:
    return await service.create(req)


@router.patch("/giveaways/{giveaway_id}", response_model=GiveawayAdminView)
async def giveaways_update(giveaway_id: str, req: GiveawayUpdate) -> GiveawayAdminView:
    return await service.update(giveaway_id, req)


@router.post("/giveaways/{giveaway_id}/draw", response_model=GiveawayAdminView)
async def giveaways_draw(giveaway_id: str) -> GiveawayAdminView:
    return await service.draw_now(giveaway_id)


@router.post("/giveaways/{giveaway_id}/cancel", response_model=GiveawayAdminView)
async def giveaways_cancel(giveaway_id: str) -> GiveawayAdminView:
    return await service.cancel(giveaway_id)
