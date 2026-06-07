import asyncio
import logging
from datetime import datetime, timedelta
from html import escape as h

from beanie.operators import Set

from app.auth.emails import send_token_expiring_email
from app.auth.models import User
from app.core.config import settings
from app.core.database import get_db
from app.core.email_outbox import queue_email
from app.core.utils import utcnow
from app.tokens.models import ApiToken
from app.usage.models import UsageEvent

logger = logging.getLogger("kiwi.maintenance")

_SWEEP_INTERVAL_SECONDS = 86400  # daily
_STATE_COLLECTION = "system_state"


async def _get_state_time(key: str) -> datetime | None:
    doc = await get_db()[_STATE_COLLECTION].find_one({"_id": key})
    return doc["value"] if doc else None


async def _set_state_time(key: str, value: datetime) -> None:
    await get_db()[_STATE_COLLECTION].update_one(
        {"_id": key}, {"$set": {"value": value}}, upsert=True
    )


async def auto_revoke_inactive_tokens() -> int:
    """Revoke tokens that haven't been used in `token_inactivity_revoke_days`.

    "Used" means last_used_at; a token never used is measured from created_at.
    Returns the number revoked.
    """
    cutoff = utcnow() - timedelta(days=settings.token_inactivity_revoke_days)
    query = {
        "revoked": False,
        "$or": [
            {"last_used_at": {"$ne": None, "$lt": cutoff}},
            {"last_used_at": None, "created_at": {"$lt": cutoff}},
        ],
    }
    count = await ApiToken.find(query).count()
    if count:
        reason = f"Auto-revoked: inactive for {settings.token_inactivity_revoke_days} days"
        await ApiToken.find(query).update(  # pyright: ignore[reportGeneralTypeIssues]
            Set(
                {
                    ApiToken.revoked: True,
                    ApiToken.revoked_at: utcnow(),
                    ApiToken.revoke_reason: reason,
                }
            )
        )
        logger.warning("Auto-revoked %d inactive token(s)", count)
    return count


async def warn_expiring_tokens() -> int:
    """Email owners whose active tokens expire within the warning window.

    Each token is warned at most once (the ``expiry_warned`` flag), reset only
    when the token is rotated. Returns the number of warnings sent.
    """
    if not settings.security_email_notifications:
        return 0

    now = utcnow()
    horizon = now + timedelta(days=settings.token_expiry_warning_days)
    query = {
        "revoked": False,
        "expiry_warned": False,
        "expires_at": {"$ne": None, "$gte": now, "$lte": horizon},
    }
    tokens = await ApiToken.find(query).to_list()
    if not tokens:
        return 0

    users = {
        u.id: u
        for u in await User.find({"_id": {"$in": list({t.user_id for t in tokens})}}).to_list()
    }
    sent = 0
    for token in tokens:
        user = users.get(token.user_id)
        if user is not None and token.expires_at is not None:
            days_left = max(0, (token.expires_at - now).days)
            await send_token_expiring_email(user, token.name, token.prefix, days_left)
            sent += 1
        token.expiry_warned = True
        await token.save()

    if sent:
        logger.info("Sent %d token-expiry warning(s)", sent)
    return sent


async def daily_rate_limit_digest() -> bool:
    """Once per window, email a digest of rate-limit triggers (429s) to the admin.

    Sends only if the total in the window meets `rate_limit_alert_threshold`. A
    state marker throttles it to once per window so restarts don't re-send.
    """
    if not settings.rate_limit_alert_email:
        return False

    # Threshold + digest window are runtime-tunable (master admin panel).
    from app.admin import runtime_config
    hours = await runtime_config.get_setting("rate_limit_digest_window_hours")
    threshold = await runtime_config.get_setting("rate_limit_alert_threshold")

    now = utcnow()
    window = timedelta(hours=hours)
    last = await _get_state_time("rate_limit_digest")
    if last is not None and (now - last) < window * 0.9:
        return False  # already processed this window

    since = now - window
    rows = await UsageEvent.aggregate(
        [
            {"$match": {"created_at": {"$gte": since}, "status_code": 429}},
            {
                "$group": {
                    "_id": "$token_id",
                    "count": {"$sum": 1},
                    "user_id": {"$first": "$user_id"},
                }
            },
            {"$sort": {"count": -1}},
            {"$limit": 50},
        ]
    ).to_list()

    # Mark this window processed regardless of whether we email.
    await _set_state_time("rate_limit_digest", now)

    total = sum(r["count"] for r in rows)
    if total < threshold:
        return False

    user_ids = list({r["user_id"] for r in rows})
    token_ids = [r["_id"] for r in rows]
    emails = {u.id: u.email for u in await User.find({"_id": {"$in": user_ids}}).to_list()}
    names = {
        t.id: (t.name, t.prefix)
        for t in await ApiToken.find({"_id": {"$in": token_ids}}).to_list()
    }

    def email_for(r):
        return emails.get(r["user_id"], "unknown")

    def token_for(r):
        return names.get(r["_id"], ("(deleted token)", ""))

    text_lines = [
        f"{r['count']:>6}  {email_for(r)}  —  {token_for(r)[0]} ({token_for(r)[1]}…)"
        for r in rows
    ]
    text = (
        f"{total} rate-limit hits (429s) in the last {hours}h across {len(rows)} token(s).\n\n"
        f"{'hits':>6}  account  —  token\n" + "\n".join(text_lines)
    )

    html_rows = "".join(
        f"<tr><td align='right'>{r['count']}</td><td>{h(email_for(r))}</td>"
        f"<td>{h(token_for(r)[0])} <code>{h(token_for(r)[1])}…</code></td></tr>"
        for r in rows
    )
    html_body = (
        f"<p><b>{total}</b> rate-limit hits (429s) in the last {hours}h across "
        f"{len(rows)} token(s).</p>"
        "<table cellpadding='6' style='border-collapse:collapse'>"
        "<tr><th align='right'>Hits</th><th align='left'>Account</th>"
        "<th align='left'>Token</th></tr>" + html_rows + "</table>"
    )

    await queue_email(
        settings.rate_limit_alert_email,
        f"{settings.app_name} — rate-limit digest ({total} hits / {hours}h)",
        text,
        html_body,
    )
    logger.warning("Queued rate-limit digest (%d hits) to %s", total, settings.rate_limit_alert_email)
    return True


async def maintenance_loop() -> None:
    """Run the daily sweeps (auto-revoke + rate-limit digest). Cancelled on shutdown."""
    while True:
        try:
            await auto_revoke_inactive_tokens()
            await warn_expiring_tokens()
            await daily_rate_limit_digest()
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("Maintenance sweep failed")
        try:
            await asyncio.sleep(_SWEEP_INTERVAL_SECONDS)
        except asyncio.CancelledError:
            raise
