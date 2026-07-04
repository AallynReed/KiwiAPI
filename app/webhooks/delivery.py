"""Webhook delivery: a durable per-worker queue + the Discord POST itself.

**Enqueue** is called from ``app/events/bus.py`` the moment an event is emitted -
and ``bus.publish()`` already guarantees that emit is *exactly once across all
uvicorn workers* (the Redis compare-and-set), so we enqueue exactly once with no
extra dedup.

**Queue.** A Redis list (``LPUSH`` enqueue / ``BRPOP`` consume). With several
workers each blocking on ``BRPOP``, every job is handed to exactly one worker - so
each webhook is POSTed once, regardless of worker count. Without Redis (single
-worker dev) we deliver inline in a background task.

**Semantics.** At-least-once *intent*, best-effort in practice: a worker that
crashes mid-delivery loses that job (a Discord notification, not state), which is
an acceptable trade for not running a consumer-group/PEL machine. Discord 429s are
honoured (``Retry-After``); a 404/410 auto-disables the webhook (user deleted it);
repeated failures auto-disable after ``MAX_CONSECUTIVE_FAILURES``.
"""

from __future__ import annotations

import asyncio
import json
import logging

import httpx

from app.core.config import settings
from app.core.features import WEBHOOKS_FLAG, is_enabled
from app.core.redis import get_redis
from app.core.utils import utcnow
from app.webhooks import embeds
from app.webhooks.models import (
    MAX_CONSECUTIVE_FAILURES,
    WEBHOOK_EVENT_TYPES,
    SiteWebhook,
)

logger = logging.getLogger("kiwi.webhooks")

_worker: asyncio.Task | None = None

_POST_TIMEOUT = 10.0
_POST_ATTEMPTS = 3


# ── enqueue (called from the event bus) ──────────────────────────────────────

async def enqueue(payload: dict) -> None:
    """Queue one event for webhook fan-out. Safe to call for every event - it
    no-ops unless webhooks are enabled and the type is deliverable."""
    if payload.get("type") not in WEBHOOK_EVENT_TYPES:
        return
    if not await is_enabled(WEBHOOKS_FLAG):
        return
    redis = get_redis()
    if redis is None:
        # Dev / no Redis: deliver inline so the feature still works.
        asyncio.create_task(_deliver(payload))
        return
    try:
        await redis.lpush(settings.webhooks_queue, json.dumps(payload, default=str))
    except Exception:
        logger.warning("webhook enqueue failed", exc_info=True)


# ── delivery ─────────────────────────────────────────────────────────────────

async def _deliver(payload: dict) -> None:
    """POST the event to every active, subscribed webhook (each rendered with that
    webhook's own custom template, if it set one)."""
    event_type = payload.get("type")
    data = payload.get("data") or {}
    # Render once with no override to confirm the type is deliverable / cache nothing.
    if embeds.render(event_type, data) is None:
        return
    try:
        hooks = await SiteWebhook.find(
            SiteWebhook.active == True,                          # noqa: E712
            {"events": event_type},
        ).to_list()
    except Exception:
        logger.warning("webhook lookup failed (%s)", event_type, exc_info=True)
        return
    if not hooks:
        return
    await asyncio.gather(
        *(_deliver_one(h, event_type, data) for h in hooks), return_exceptions=True)


async def _deliver_one(hook: SiteWebhook, event_type: str, data: dict) -> None:
    tmpl = (hook.templates or {}).get(event_type)
    body = embeds.render(event_type, data, tmpl)
    if body is None:
        return
    design_id = tmpl.image_design_id if (tmpl and tmpl.enabled and tmpl.show_image) else None
    ok, status, error = await post_to_discord(hook.url, body, image_design_id=design_id)
    hook.last_status = status
    hook.updated_at = utcnow()
    if ok:
        hook.consecutive_failures = 0
        hook.last_error = None
        hook.last_delivered_at = utcnow()
    elif status in (404, 410):
        # The user deleted the Discord webhook - never coming back; disable it.
        hook.active = False
        hook.last_error = error
        hook.disabled_reason = f"Discord webhook was deleted (HTTP {status})."
    else:
        hook.consecutive_failures += 1
        hook.last_error = error
        if hook.consecutive_failures >= MAX_CONSECUTIVE_FAILURES:
            hook.active = False
            hook.disabled_reason = (
                f"Auto-disabled after {hook.consecutive_failures} failed deliveries."
            )
    try:
        await hook.save()
    except Exception:
        logger.warning("webhook bookkeeping save failed", exc_info=True)


def _retry_after(resp: httpx.Response) -> float:
    try:
        return min(float(resp.headers.get("Retry-After", "1")), 5.0)
    except (TypeError, ValueError):
        return 1.0


async def _render_attachment(image_design_id: str | None):
    """Render an Image Studio design to a multipart ``files`` arg (live data, baked in
    fresh each post), or None. Failure is non-fatal - the embed just goes without it."""
    if not image_design_id:
        return None
    try:
        from app.embed_templates import EMBED_IMAGE_ATTACHMENT
        from app.images import service as images
        png = await images.render_public(image_design_id)
        if png:
            return {"files[0]": (EMBED_IMAGE_ATTACHMENT, png, "image/png")}
    except Exception:
        logger.warning("webhook image render failed (%s)", image_design_id, exc_info=True)
    return None


async def post_to_discord(
    url: str, body: dict, *, image_design_id: str | None = None,
) -> tuple[bool, int | None, str | None]:
    """POST a webhook body to Discord. Returns ``(ok, http_status, error)``. When the
    template references an image design, render + upload it as a multipart attachment.

    Retries transient failures (429 with ``Retry-After``, 5xx, network errors);
    treats 4xx (other than 429) as permanent."""
    status: int | None = None
    error: str | None = None
    files = await _render_attachment(image_design_id)
    try:
        async with httpx.AsyncClient(timeout=_POST_TIMEOUT) as client:
            for attempt in range(_POST_ATTEMPTS):
                try:
                    if files:
                        r = await client.post(url, data={"payload_json": json.dumps(body)}, files=files)
                    else:
                        r = await client.post(url, json=body)
                except Exception as e:                       # network / timeout
                    status, error = None, str(e)[:200]
                    await asyncio.sleep(0.5 * (attempt + 1))
                    continue
                status = r.status_code
                if 200 <= status < 300:
                    return True, status, None
                if status == 429:
                    error = "rate limited by Discord"
                    if attempt < _POST_ATTEMPTS - 1:
                        await asyncio.sleep(_retry_after(r))
                        continue
                    return False, status, error
                if 400 <= status < 500:                      # 404/410/400 - permanent
                    return False, status, (r.text or "")[:200] or "rejected by Discord"
                error = (r.text or "")[:200] or f"HTTP {status}"   # 5xx - retry
                await asyncio.sleep(0.5 * (attempt + 1))
    except Exception as e:                                   # client construction, etc.
        return False, status, str(e)[:200]
    return False, status, error


# ── per-worker consumer loop ─────────────────────────────────────────────────

async def _consume() -> None:
    redis = get_redis()
    if redis is None:
        return
    while True:
        try:
            item = await redis.brpop([settings.webhooks_queue], timeout=5)
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("webhook BRPOP failed", exc_info=True)
            await asyncio.sleep(1)
            continue
        if item is None:
            continue                                         # timeout - just loop
        try:
            _key, raw = item
            await _deliver(json.loads(raw))
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("webhook delivery failed", exc_info=True)


def start_webhook_delivery() -> None:
    # The consumer is cheap (a blocking BRPOP loop) and the master switch is
    # runtime-flippable, so we always run it and gate at enqueue time instead.
    global _worker
    if _worker is None:
        _worker = asyncio.create_task(_consume())


async def stop_webhook_delivery() -> None:
    global _worker
    if _worker is not None:
        _worker.cancel()
        try:
            await _worker
        except asyncio.CancelledError:
            pass
    _worker = None
