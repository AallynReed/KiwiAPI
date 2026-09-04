"""File-drop rules + storage.

A drop is open while all three of these hold: it hasn't been revoked, it hasn't
expired, and it hasn't used up its uploads. The upload slot is claimed with one
atomic ``$inc`` guarded by those same conditions, so two people who hit the
button at the same moment on a one-time link can't both get through - the loser
is told the link is spent rather than quietly overwriting the winner.

Files live on disk under ``<drops_store_dir>/<slug>/``, streamed in chunks (a
friend sending a 200 MB video shouldn't cost 200 MB of API memory) and never
served to anyone but a master.
"""
import asyncio
import hashlib
import logging
import os
import re
import secrets
import shutil
from datetime import timedelta
from pathlib import Path

from beanie import PydanticObjectId
from beanie.operators import In
from fastapi import UploadFile

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.security import hash_password, verify_password
from app.core.utils import utcnow
from app.drops.models import DropUpload, FileDrop
from app.drops.schemas import (
    DropCreate,
    DropCreated,
    DropPublicView,
    DropUpdate,
    DropUploadView,
    DropView,
)

logger = logging.getLogger("kiwi.drops")

_CHUNK = 1024 * 1024          # 1 MB - the streaming read size
_SLUG_BYTES = 12              # -> 16 url-safe chars


def _root() -> Path:
    return Path(settings.drops_store_dir)


def _drop_dir(slug: str) -> Path:
    return _root() / slug


# Anything that isn't plainly a filename is replaced. The stored name is
# generated separately, so this only has to be good enough to show and to hand
# back as a download name - it never becomes a path on its own.
_UNSAFE = re.compile(r"[^A-Za-z0-9._ ()\[\]-]+")


def safe_filename(name: str | None) -> str:
    name = (name or "").strip().replace("\\", "/").rsplit("/", 1)[-1]
    name = _UNSAFE.sub("_", name).strip(" .")
    return name[:120] or "upload.bin"


def _upload_view(u: DropUpload) -> DropUploadView:
    return DropUploadView(
        id=str(u.id), filename=u.filename, size=u.size,
        content_type=u.content_type, sha256=u.sha256, note=u.note,
        uploaded_at=u.uploaded_at,
    )


def _view(drop: FileDrop, uploads: list[DropUpload] | None = None) -> DropView:
    return DropView(
        id=str(drop.id),
        slug=drop.slug,
        url=f"{settings.app_url.rstrip('/')}/drop/{drop.slug}",
        label=drop.label,
        max_uploads=drop.max_uploads,
        upload_count=drop.upload_count,
        max_file_bytes=drop.max_file_bytes,
        expires_at=drop.expires_at,
        revoked=drop.revoked,
        open=is_open(drop),
        created_at=drop.created_at,
        uploads=[_upload_view(u) for u in (uploads or [])],
    )


def is_open(drop: FileDrop) -> bool:
    return (
        not drop.revoked
        and drop.expires_at > utcnow()
        and drop.upload_count < drop.max_uploads
    )


# -- Master side ------------------------------------------------------------

async def create(req: DropCreate) -> DropCreated:
    """Mint a link. The PIN comes back in this response and nowhere else."""
    drop = FileDrop(
        slug=secrets.token_urlsafe(_SLUG_BYTES),
        label=req.label.strip(),
        pin_hash=hash_password(req.pin),
        max_uploads=req.max_uploads,
        max_file_bytes=min(req.max_file_mb * 1024 * 1024,
                           settings.drops_max_request_body_bytes),
        expires_at=utcnow() + timedelta(hours=req.expires_in_hours),
    )
    await drop.insert()
    return DropCreated(**_view(drop).model_dump(), pin=req.pin)


async def list_drops() -> list[DropView]:
    """Every drop, newest first, each with the files it has received."""
    drops = await FileDrop.find_all().sort(-FileDrop.created_at).to_list()
    if not drops:
        return []
    uploads = await DropUpload.find(
        In(DropUpload.drop_id, [d.id for d in drops])
    ).sort(-DropUpload.uploaded_at).to_list()
    by_drop: dict[PydanticObjectId, list[DropUpload]] = {}
    for u in uploads:
        by_drop.setdefault(u.drop_id, []).append(u)
    return [_view(d, by_drop.get(d.id, [])) for d in drops]


async def _get(drop_id: str) -> FileDrop:
    drop = await FileDrop.get(drop_id)
    if drop is None:
        raise APIError(404, ErrorCode.not_found, "That drop doesn't exist.")
    return drop


async def update(drop_id: str, req: DropUpdate) -> DropView:
    drop = await _get(drop_id)
    if req.label is not None:
        drop.label = req.label.strip()
    if req.max_uploads is not None:
        drop.max_uploads = req.max_uploads
    if req.extend_hours is not None:
        # Extending an ALREADY-expired drop gives the full window from now rather
        # than tacking hours onto a date in the past that leaves it expired anyway.
        drop.expires_at = max(drop.expires_at, utcnow()) + timedelta(hours=req.extend_hours)
    if req.revoked is not None:
        drop.revoked = req.revoked
    await drop.save()
    uploads = await DropUpload.find(DropUpload.drop_id == drop.id).sort(
        -DropUpload.uploaded_at).to_list()
    return _view(drop, uploads)


async def delete(drop_id: str) -> None:
    """Delete the drop, its upload rows and every file it received."""
    drop = await _get(drop_id)
    await DropUpload.find(DropUpload.drop_id == drop.id).delete()
    shutil.rmtree(_drop_dir(drop.slug), ignore_errors=True)
    await drop.delete()


async def delete_upload(upload_id: str) -> None:
    """Delete one received file. The drop's used-up count is deliberately left
    alone - deleting the file you were sent is not the same as handing the
    sender another go at the link."""
    upload = await DropUpload.get(upload_id)
    if upload is None:
        raise APIError(404, ErrorCode.not_found, "That file doesn't exist.")
    drop = await FileDrop.get(upload.drop_id)
    if drop is not None:
        (_drop_dir(drop.slug) / upload.stored_name).unlink(missing_ok=True)
    await upload.delete()


async def upload_path(upload_id: str) -> tuple[DropUpload, Path]:
    """The on-disk file behind an upload row, for the master-only download."""
    upload = await DropUpload.get(upload_id)
    if upload is None:
        raise APIError(404, ErrorCode.not_found, "That file doesn't exist.")
    drop = await FileDrop.get(upload.drop_id)
    path = _drop_dir(drop.slug) / upload.stored_name if drop else None
    if path is None or not path.is_file():
        raise APIError(404, ErrorCode.not_found, "That file is no longer on disk.")
    return upload, path


# -- Uploader side ----------------------------------------------------------

async def by_slug(slug: str) -> FileDrop:
    """Resolve a link. A spent, revoked, expired or unknown slug all answer the
    same 404 - which one it is isn't the visitor's business, and telling them
    turns the slug into an oracle."""
    drop = await FileDrop.find_one(FileDrop.slug == slug)
    if drop is None or not is_open(drop):
        raise APIError(404, ErrorCode.not_found,
                       "This upload link isn't active - it may have expired, "
                       "been used already, or been turned off.")
    return drop


def public_view(drop: FileDrop) -> DropPublicView:
    return DropPublicView(
        label=drop.label,
        max_file_bytes=drop.max_file_bytes,
        uploads_left=max(0, drop.max_uploads - drop.upload_count),
        expires_at=drop.expires_at,
    )


def check_pin(drop: FileDrop, pin: str) -> None:
    if not pin or not verify_password(pin, drop.pin_hash):
        raise APIError(403, ErrorCode.forbidden, "That PIN isn't right.")


async def _claim_slot(drop: FileDrop) -> bool:
    """Take one of the drop's uploads, atomically. False if there was none left.

    The guard is the whole open-ness test, so the claim and the check can't drift
    apart under concurrency: the update only applies to a document that is still
    un-revoked, unexpired and under budget."""
    result = await FileDrop.get_motor_collection().find_one_and_update(
        {
            "_id": drop.id,
            "revoked": False,
            "expires_at": {"$gt": utcnow()},
            "$expr": {"$lt": ["$upload_count", "$max_uploads"]},
        },
        {"$inc": {"upload_count": 1}},
    )
    return result is not None


async def _release_slot(drop: FileDrop) -> None:
    await FileDrop.get_motor_collection().update_one(
        {"_id": drop.id}, {"$inc": {"upload_count": -1}})


def _write_stream(src, dest: Path, limit: int) -> tuple[int, str]:
    """Copy an upload to disk in chunks, hashing as it goes and stopping dead the
    moment it goes over the drop's cap. Sync - call it in a thread."""
    dest.parent.mkdir(parents=True, exist_ok=True)
    digest = hashlib.sha256()
    size = 0
    with open(dest, "wb") as out:
        while True:
            chunk = src.read(_CHUNK)
            if not chunk:
                break
            size += len(chunk)
            if size > limit:
                raise ValueError("over the per-file limit")
            digest.update(chunk)
            out.write(chunk)
        out.flush()
        os.fsync(out.fileno())
    return size, digest.hexdigest()


async def receive(drop: FileDrop, file: UploadFile, note: str | None) -> DropUploadView:
    """Store one uploaded file against a drop.

    The slot is claimed BEFORE the bytes are written, so a link can't be used
    twice by two uploads racing each other; if the write then fails, the slot is
    handed back so a genuine failure doesn't burn someone's only attempt."""
    filename = safe_filename(file.filename)
    if not await _claim_slot(drop):
        raise APIError(409, ErrorCode.conflict,
                       "This link was just used up. Ask for a new one.")

    stored_name = f"{secrets.token_hex(8)}-{filename}"
    dest = _drop_dir(drop.slug) / stored_name
    try:
        size, sha = await asyncio.to_thread(
            _write_stream, file.file, dest, drop.max_file_bytes)
    except ValueError:
        dest.unlink(missing_ok=True)
        await _release_slot(drop)
        mb = drop.max_file_bytes // (1024 * 1024)
        raise APIError(413, ErrorCode.bad_request,
                       f"That file is bigger than this link's {mb} MB limit.") from None
    except OSError:
        dest.unlink(missing_ok=True)
        await _release_slot(drop)
        logger.exception("file drop write failed (slug=%s)", drop.slug)
        raise APIError(503, ErrorCode.service_unavailable,
                       "The file couldn't be saved. Try again in a moment.") from None

    if size == 0:
        dest.unlink(missing_ok=True)
        await _release_slot(drop)
        raise APIError(400, ErrorCode.bad_request, "That file is empty.")

    upload = DropUpload(
        drop_id=drop.id,
        filename=filename,
        stored_name=stored_name,
        size=size,
        content_type=(file.content_type or None),
        sha256=sha,
        note=(note or "").strip()[:500] or None,
    )
    await upload.insert()
    logger.info("file drop received %s (%d bytes) on %s", filename, size, drop.slug)
    return _upload_view(upload)
