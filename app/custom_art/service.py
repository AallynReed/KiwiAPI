"""Custom art requests: intake, the review queue, and the emails that answer them.

A player request carries a profile picture. A club request carries a club picture,
a club banner, or both. A profile or club picture is square; a banner is from
square up to five times as wide as it is tall. The page crops to those shapes and
this checks them again, so nothing of the wrong shape reaches the queue.

Nothing is kept but what was asked for, the pictures, and the address the answer
goes to. The name rules mirror TroveUI's ``custom_art.py``: the name becomes the
picture's file name there, so anything that cannot be one is refused here, where
the person asking can still fix it.
"""
import asyncio
import html
import io
import logging
from collections import Counter

from email_validator import EmailNotValidError, validate_email
from PIL import Image

from app.auth.disposable import is_disposable_email
from app.core.captcha import verify_captcha
from app.core.config import settings
from app.core.email_outbox import queue_email
from app.core.errors import APIError, ErrorCode
from app.core.utils import iso, to_oid, utcnow
from app.custom_art.models import ArtPicture, ArtRequest
from app.trove.mods_hub import store

logger = logging.getLogger("kiwi.custom_art")

CHAT = "Zakros UI - Chat"
NAMEPLATE = "Zakros UI - Nameplate"
SLOTS = {"player": ("pfp",), "club": ("pfp", "banner")}
LANES = {("player", "pfp"): "pfp", ("club", "pfp"): "club", ("club", "banner"): "banner"}
LANE_MODS = {"pfp": (CHAT,), "club": (CHAT,), "banner": (CHAT, NAMEPLATE)}
LANE_LABELS = {"pfp": "profile picture", "club": "club picture", "banner": "club banner"}
WIDEST = 5
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


def check_shape(kind: str, slot: str, width: int, height: int) -> None:
    label = LANE_LABELS[LANES[(kind, slot)]]
    if slot == "pfp" and width != height:
        raise APIError(400, ErrorCode.validation_error,
                       f"A {label} has to be square - that one is {width}×{height}.")
    if slot == "banner" and not height <= width <= WIDEST * height:
        raise APIError(400, ErrorCode.validation_error,
                       f"A {label} has to be from square up to {WIDEST} times as wide as it is "
                       f"tall - that one is {width}×{height}.")


def lanes(request) -> list[str]:
    return [LANES[(request.kind, slot)] for slot in SLOTS[request.kind] if slot in request.pictures]


def mods_of(request) -> list[str]:
    return [mod for mod in (CHAT, NAMEPLATE) if any(mod in LANE_MODS[lane] for lane in lanes(request))]


def what(request, mod: str | None = None) -> str:
    """The pictures a request carries, in words - only those that ship in ``mod``."""
    shown = [lane for lane in lanes(request) if mod is None or mod in LANE_MODS[lane]]
    if shown == ["club", "banner"]:
        return "club picture and banner"
    return LANE_LABELS[shown[0]]


def _join(parts: list[str]) -> str:
    return parts[0] if len(parts) == 1 else ", ".join(parts[:-1]) + " and " + parts[-1]


def changelog(requests: list, mod: str) -> str:
    """What a player reads in ``mod``'s update list. Every request by name when that
    fits, otherwise a count per kind of picture."""
    mine = [r for r in requests if mod in mods_of(r)]
    text = "Added " + _join([f"a {what(r, mod)} for {r.name}" for r in mine]) + "."
    if len(text) <= CHANGELOG_MAX:
        return text
    counts = Counter(lane for r in mine for lane in lanes(r) if mod in LANE_MODS[lane])
    return "Added " + _join([f"{n} {LANE_LABELS[lane]}{'' if n == 1 else 's'}"
                             for lane, n in counts.items()]) + "."


def view(r: ArtRequest) -> dict:
    return {
        "id": str(r.id), "kind": r.kind, "label": what(r), "name": r.name,
        "email": r.email, "note": r.note, "mods": mods_of(r),
        "pictures": {slot: {"width": p.width, "height": p.height} for slot, p in r.pictures.items()},
        "status": r.status, "reason": r.reason, "log": r.log, "versions": r.versions,
        "steam_pending": r.steam_pending, "created_at": iso(r.created_at),
        "decided_at": iso(r.decided_at), "released_at": iso(r.released_at),
    }


# -- Intake -------------------------------------------------------------------

def _measure(data: bytes) -> tuple[str, int, int] | None:
    sniffed = store.sniff_image(data)
    if sniffed is None:
        return None
    try:
        with Image.open(io.BytesIO(data)) as im:
            return sniffed[0], im.width, im.height
    except Exception:
        return None


async def submit(*, kind: str, name: str, email: str, note: str | None,
                 files: dict[str, bytes], captcha_token: str | None,
                 ip: str | None) -> ArtRequest:
    """Everything a person can correct is checked before the captcha, because a
    captcha token is spent the moment it is verified."""
    name = clean_name(name)
    address = clean_email(email)
    files = {slot: data for slot, data in files.items() if data}
    if not files:
        raise APIError(400, ErrorCode.validation_error,
                       "Add a profile picture." if kind == "player"
                       else "Add a club picture, a club banner, or both.")
    if set(files) - set(SLOTS[kind]):
        raise APIError(400, ErrorCode.validation_error,
                       "A player request carries a profile picture only.")
    limit = settings.custom_art_image_max_bytes
    measured = {}
    for slot, data in files.items():
        label = LANE_LABELS[LANES[(kind, slot)]]
        if len(data) > limit:
            raise APIError(413, ErrorCode.bad_request,
                           f"The {label} is over the {limit // (1024 * 1024)} MB limit.")
        found = await asyncio.to_thread(_measure, data)
        if found is None:
            raise APIError(400, ErrorCode.bad_request,
                           f"The {label} has to be a PNG, JPEG, WebP or GIF picture.")
        check_shape(kind, slot, found[1], found[2])
        measured[slot] = found
    if not await verify_captcha(captcha_token, ip):
        raise APIError(400, ErrorCode.captcha_failed, "The captcha check didn't pass. Try it again.")
    pictures = {}
    for slot, (content_type, width, height) in measured.items():
        sha, _ = await store.put_blob(files[slot])
        pictures[slot] = ArtPicture(sha=sha, content_type=content_type, width=width, height=height)
    request = ArtRequest(kind=kind, name=name, email=address,
                         note=(note or "").strip()[:500] or None, pictures=pictures)
    await request.insert()
    logger.info("custom art request %s: %s %r (%s)", request.id, kind, name, ", ".join(pictures))
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


async def image(request_id: str, slot: str) -> tuple[bytes, str]:
    request = await _get(request_id)
    picture = request.pictures.get(slot)
    data = await store.get_blob(picture.sha) if picture else None
    if data is None:
        raise APIError(404, ErrorCode.not_found, "That picture isn't there.")
    return data, picture.content_type


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
    asked = f"{what(request)} for {request.name}"
    subject = "Your custom art request wasn't added"
    text = "\n".join([
        f"Your request for a {asked} in Zakros UI wasn't added.", "",
        f"Reason: {request.reason}", "",
        "You're welcome to send a new request with that sorted out.",
        f"{settings.app_url.rstrip('/')}/zakros-ui-requests", "",
        "- Better Trove Tools",
    ])
    body = (f"<p>Your request for a {html.escape(asked)} in Zakros UI wasn't added.</p>"
            "<p style='margin:16px 0 4px;color:#9aa4b2'>Reason</p>"
            "<p style='background:#161b22;border:1px solid #232a33;border-radius:8px;"
            f"padding:12px 14px'>{html.escape(request.reason or '')}</p>"
            "<p>You're welcome to send a new request with that sorted out.</p>")
    await _send(request.email, subject, text, body)


async def notify_released(request: ArtRequest, pages: dict[str, str]) -> None:
    asked = f"{what(request)} for {request.name}"
    subject = "Your custom art is in Zakros UI"
    mods = mods_of(request)
    shipped = [f"{mod} {request.versions[mod]}" + (f": {pages[mod]}" if pages.get(mod) else "")
               for mod in mods]
    text = "\n".join([f"Your {asked} has been added. It's in:", "", *shipped, "",
                      "Update the mod to see it.", "", "- Better Trove Tools"])
    items = "".join(
        f"<li>{html.escape(mod)} {html.escape(request.versions[mod])}"
        + (f" - <a href='{html.escape(pages[mod])}' style='color:#569cff'>mod page</a>"
           if pages.get(mod) else "") + "</li>"
        for mod in mods)
    body = (f"<p>Your {html.escape(asked)} has been added. It's in:</p><ul>{items}</ul>"
            "<p>Update the mod to see it.</p>")
    await _send(request.email, subject, text, body)
