"""Discord message bodies for DM alerts.

Small, self-contained embeds built from the event payload (or, for the market
watchlist, the match we constructed). Kept separate from the guild-announcement
embeds in ``app/discord/embeds.py`` because a DM wants a tighter, personal shape
and must never depend on live global state that may have moved on by delivery time.
"""

from __future__ import annotations

from datetime import UTC, datetime

from app.trove.captures import classify_challenge

# Accent colours per alert kind (Discord ints).
_COLOR = {
    "challenge": 0x9B59B6,
    "corruxion": 0xF1C40F,
    "fluxion": 0xF39C12,
    "game_update": 0x5EC6FF,
    "market_watch": 0x2ECC71,
}
_FOOTER = "Kiwi DM alerts · manage at trove.aallyn.net/dashboard"


def _ts(unix: int | None) -> str | None:
    if not unix:
        return None
    try:
        return datetime.fromtimestamp(int(unix), UTC).isoformat()
    except (TypeError, ValueError, OSError):
        return None


def _rel(unix: int | None) -> str:
    """A Discord relative timestamp (<t:...:R>) or empty string."""
    return f"<t:{int(unix)}:R>" if unix else ""


def _embed(kind: str, title: str, description: str, *, ts: int | None = None) -> dict:
    e: dict = {"title": title, "description": description,
               "color": _COLOR.get(kind, 0x5EC6FF), "footer": {"text": _FOOTER}}
    iso = _ts(ts)
    if iso:
        e["timestamp"] = iso
    return {"embeds": [e]}


def build(event_type: str, data: dict) -> dict | None:
    """Render the DM body for an event, or None if we can't (skip delivery)."""
    data = data or {}
    if event_type == "challenge":
        name = data.get("name") or "Challenge"
        ctype = classify_challenge(name)
        ends = data.get("window_ends_at") or data.get("ends_at")
        label = {"collection": "Collection", "rampage": "Rampage", "racing": "Racing",
                 "target": "Target", "dungeon": "Dungeon"}.get(ctype, "Challenge")
        desc = f"**{name}**"
        if ends:
            desc += f"\nEnds {_rel(ends)}"
        return _embed("challenge", f"⚔️ {label} challenge is live", desc, ts=ends)
    if event_type in ("corruxion", "fluxion"):
        pretty = event_type.capitalize()
        ends = data.get("ends_at")
        desc = "The merchant is in town."
        if ends:
            desc += f"\nLeaves {_rel(ends)}"
        return _embed(event_type, f"🛒 {pretty} has arrived", desc, ts=ends)
    if event_type == "game_update":
        tag = data.get("version_tag") or data.get("tag") or data.get("version") or ""
        branch = data.get("branch") or "live"
        desc = f"A new Trove build shipped on **{branch}**."
        if tag:
            desc += f"\nVersion `{tag}`"
        desc += "\nBrowse the changed files at trove.aallyn.net/updates"
        return _embed("game_update", "🧩 New Trove update", desc)
    if event_type == "market_watch":
        name = data.get("name") or "Item"
        price = data.get("price_each")
        median = data.get("median_each")
        thr = data.get("max_price_each")
        desc = f"**{name}** is listed at **{_flux(price)}** flux each."
        if thr is not None:
            desc += f"\nYour alert: at or below {_flux(thr)}."
        if median is not None:
            desc += f"\nCurrent median: {_flux(median)}."
        desc += "\nSee live listings at trove.aallyn.net/market"
        return _embed("market_watch", f"📉 {name} hit your price", desc)
    return None


def test_body() -> dict:
    return _embed("game_update", "🥝 DM alerts connected",
                  "This is a test message. You'll get alerts like this when your "
                  "subscribed events fire. Manage them from your Dashboard.")


def _flux(n) -> str:
    try:
        n = float(n)
    except (TypeError, ValueError):
        return "?"
    if n >= 1_000_000:
        return f"{n / 1_000_000:.2f}".rstrip("0").rstrip(".") + "M"
    if n >= 1_000:
        return f"{n / 1_000:.1f}".rstrip("0").rstrip(".") + "k"
    return f"{round(n):,}"
