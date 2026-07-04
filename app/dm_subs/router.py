"""User Dashboard CRUD for Discord DM subscriptions.

Site-login-gated (``get_current_site_user``) and ``include_in_schema=False`` - a
website Dashboard feature, not a developer API-token surface. The whole router is
gated by ``require_dm_subs_enabled`` where it's mounted in main.py.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, Response

from app.dm_subs import service
from app.dm_subs.models import CHALLENGE_TYPES, DM_EVENT_TYPES
from app.dm_subs.schemas import DmSubCreate, DmSubUpdate
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser

router = APIRouter(prefix="/v1/dm-subscriptions", tags=["dm-subscriptions"])

_USER = Depends(get_current_site_user)


@router.get("/events")
async def list_event_types() -> dict:
    """The alert types + the challenge sub-types available for filtering."""
    return {"events": list(DM_EVENT_TYPES), "challenge_types": list(CHALLENGE_TYPES)}


@router.get("")
async def list_subscriptions(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_subscriptions(user),
            "discord_linked": bool(user.discord_id)}


@router.post("", status_code=201)
async def create_subscription(req: DmSubCreate, user: SiteUser = _USER) -> dict:
    return await service.create_subscription(user, req.events, req.filters, req.label)


@router.patch("/{sub_id}")
async def update_subscription(sub_id: str, req: DmSubUpdate, user: SiteUser = _USER) -> dict:
    return await service.update_subscription(
        user, sub_id, events=req.events, filters=req.filters,
        label=req.label, active=req.active)


@router.delete("/{sub_id}", status_code=204)
async def delete_subscription(sub_id: str, user: SiteUser = _USER) -> Response:
    await service.delete_subscription(user, sub_id)
    return Response(status_code=204)


@router.post("/{sub_id}/test")
async def test_subscription(sub_id: str, user: SiteUser = _USER) -> dict:
    """DM the owner a test message now and report the Discord result."""
    return await service.send_test(user, sub_id)
