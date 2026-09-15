"""Custom art requests: intake, the review queue, and the emails that answer them.

Nothing is kept but what was asked for, the picture, and the address the answer
goes to. The name rules mirror TroveUI's ``custom_art.py``: the name becomes the
picture's file name there, so anything that cannot be one is refused here, where
the person asking can still fix it.
"""
import html
import logging
from collections import Counter

from email_validator import EmailNotValidError, validate_email

from app.auth.disposable import is_disposable_email
from app.core.captcha import verify_captcha
from app.core.config import settings
from app.core.email_outbox import queue_email
from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, to_oid, utcnow
from app.custom_art.models import ArtRequest
from app.trove.mods_hub import store

logger = logging.getLogger("kiwi.custom_art")

CHAT = "Zakros UI - Chat"
NAMEPLATE = "Zakros UI - Nameplate"
MODS = {"pfp": (CHAT,), "club": (CHAT,), "banner": (CHAT, NAMEPLATE)}
LABELS = {"pfp": "profile picture", "club": "club picture", "banner": "club banner"}
CHANGELOG_MAX = 240

_BANNED = set('\t\n\r",\\/:*?<>|')


def clean_name(name: str | None) -> str:
    name = (name or "").strip()
    if not name or len(name) > 64 or name.endswith(".") or _BANNED & set(name):
        raise APIError(400, ErrorCode.validation_error,
                       'That name can\'t be used. Names are up to 64 characters, can\'t end '
                       'in a full stop, and can\'t contain , " \\ / : * ? < > or |.')
    return name


def clean_email(email: str | None) -> str:
    try:
        address = validate_email((email or "").strip(), check_deliverability=False).normalized
    except EmailNotValidError:
        raise APIError(400, ErrorCode.validation_error,
                       "That email address doesn't look right.") from None
    if is_disposable_email(address):
        raise APIError(400, ErrorCode.disposable_email,
                       "Use an address you'll still have in a few days - the answer goes there.")
    return address


def _join(parts: list[str]) -> str:
    return parts[0] if len(parts) == 1 else ", ".join(parts[:-1]) + " and " + parts[-1]


def changelog(requests: list[ArtRequest]) -> str:
    """What a player reads in the update list. Every picture by name when that fits,
    otherwise a count per kind."""
    text = "Added " + _join([f"a {LABELS[r.kind]} for {r.name}" for r in requests]) + "."
    if len(text) <= CHANGELOG_MAX:
        return text
    counts = Counter(r.kind for r in requests)
    return "Added " + _join([f"{n} {LABELS[k]}{'' if n == 1 else 's'}"
                             for k, n in counts.items()]) + "."


def view(r: ArtRequest) -> dict:
    return {
        "id": str(r.id), "kind": r.kind, "label": LABELS[r.kind], "name": r.name,
        "email": r.email, "note": r.note, "width": r.width, "height": r.height,
        "mods": list(MODS[r.kind]), "status": r.status, "reason": r.reason,
        "log": r.log, "versions": r.versions, "steam_pending": r.steam_pending,
        "created_at": iso(r.created_at), "decided_at": iso(r.decided_at),
        "released_at": iso(r.released_at),
    }


# -- Intake -------------------------------------------------------------------

async def submit(*, kind: str, name: str, email: str, note: str | None, data: bytes,
                 captcha_token: str | None, ip: str | None) -> ArtRequest:
    """Everything a person can correct is checked before the captcha, because a
    captcha token is spent the moment it is verified."""
    name = clean_name(name)
    address = clean_email(email)
    if len(data) > settings.mods_image_max_bytes:
        mb = settings.mods_image_max_bytes // (1024 * 1024)
        raise APIError(413, ErrorCode.bad_request, f"That picture is over the {mb} MB limit.")
    sniffed = store.sniff_image(data)
    if sniffed is None:
        raise APIError(400, ErrorCode.bad_request, "Send a PNG, JPEG, WebP or GIF picture.")
    if not await verify_captcha(captcha_token, ip):
        raise APIError(400, ErrorCode.captcha_failed, "The captcha check didn't pass. Try it again.")
    content_type, width, height = sniffed
    sha, _ = await store.put_blob(data)
    request = ArtRequest(kind=kind, name=name, email=address,
                         note=(note or "").strip()[:500] or None, image_sha=sha,
                         content_type=content_type, width=width, height=height)
    await request.insert()
    logger.info("custom art request %s: %s %r", request.id, kind, name)
    return request


# -- Review -------------------------------------------------------------------

async def list_requests() -> list[dict]:
    rows = await ArtRequest.find_all().sort(-ArtRequest.created_at).limit(300).to_list()
    return [view(r) for r in rows]


async def _get(request_id: str) -> ArtRequest:
    oid = to_oid(request_id)
    request = await ArtRequest.get(oid) if oid else None
    if request is None:
        raise APIError(404, ErrorCode.not_found, "That request doesn't exist.")
    return request


async def image(request_id: str) -> tuple[bytes, str]:
    request = await _get(request_id)
    data = await store.get_blob(request.image_sha)
    if data is None:
        raise APIError(404, ErrorCode.not_found, "That picture is missing from the store.")
    return data, request.content_type


async def approve(request_id: str, name: str | None) -> dict:
    request = await _get(request_id)
    if request.status != "pending":
        raise APIError(409, ErrorCode.conflict, "Only a pending request can be approved.")
    if name is not None:
        request.name = clean_name(name)
    request.status = "approved"
    request.decided_at = utcnow()
    await request.save()
    return view(request)


async def deny(request_id: str, reason: str) -> dict:
    request = await _get(request_id)
    if request.status not in ("pending", "approved"):
        raise APIError(409, ErrorCode.conflict, "That request is already past review.")
    request.status = "denied"
    request.reason = reason.strip()
    request.decided_at = utcnow()
    await request.save()
    await _notify_denied(request)
    return view(request)


async def release() -> int:
    """Hand every approved request to the worker as one batch."""
    result = await ArtRequest.get_pymongo_collection().update_many(
        {"status": "approved"}, {"$set": {"status": "queued", "log": None}})
    return result.modified_count


async def retry(request_id: str) -> int:
    """Put a failed request back in the queue, with the rest of its batch."""
    request = await _get(request_id)
    if request.status != "failed":
        raise APIError(409, ErrorCode.conflict, "Only a failed request can be retried.")
    result = await ArtRequest.get_pymongo_collection().update_many(
        {"status": "failed", "batch": request.batch}, {"$set": {"status": "queued"}})
    return result.modified_count


async def steam_done() -> int:
    result = await ArtRequest.get_pymongo_collection().update_many(
        {"steam_pending": True}, {"$set": {"steam_pending": False}})
    return result.modified_count


# -- Email --------------------------------------------------------------------

def _wrap(title: str, body: str) -> str:
    return ("<div style=\"font-family:system-ui,sans-serif;max-width:480px;margin:0 auto;"
            "padding:24px;color:#e8ecf3;background:#0a0e14\">"
            f"<h1 style='font-size:1.3rem;margin:0 0 12px'>{html.escape(title)}</h1>"
            f"{body}<p style='color:#9aa4b2'>- Better Trove Tools</p></div>")


async def _send(to: str, subject: str, text: str, body: str) -> None:
    try:
        await queue_email(to, subject, text, _wrap(subject, body))
    except Exception:
        logger.warning("custom art email to %s failed", to, exc_info=True)


async def _notify_denied(request: ArtRequest) -> None:
    what = f"{LABELS[request.kind]} for {request.name}"
    subject = f"Your {LABELS[request.kind]} request wasn't added"
    text = "\n".join([
        f"Your request for a {what} in Zakros UI wasn't added.", "",
        f"Reason: {request.reason}", "",
        "You're welcome to send a new request with that sorted out.",
        f"{settings.app_url.rstrip('/')}/custom-art", "",
        "- Better Trove Tools",
    ])
    body = (f"<p>Your request for a {html.escape(what)} in Zakros UI wasn't added.</p>"
            "<p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>"
            "<p style='background:#161b22;border:1px solid #232a33;border-radius:8px;"
            f"padding:12px 14px'>{html.escape(request.reason or '')}</p>"
            "<p>You're welcome to send a new request with that sorted out.</p>")
    await _send(request.email, subject, text, body)


async def notify_released(request: ArtRequest, pages: dict[str, str]) -> None:
    what = f"{LABELS[request.kind]} for {request.name}"
    subject = f"Your {LABELS[request.kind]} is in Zakros UI"
    shipped = [f"{mod} {request.versions[mod]}: {pages.get(mod, '')}".rstrip(": ")
               for mod in MODS[request.kind]]
    text = "\n".join([f"Your {what} has been added. It's in:", "", *shipped, "",
                      "Update the mod to see it.", "", "- Better Trove Tools"])
    items = "".join(
        f"<li>{html.escape(mod)} {html.escape(request.versions[mod])}"
        + (f" - <a href='{html.escape(pages[mod])}' style='color:#569cff'>mod page</a>"
           if pages.get(mod) else "") + "</li>"
        for mod in MODS[request.kind])
    body = (f"<p>Your {html.escape(what)} has been added. It's in:</p><ul>{items}</ul>"
            "<p>Update the mod to see it.</p>")
    await _send(request.email, subject, text, body)
