"""Misc endpoints: third-party modding software list + a time converter.

- Software: serves `gamedata/modding_software.json` as-is (cleaned into a list).
- Time: converts a wall-clock time in a chosen timezone to a unix instant, then
  renders it across the supported zones (incl. Trove "server" time = UTC-11) and
  as Discord timestamp codes. Ported from BetterTroveTools' sidebar time modal.
"""

from __future__ import annotations

import json
from datetime import UTC, datetime, timedelta, timezone
from functools import cache
from pathlib import Path
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import BaseModel

_DATA_DIR = Path(__file__).parent / "gamedata"

# Trove "server"/reset time runs a fixed 11 hours behind UTC (no DST).
TROVE_OFFSET = timedelta(hours=11)
_TROVE_TZ = timezone(-TROVE_OFFSET)

# Supported zones for the converter/clocks — (id, display name), Trove first.
# "local" from the app is dropped: server-side it's meaningless, clients pass a
# real IANA id instead.
TIMEZONES: list[tuple[str, str]] = [
    ("trove", "Trove Server (Reset)"),
    ("UTC", "UTC"),
    ("America/Sao_Paulo", "Brazil (Brasília)"),
    ("America/New_York", "US Eastern"),
    ("America/Los_Angeles", "US Pacific"),
    ("Europe/Lisbon", "Portugal / UK"),
    ("Europe/Paris", "Central Europe (FR, DE, ES)"),
    ("Europe/Moscow", "Russia (Moscow)"),
    ("Asia/Shanghai", "China (Beijing)"),
    ("Asia/Tokyo", "Japan & South Korea"),
    ("Australia/Sydney", "Australia (Sydney)"),
]

# Discord <t:unix:style> codes — paste into Discord, it renders in each viewer's
# own local time (so we return the codes, not a misleading server-side preview).
DISCORD_STYLES: list[tuple[str, str]] = [
    ("t", "Short time (16:20)"),
    ("T", "Long time (16:20:30)"),
    ("d", "Short date (20/04/2021)"),
    ("D", "Long date (20 April 2021)"),
    ("f", "Short date/time (20 April 2021 16:20)"),
    ("F", "Long date/time (Tuesday, 20 April 2021 16:20)"),
    ("R", "Relative (in 2 months)"),
]


class MiscError(ValueError):
    """Raised on invalid converter input (mapped to 400 at the router)."""


# --- Third-party software --------------------------------------------------


class SoftwareTool(BaseModel):
    name: str
    free: bool
    url: str
    description: str


class SoftwareCategory(BaseModel):
    key: str                      # "blueprints" | "vfx" | "ui" | "sound" | "textures"
    description: str
    software: list[SoftwareTool]


class ModdingSoftware(BaseModel):
    categories: list[SoftwareCategory]
    count: int


@cache
def _software_data() -> dict:
    return json.loads((_DATA_DIR / "modding_software.json").read_text(encoding="utf-8"))


def modding_software() -> dict:
    """Third-party Trove modding software, grouped by category."""
    categories = [
        {"key": key, "description": cat.get("description", ""), "software": cat.get("software", [])}
        for key, cat in _software_data().items()
    ]
    return {"categories": categories, "count": len(categories)}


# --- Time converter --------------------------------------------------------


class TimezoneInfo(BaseModel):
    id: str
    name: str


class TimezoneList(BaseModel):
    items: list[TimezoneInfo]
    count: int


class ZoneTime(BaseModel):
    id: str
    name: str
    datetime: str                 # ISO 8601 with offset
    date: str                     # e.g. "Jun 05, 2026"
    time: str                     # e.g. "14:30:00"


class DiscordTimestamp(BaseModel):
    style: str                    # t T d D f F R
    label: str
    code: str                     # "<t:1717596600:f>"


class TimeConvertRequest(BaseModel):
    datetime: str | None = None   # ISO wall clock, e.g. "2026-06-05T14:30" (naive = in `timezone`)
    timezone: str = "UTC"         # "trove" | "UTC" | any IANA id
    unix: int | None = None       # alternatively an absolute unix timestamp (timezone ignored)


class TimeConvertResponse(BaseModel):
    unix: int
    iso_utc: str
    zones: list[ZoneTime]
    discord: list[DiscordTimestamp]


class TimeNow(BaseModel):
    unix: int
    zones: list[ZoneTime]


def _tzinfo(tz_id: str):
    if tz_id == "trove":
        return _TROVE_TZ
    if tz_id == "UTC":
        return UTC
    try:
        return ZoneInfo(tz_id)
    except (ZoneInfoNotFoundError, ValueError) as e:
        raise MiscError(f"Unknown timezone '{tz_id}'.") from e


def _render_zone(unix: int, tz_id: str, name: str) -> dict:
    local = datetime.fromtimestamp(unix, _tzinfo(tz_id))
    return {
        "id": tz_id, "name": name,
        "datetime": local.isoformat(),
        "date": local.strftime("%b %d, %Y"),
        "time": local.strftime("%H:%M:%S"),
    }


def _all_zones(unix: int) -> list[dict]:
    return [_render_zone(unix, zid, name) for zid, name in TIMEZONES]


def timezones() -> dict:
    """The supported timezone reference list."""
    return {"items": [{"id": zid, "name": name} for zid, name in TIMEZONES], "count": len(TIMEZONES)}


def convert_time(datetime_str: str | None, timezone_id: str, unix: int | None) -> dict:
    """Convert a wall-clock time (in `timezone_id`) — or an absolute `unix` — to
    a unix instant rendered across every supported zone + Discord timestamp codes.
    """
    if unix is None:
        if not datetime_str:
            raise MiscError("Provide either 'unix' or 'datetime'.")
        try:
            naive = datetime.fromisoformat(datetime_str)
        except ValueError as e:
            raise MiscError(f"Invalid datetime '{datetime_str}': {e}") from e
        if naive.tzinfo is not None:
            unix = int(naive.timestamp())  # explicit offset given — already absolute
        else:
            unix = int(naive.replace(tzinfo=_tzinfo(timezone_id)).timestamp())
    instant = datetime.fromtimestamp(unix, UTC)
    return {
        "unix": unix,
        "iso_utc": instant.isoformat(),
        "zones": _all_zones(unix),
        "discord": [
            {"style": style, "label": label, "code": f"<t:{unix}:{style}>"}
            for style, label in DISCORD_STYLES
        ],
    }


def time_now(now: datetime | None = None) -> dict:
    """Current instant rendered across every supported zone (the live clocks)."""
    instant = now or datetime.now(UTC)
    unix = int(instant.timestamp())
    return {"unix": unix, "zones": _all_zones(unix)}
