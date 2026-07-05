"""Shared number/timestamp formatters for Discord embed bodies.

Kept out of the individual embed modules (``app/discord``, ``app/dm_subs``,
``app/webhooks``) so the *pure* formatters live in one place while each module
keeps its own embed *builders*. Only behaviour-identical helpers belong here —
anything whose output differs between callers (e.g. the compact flux formatter,
whose rounding and None fallback diverge per surface) stays local on purpose.
"""

from __future__ import annotations

from datetime import UTC, datetime


def discord_ts(unix, style: str = "f") -> str:
    """A Discord ``<t:unix:style>`` tag — renders in each viewer's local zone.

    Styles: f = date+time, F = full, R = relative, t = time, d = date.
    Caller is responsible for any falsy/None guard it wants around this.
    """
    return f"<t:{int(unix)}:{style}>"


def unix_to_iso(unix) -> str | None:
    """Unix seconds → an ISO-8601 UTC string, or ``None`` for falsy/invalid input.

    Used for an embed's ``timestamp`` field, which Discord expects as ISO-8601.
    """
    if not unix:
        return None
    try:
        return datetime.fromtimestamp(int(unix), UTC).isoformat()
    except (TypeError, ValueError, OSError):
        return None
