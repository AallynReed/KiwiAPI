"""Shared delivery-health bookkeeping for the outbound-notification features.

Both webhooks and DM subscriptions track the same per-target health fields and
apply the same rule after each delivery attempt: success resets the failure
streak; a permanent rejection disables the target for good; anything else counts
toward an auto-disable at ``MAX_CONSECUTIVE_FAILURES``. Only the permanent-reject
status codes and the human-readable disable reasons differ per feature, so those
are passed in by the caller.
"""

from __future__ import annotations

from datetime import datetime

from pydantic import BaseModel

from app.core.utils import utcnow

# Auto-disable a target after this many consecutive failed deliveries, so a dead
# endpoint isn't retried forever (GitHub-style).
MAX_CONSECUTIVE_FAILURES = 12


class DeliveryHealth(BaseModel):
    """Beanie-document mixin holding the delivery-health fields, updated after
    each attempt by :func:`record_delivery`."""

    active: bool = True

    consecutive_failures: int = 0
    last_status: int | None = None               # last HTTP status seen
    last_error: str | None = None
    last_delivered_at: datetime | None = None
    disabled_reason: str | None = None           # set when auto-disabled


def record_delivery(
    doc: DeliveryHealth,
    ok: bool,
    status: int | None,
    error: str | None,
    *,
    permanent_statuses: tuple[int, ...],
    permanent_reason: str,
    auto_disable_reason: str,
    max_failures: int = MAX_CONSECUTIVE_FAILURES,
) -> None:
    """Fold one delivery outcome into ``doc``'s health fields (does not save).

    On success: reset the failure streak, clear the error, stamp
    ``last_delivered_at``. On a permanent-reject ``status``: disable with
    ``permanent_reason``. Otherwise: increment the streak and auto-disable with
    ``auto_disable_reason`` once it reaches ``max_failures``. ``permanent_reason``
    and ``auto_disable_reason`` are ``str.format`` templates given ``status`` and
    ``failures``.
    """
    doc.last_status = status
    doc.updated_at = utcnow()
    if ok:
        doc.consecutive_failures = 0
        doc.last_error = None
        doc.last_delivered_at = utcnow()
        return
    if status in permanent_statuses:
        doc.active = False
        doc.last_error = error
        doc.disabled_reason = permanent_reason.format(status=status)
        return
    doc.consecutive_failures += 1
    doc.last_error = error
    if doc.consecutive_failures >= max_failures:
        doc.active = False
        doc.disabled_reason = auto_disable_reason.format(failures=doc.consecutive_failures)
