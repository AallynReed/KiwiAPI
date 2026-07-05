"""CRUD + validation for Discord DM subscriptions."""

from __future__ import annotations

from beanie import PydanticObjectId

from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, utcnow
from app.dm_subs.models import (
    CHALLENGE_TYPES,
    DM_EVENT_TYPES,
    MAX_SUBSCRIPTIONS_PER_USER,
    MAX_WATCH_ITEMS,
    DmSubscription,
)
from app.site_auth.models import SiteUser


def _clean_events(events: list[str] | None) -> list[str]:
    chosen = {e for e in (events or []) if e in DM_EVENT_TYPES}
    if not chosen:
        raise APIError(400, ErrorCode.bad_request,
                       f"Pick at least one alert: {', '.join(DM_EVENT_TYPES)}.")
    return [e for e in DM_EVENT_TYPES if e in chosen]   # canonical order


def _clean_filters(filters: dict | None, events: list[str]) -> dict:
    """Validate + trim the per-event filter blob to only what the chosen events use."""
    filters = filters or {}
    out: dict = {}

    if "challenge" in events:
        types = filters.get("challenge_types")
        if types:
            bad = [t for t in types if t not in CHALLENGE_TYPES]
            if bad:
                raise APIError(400, ErrorCode.bad_request,
                               f"Unknown challenge type(s): {', '.join(bad)}.")
            out["challenge_types"] = [t for t in CHALLENGE_TYPES if t in types]

    if "market_watch" in events:
        raw = filters.get("watch") or []
        watch = []
        for w in raw:
            name = (w.get("name") or "").strip() if isinstance(w, dict) else ""
            try:
                thr = float(w.get("max_price_each"))
            except (TypeError, ValueError, AttributeError):
                thr = 0
            if not name or thr <= 0:
                continue
            watch.append({"name": name[:120], "max_price_each": thr})
        if not watch:
            raise APIError(400, ErrorCode.bad_request,
                           "Add at least one item (with a price) to your market watchlist.")
        if len(watch) > MAX_WATCH_ITEMS:
            raise APIError(400, ErrorCode.bad_request,
                           f"At most {MAX_WATCH_ITEMS} watchlist items.")
        out["watch"] = watch

    return out


def _dto(s: DmSubscription) -> dict:
    return {
        "id": str(s.id),
        "label": s.label,
        "events": s.events,
        "filters": s.filters or {},
        "active": s.active,
        "last_status": s.last_status,
        "last_error": s.last_error,
        "last_delivered_at": iso(s.last_delivered_at),
        "disabled_reason": s.disabled_reason,
        "created_at": s.created_at.isoformat(),
    }


def _require_discord(actor: SiteUser) -> int:
    if not actor.discord_id:
        raise APIError(400, ErrorCode.bad_request,
                       "Your account isn't linked to Discord, so we can't DM you.")
    return int(actor.discord_id)


async def _owned(actor: SiteUser, sub_id: str) -> DmSubscription:
    try:
        s = await DmSubscription.get(PydanticObjectId(sub_id))
    except Exception:
        s = None
    if s is None or s.owner_id != actor.id:
        raise APIError(404, ErrorCode.not_found, "Subscription not found.")
    return s


async def list_subscriptions(actor: SiteUser) -> list[dict]:
    docs = await DmSubscription.find(
        DmSubscription.owner_id == actor.id,
    ).sort("-created_at").to_list()
    return [_dto(d) for d in docs]


async def create_subscription(
    actor: SiteUser, events: list[str], filters: dict | None, label: str,
) -> dict:
    discord_id = _require_discord(actor)
    count = await DmSubscription.find(DmSubscription.owner_id == actor.id).count()
    if count >= MAX_SUBSCRIPTIONS_PER_USER:
        raise APIError(400, ErrorCode.bad_request,
                       f"You can have at most {MAX_SUBSCRIPTIONS_PER_USER} DM subscriptions.")
    clean_events = _clean_events(events)
    doc = DmSubscription(
        owner_id=actor.id,
        owner_discord_id=discord_id,
        events=clean_events,
        filters=_clean_filters(filters, clean_events),
        label=(label or "").strip()[:80],
    )
    await doc.insert()
    return _dto(doc)


async def update_subscription(
    actor: SiteUser, sub_id: str, *,
    events: list[str] | None = None, filters: dict | None = None,
    label: str | None = None, active: bool | None = None,
) -> dict:
    s = await _owned(actor, sub_id)
    if events is not None:
        s.events = _clean_events(events)
    if filters is not None or events is not None:
        s.filters = _clean_filters(filters if filters is not None else s.filters, s.events)
    if label is not None:
        s.label = label.strip()[:80]
    if active is not None:
        s.active = active
        if active:
            s.consecutive_failures = 0
            s.disabled_reason = None
            # Refresh the cached snowflake in case the account re-linked Discord.
            s.owner_discord_id = _require_discord(actor)
    s.updated_at = utcnow()
    await s.save()
    return _dto(s)


async def delete_subscription(actor: SiteUser, sub_id: str) -> None:
    s = await _owned(actor, sub_id)
    await s.delete()


async def send_test(actor: SiteUser, sub_id: str) -> dict:
    """DM the owner a test message now and report the result."""
    from app.bot import discord_rest
    from app.dm_subs import embeds
    s = await _owned(actor, sub_id)
    ok, status, error = await discord_rest.send_dm(s.owner_discord_id, embeds.test_body())
    return {"ok": ok, "status": status, "error": error}
