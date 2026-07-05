"""CRUD + Discord-URL validation for outbound webhooks."""

from __future__ import annotations

from urllib.parse import urlparse

from beanie import PydanticObjectId

from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, utcnow
from app.embed_templates import EmbedTemplate, template_to_dict
from app.site_auth.models import SiteUser
from app.webhooks import embeds
from app.webhooks.models import WEBHOOK_EVENT_TYPES, SiteWebhook

# Discord webhook URLs only. This is the whole SSRF defence: we never POST to a
# host the user picked - just Discord's documented webhook endpoint.
_DISCORD_HOSTS = {
    "discord.com", "discordapp.com",
    "canary.discord.com", "ptb.discord.com",
}
MAX_WEBHOOKS_PER_USER = 15


def normalise_url(raw: str) -> str:
    """Validate a Discord webhook URL and return it normalised, or raise 400.

    Accepts ``https://(canary.|ptb.)?discord(app)?.com/api/webhooks/<id>/<token>``
    and nothing else."""
    url = (raw or "").strip()
    p = urlparse(url)
    if p.scheme != "https" or (p.hostname or "").lower() not in _DISCORD_HOSTS:
        raise APIError(400, ErrorCode.bad_request,
                       "URL must be an https Discord webhook (discord.com/api/webhooks/…).")
    parts = [seg for seg in p.path.split("/") if seg]
    # .../api/webhooks/<id>/<token>  (optionally a leading 'v10' etc.)
    if "webhooks" not in parts:
        raise APIError(400, ErrorCode.bad_request,
                       "That doesn't look like a Discord webhook URL.")
    i = parts.index("webhooks")
    if len(parts) < i + 3:                       # need an id AND a token segment
        raise APIError(400, ErrorCode.bad_request,
                       "Discord webhook URL is missing its id/token.")
    return f"https://{p.hostname.lower()}{p.path}"


def _clean_events(events: list[str] | None) -> list[str]:
    chosen = [e for e in (events or []) if e in WEBHOOK_EVENT_TYPES]
    if not chosen:
        raise APIError(400, ErrorCode.bad_request,
                       f"Pick at least one event: {', '.join(WEBHOOK_EVENT_TYPES)}.")
    # de-dupe, preserve the canonical order
    return [e for e in WEBHOOK_EVENT_TYPES if e in chosen]


def _clean_templates(
    templates: dict[str, EmbedTemplate] | None, events: list[str],
) -> dict[str, EmbedTemplate]:
    """Keep only templates for subscribed, valid event types."""
    out: dict[str, EmbedTemplate] = {}
    for key, tmpl in (templates or {}).items():
        if key in WEBHOOK_EVENT_TYPES and key in events and tmpl is not None:
            out[key] = tmpl
    return out


def event_meta() -> list[dict]:
    """Per-event editor metadata: the variables, the default template, and a sample
    context (for the variable palette + live preview)."""
    return [
        {
            "key": key,
            "variables": embeds.variables(key),
            "default_template": template_to_dict(embeds.default_template(key)),
            "sample": embeds.sample_context(key),
        }
        for key in WEBHOOK_EVENT_TYPES
    ]


def _mask_url(url: str) -> str:
    """Hide the secret token segment for display (the token authorises posting)."""
    p = urlparse(url)
    parts = p.path.split("/")
    if len(parts) >= 2 and len(parts[-1]) > 6:
        parts[-1] = parts[-1][:4] + "…" + parts[-1][-2:]
    return f"https://{p.hostname}{'/'.join(parts)}"


def _dto(w: SiteWebhook) -> dict:
    return {
        "id": str(w.id),
        "label": w.label,
        "url": _mask_url(w.url),                  # never echo the full token
        "events": w.events,
        "templates": {k: template_to_dict(v) for k, v in (w.templates or {}).items()},
        "active": w.active,
        "last_status": w.last_status,
        "last_error": w.last_error,
        "last_delivered_at": iso(w.last_delivered_at),
        "disabled_reason": w.disabled_reason,
        "created_at": w.created_at.isoformat(),
    }


async def _owned(actor: SiteUser, webhook_id: str) -> SiteWebhook:
    try:
        w = await SiteWebhook.get(PydanticObjectId(webhook_id))
    except Exception:
        w = None
    if w is None or w.owner_id != actor.id:
        raise APIError(404, ErrorCode.not_found, "Webhook not found.")
    return w


async def list_webhooks(actor: SiteUser) -> list[dict]:
    docs = await SiteWebhook.find(
        SiteWebhook.owner_id == actor.id,
    ).sort("-created_at").to_list()
    return [_dto(d) for d in docs]


async def create_webhook(
    actor: SiteUser, url: str, events: list[str], label: str,
    templates: dict[str, EmbedTemplate] | None = None,
) -> dict:
    count = await SiteWebhook.find(SiteWebhook.owner_id == actor.id).count()
    if count >= MAX_WEBHOOKS_PER_USER:
        raise APIError(400, ErrorCode.bad_request,
                       f"You can have at most {MAX_WEBHOOKS_PER_USER} webhooks.")
    clean_events = _clean_events(events)
    doc = SiteWebhook(
        owner_id=actor.id,
        url=normalise_url(url),
        events=clean_events,
        label=(label or "").strip()[:80],
        templates=_clean_templates(templates, clean_events),
    )
    await doc.insert()
    return _dto(doc)


async def update_webhook(
    actor: SiteUser, webhook_id: str, *,
    url: str | None = None, events: list[str] | None = None,
    label: str | None = None, active: bool | None = None,
    templates: dict[str, EmbedTemplate] | None = None,
) -> dict:
    w = await _owned(actor, webhook_id)
    if url is not None:
        w.url = normalise_url(url)
    if events is not None:
        w.events = _clean_events(events)
    if templates is not None:
        w.templates = _clean_templates(templates, w.events)
    elif events is not None:
        # events narrowed - drop templates for events no longer subscribed
        w.templates = _clean_templates(w.templates, w.events)
    if label is not None:
        w.label = label.strip()[:80]
    if active is not None:
        w.active = active
        if active:
            # Re-enabling clears the auto-disable bookkeeping for a fresh start.
            w.consecutive_failures = 0
            w.disabled_reason = None
    w.updated_at = utcnow()
    await w.save()
    return _dto(w)


async def delete_webhook(actor: SiteUser, webhook_id: str) -> None:
    w = await _owned(actor, webhook_id)
    await w.delete()


async def send_test(actor: SiteUser, webhook_id: str, event: str | None = None) -> dict:
    """Deliver a one-off test embed immediately and report the result. If ``event``
    is a valid type, render THAT event's embed (custom template if set, else default)
    with sample data so the user previews their customization in Discord; otherwise a
    generic "connected" message."""
    from app.webhooks import delivery
    w = await _owned(actor, webhook_id)
    design_id = None
    if event in WEBHOOK_EVENT_TYPES:
        tmpl = (w.templates or {}).get(event)
        body = embeds.render_sample(event, tmpl) or embeds.test_body()
        design_id = tmpl.image_design_id if (tmpl and tmpl.enabled and tmpl.show_image) else None
    else:
        body = embeds.test_body()
    ok, status, error = await delivery.post_to_discord(w.url, body, image_design_id=design_id)
    return {"ok": ok, "status": status, "error": error}
