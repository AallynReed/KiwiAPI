"""User Dashboard CRUD for outbound (Discord) webhooks.

Site-login-gated (``get_current_site_user``) and ``include_in_schema=False`` - this
is a website Dashboard feature, not an API-token developer surface. The whole
router is gated by ``require_webhooks_enabled`` where it's mounted in main.py.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, Query, Response

from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser
from app.webhooks import service
from app.webhooks.models import WEBHOOK_EVENT_TYPES
from app.webhooks.schemas import WebhookCreate, WebhookUpdate

router = APIRouter(prefix="/v1/webhooks", tags=["webhooks"])

_USER = Depends(get_current_site_user)


@router.get("/events")
async def list_event_types() -> dict:
    """The deliverable event types + per-event embed-editor metadata (variables,
    default template, sample context for the live preview)."""
    return {"events": list(WEBHOOK_EVENT_TYPES), "meta": service.event_meta()}


@router.get("")
async def list_webhooks(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_webhooks(user)}


@router.post("", status_code=201)
async def create_webhook(req: WebhookCreate, user: SiteUser = _USER) -> dict:
    return await service.create_webhook(
        user, req.url, req.events, req.label, req.templates)


@router.patch("/{webhook_id}")
async def update_webhook(webhook_id: str, req: WebhookUpdate, user: SiteUser = _USER) -> dict:
    return await service.update_webhook(
        user, webhook_id,
        url=req.url, events=req.events, label=req.label, active=req.active,
        templates=req.templates,
    )


@router.delete("/{webhook_id}", status_code=204)
async def delete_webhook(webhook_id: str, user: SiteUser = _USER) -> Response:
    await service.delete_webhook(user, webhook_id)
    return Response(status_code=204)


@router.post("/{webhook_id}/test")
async def test_webhook(
    webhook_id: str,
    event: str | None = Query(None, description="Preview a specific event's embed"),
    user: SiteUser = _USER,
) -> dict:
    """POST a test embed to the webhook now and report the HTTP result. With
    ``?event=`` it renders that event's embed (custom-or-default) using sample data."""
    return await service.send_test(user, webhook_id, event)
