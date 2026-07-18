"""Content moderation: the public notice-and-action report endpoint (DSA Art. 16),
master triage helpers, and the takedown "statement of reasons" DM (DSA Art. 17).

Reporting is open to ANYONE with no account and stores no reporter identity - only
what is reported and why. Takedowns notify the content owner over Discord (we no
longer store emails) with the reason and how to appeal.
"""
import logging
from typing import Literal

from fastapi import APIRouter
from pydantic import BaseModel, Field

from app.core.email_outbox import queue_email
from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, to_oid
from app.trove.modpacks.models import ModpackProject
from app.trove.mods_hub.models import ContentReport, ModProfile, ModProject

logger = logging.getLogger("kiwi.moderation")

router = APIRouter(prefix="/v1/moderation", tags=["moderation"])

_APPEAL_CONTACT = "support@aallyn.net"
_PUBLIC = ("public", "unlisted")   # visibilities a reporter could have reached


class ReportBody(BaseModel):
    target_type: Literal["mod", "modpack", "profile"]
    handle: str = Field(min_length=1, max_length=64)
    slug: str = Field(default="", max_length=140)   # unused for a profile report
    reason: str = Field(min_length=3, max_length=2000)


async def _resolve_target(target_type: str, handle: str, slug: str):
    """Locate the reported content -> (target_id, label, url), or None if it isn't
    publicly reachable (so drafts / unknown targets can't be reported)."""
    handle = handle.strip().lower()
    slug = slug.strip().lower()
    if target_type == "mod":
        p = await ModProject.find_one(
            ModProject.owner_handle == handle, ModProject.slug == slug
        )
        if p and p.visibility in _PUBLIC:
            return p.id, p.title, f"/mods/{handle}/{slug}"
    elif target_type == "modpack":
        p = await ModpackProject.find_one(
            ModpackProject.owner_handle == handle, ModpackProject.slug == slug
        )
        if p and p.visibility in _PUBLIC:
            return p.id, p.title, f"/modpacks/{handle}/{slug}"
    elif target_type == "profile":
        pr = await ModProfile.find_one(ModProfile.handle == handle)
        if pr:
            return pr.id, (pr.display_name or handle), f"/mods/{handle}"
    return None


@router.post("/report", status_code=202)
async def submit_report(body: ReportBody) -> dict:
    """File a report against a mod, modpack, or profile. No account required; we
    store only the target and the reason (never who reported it)."""
    target = await _resolve_target(body.target_type, body.handle, body.slug)
    if target is None:
        raise APIError(404, ErrorCode.not_found, "That content couldn't be found.")
    target_id, label, url = target
    await ContentReport(
        target_type=body.target_type,
        target_id=target_id,
        target_label=label or "",
        target_url=url,
        reason=body.reason.strip(),
    ).insert()
    return {"status": "received"}


# ── master triage (called from the admin panel) ────────────────────────────

async def list_reports(resolved: bool = False) -> list[dict]:
    reports = await ContentReport.find(
        ContentReport.resolved == resolved
    ).sort("-created_at").to_list()
    return [
        {
            "id": str(r.id), "target_type": r.target_type,
            "target_id": str(r.target_id), "target_label": r.target_label,
            "target_url": r.target_url, "reason": r.reason,
            "resolved": r.resolved, "created_at": iso(r.created_at),
        }
        for r in reports
    ]


async def dismiss_report(report_id: str) -> None:
    oid = to_oid(report_id)
    r = await ContentReport.get(oid) if oid else None
    if r is not None and not r.resolved:
        r.resolved = True
        await r.save()


async def resolve_reports_for(target_type: str, target_id) -> None:
    """Mark every open report against a target resolved (e.g. on takedown)."""
    from beanie.operators import Set
    await ContentReport.find(
        ContentReport.target_type == target_type,
        ContentReport.target_id == target_id,
        ContentReport.resolved == False,  # noqa: E712
    ).update(Set({ContentReport.resolved: True}))


# ── statement of reasons (DSA Art. 17) ─────────────────────────────────────

_KIND_LABEL = {"mod": "mod", "modpack": "modpack", "profile": "profile"}


async def notify_takedown(
    notify_email: str | None, kind: str, label: str, reason: str, url: str = ""
) -> None:
    """Email the content owner the statement of reasons + how to appeal - but ONLY
    if they added an opt-in notification address. Either way the reason is shown on
    the content page, so no email is not a miss. Best-effort."""
    if not notify_email:
        return
    what = _KIND_LABEL.get(kind, "content")
    reason = (reason or "Removed by a moderator.").strip()
    subject = f"Your {what} was removed on Better Trove Tools"
    text = "\n".join([
        f'Your {what} "{label}" has been removed by a moderator on Better Trove Tools.',
        f"Location: {url}" if url else "",
        "", f"Reason: {reason}", "",
        f"If you think this was a mistake, email {_APPEAL_CONTACT} and we'll take "
        "another look.",
        "- Better Trove Tools",
    ])
    loc_html = f"<p style='color:#9aa4b2'>{url}</p>" if url else ""
    html = (
        "<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;"
        "padding:24px;color:#e8ecf3;background:#0a0e14\">"
        "<h1 style='font-size:1.3rem;margin:0 0 12px'>Your content was removed</h1>"
        f"<p>Your {what} <strong>{label}</strong> has been removed by a moderator.</p>"
        f"{loc_html}"
        "<p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>"
        f"<p style='background:#161b22;border:1px solid #232a33;border-radius:8px;"
        f"padding:12px 14px'>{reason}</p>"
        f"<p style='margin-top:18px'>If you think this was a mistake, email "
        f"<a href='mailto:{_APPEAL_CONTACT}' style='color:#569cff'>{_APPEAL_CONTACT}</a> "
        "and we'll take another look.</p>"
        "<p style='color:#9aa4b2'>- Better Trove Tools</p></div>"
    )
    try:
        await queue_email(notify_email, subject, text, html)
    except Exception:
        logger.warning("takedown email failed for %s %s", kind, label, exc_info=True)
