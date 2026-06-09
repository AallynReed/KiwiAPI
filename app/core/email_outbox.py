"""Durable email outbox with retry + bounce handling.

Mail is written to a Mongo-backed outbox and delivered by a background worker, so
request latency never depends on SMTP and transient failures are retried with
exponential backoff. A hard 5xx rejection is treated as a bounce: the record is
marked ``bounced`` and the recipient (if a user) is flagged so we stop mailing a
dead address. (Asynchronous DSN/bounce-inbox processing is a mail-server concern
and out of scope here - this handles synchronous SMTP rejections.)
"""

import asyncio
import logging
from datetime import datetime, timedelta
from typing import Literal

from beanie import Document
from pydantic import Field
from pymongo import ASCENDING, IndexModel, ReturnDocument

from app.auth.models import User
from app.core.config import settings
from app.core.mailer import EmailDeliveryError, deliver_email
from app.core.utils import utcnow

# NOTE: get_db is imported lazily inside _claim_next to avoid an import cycle
# (database.py registers OutboxEmail, so it imports this module at load time).

logger = logging.getLogger("kiwi.email")

EmailStatus = Literal["pending", "sending", "sent", "bounced", "failed"]
_RECLAIM_AFTER = timedelta(minutes=5)  # un-stick a "sending" doc a dead worker left


class OutboxEmail(Document):
    to: str
    subject: str
    text: str
    html: str | None = None

    status: EmailStatus = "pending"
    attempts: int = 0
    last_error: str | None = None
    next_attempt_at: datetime = Field(default_factory=utcnow)
    claimed_at: datetime | None = None

    created_at: datetime = Field(default_factory=utcnow)
    sent_at: datetime | None = None

    class Settings:
        name = "email_outbox"
        indexes = [
            IndexModel([("status", ASCENDING), ("next_attempt_at", ASCENDING)]),
        ]


async def queue_email(to: str, subject: str, text: str, html: str | None = None) -> None:
    """Queue a message for the worker. No-op (logged) when SMTP isn't configured,
    and skipped for addresses we've already seen hard-bounce."""
    if not settings.email_enabled:
        logger.warning("SMTP not configured - not queueing email to %s (%s)", to, subject)
        logger.info("Email body (would send):\n%s", text)
        return

    bounced = await User.find_one(User.email == to.lower(), User.email_bounced == True)  # noqa: E712
    if bounced is not None:
        logger.warning("Skipping email to bounced address %s (%s)", to, subject)
        return

    await OutboxEmail(to=to, subject=subject, text=text, html=html).insert()


def _backoff(attempts: int) -> timedelta:
    return timedelta(seconds=settings.email_retry_base_seconds * (2 ** (attempts - 1)))


async def _claim_next() -> OutboxEmail | None:
    """Atomically take the next due message (lock via status='sending').

    The conditional update means only one worker can flip a given record from
    pending→sending, so concurrent workers never double-send. We then re-fetch
    the locked record through Beanie for a fully-typed document to update.
    """
    from app.core.database import get_db  # lazy: avoids a load-time import cycle

    now = utcnow()
    reclaim_before = now - _RECLAIM_AFTER
    doc = await get_db()["email_outbox"].find_one_and_update(
        {
            "$or": [
                {"status": "pending", "next_attempt_at": {"$lte": now}},
                {"status": "sending", "claimed_at": {"$lte": reclaim_before}},
            ]
        },
        {"$set": {"status": "sending", "claimed_at": now}},
        sort=[("next_attempt_at", ASCENDING)],
        return_document=ReturnDocument.AFTER,
    )
    if doc is None:
        return None
    return await OutboxEmail.get(doc["_id"])


async def _process_one(msg: OutboxEmail) -> None:
    try:
        await deliver_email(msg.to, msg.subject, msg.text, msg.html)
    except EmailDeliveryError as exc:
        await _handle_failure(msg, exc)
        return
    msg.status = "sent"
    msg.sent_at = utcnow()
    msg.attempts += 1
    await msg.save()


async def _handle_failure(msg: OutboxEmail, exc: EmailDeliveryError) -> None:
    msg.attempts += 1
    msg.last_error = str(exc)[:500]

    if exc.permanent:
        msg.status = "bounced"
        await msg.save()
        await _flag_bounced(msg.to)
        logger.warning("Email to %s bounced: %s", msg.to, exc)
        return

    if msg.attempts >= settings.email_max_attempts:
        msg.status = "failed"
        await msg.save()
        logger.error("Email to %s failed after %d attempts: %s", msg.to, msg.attempts, exc)
        return

    msg.status = "pending"
    msg.next_attempt_at = utcnow() + _backoff(msg.attempts)
    await msg.save()
    logger.info("Email to %s retry %d scheduled: %s", msg.to, msg.attempts, exc)


async def _flag_bounced(to: str) -> None:
    user = await User.find_one(User.email == to.lower())
    if user is not None and not user.email_bounced:
        user.email_bounced = True
        await user.save()


_worker_task: asyncio.Task | None = None


async def _worker_loop() -> None:
    while True:
        try:
            processed = 0
            while (msg := await _claim_next()) is not None:
                await _process_one(msg)
                processed += 1
                if processed >= 50:  # yield between large bursts
                    break
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("Email worker iteration failed")
        try:
            await asyncio.sleep(settings.email_worker_poll_seconds)
        except asyncio.CancelledError:
            raise


def start_email_worker() -> None:
    global _worker_task
    if _worker_task is None:
        _worker_task = asyncio.create_task(_worker_loop())


async def stop_email_worker() -> None:
    global _worker_task
    if _worker_task is not None:
        _worker_task.cancel()
        try:
            await _worker_task
        except asyncio.CancelledError:
            pass
        _worker_task = None
