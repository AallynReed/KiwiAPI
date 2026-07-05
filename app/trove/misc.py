"""Misc endpoints: third-party modding software list + a time converter.

- Software: serves `gamedata/modding_software.json` as-is (cleaned into a list).
- Time: converts a wall-clock time in a chosen timezone to a unix instant, then
  renders it across the supported zones (incl. Trove "server" time = UTC-11) and
  as Discord timestamp codes. Ported from BetterTroveTools' sidebar time modal.
"""

from __future__ import annotations

import json
import re
from datetime import UTC, datetime, timedelta, timezone
from functools import cache
from pathlib import Path
from typing import TYPE_CHECKING
from zoneinfo import ZoneInfo, ZoneInfoNotFoundError

from pydantic import BaseModel

if TYPE_CHECKING:
    from app.trove.models import FeedbackEntry

_DATA_DIR = Path(__file__).parent / "gamedata"

# Trove "server"/reset time runs a fixed 11 hours behind UTC (no DST).
TROVE_OFFSET = timedelta(hours=11)
_TROVE_TZ = timezone(-TROVE_OFFSET)

# Supported zones for the converter/clocks - (id, display name), Trove first.
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

# Discord <t:unix:style> codes - paste into Discord, it renders in each viewer's
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
    """Convert a wall-clock time (in `timezone_id`) - or an absolute `unix` - to
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
            unix = int(naive.timestamp())  # explicit offset given - already absolute
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


# --- Feedback ingest -------------------------------------------------------
#
# parse_user_agent decomposes a UA string into the two fields that ACTUALLY
# matter for triage ("what OS am I on?", "what browser/app sent this?") so
# the Discord webhook can render them as clean key/value pairs instead of a
# 200-char hieroglyph. We do NOT use a UA-parsing library because:
#   - the input space is small (we only care about ~5 OSes and ~5 clients)
#   - the libraries that DO this well are heavyweight (~50KB+ of regex tables)
#   - falling back to "Unknown" is fine - UA is a hint, not a contract
#
# UA conventions for the BTT first-party apps:
#   Desktop (Windows/Linux/macOS):
#     BetterTroveTools/<version> (<OS-token>)
#       e.g. "BetterTroveTools/2026.06.07 (Windows NT 10.0)"
#            "BetterTroveTools/2026.06.07 (X11; Linux x86_64)"
#            "BetterTroveTools/2026.06.07 (Macintosh; Mac OS X 14_0)"
#   Mobile (Android):
#     BetterTroveTools/<version> (Android <version>; <device>)
#       e.g. "BetterTroveTools/2026.06.07 (Android 14; Pixel 8)"
#     The "(Android)" parenthetical is the IMPORTANT bit - without it the
#     webhook embed can't tell the desktop and mobile build apart, since
#     the BetterTroveTools/<ver> token by itself doesn't carry a platform.
#   Web (browser):
#     Whatever the browser sends - we never touch it client-side.


# OS markers, in detection priority order. We check Android BEFORE Linux
# because Android UAs always say "Linux; Android 14" - Linux first would
# steal them. Same for Windows-on-WSL2 (we still want Windows). The
# version capture group is OPTIONAL because the BTT Android app may ship
# a terse UA like "BetterTroveTools/X.Y.Z (Android)" without an OS
# version; we still want the OS row to read "Android" instead of None.
_OS_RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("iOS",      re.compile(r"\b(?:iPhone|iPad|iPod)(?:.*?OS\s+(\d+(?:[._]\d+)?))?", re.I)),
    ("Android",  re.compile(r"\bAndroid(?:\s+(\d+(?:\.\d+)?))?", re.I)),
    ("Windows",  re.compile(r"\bWindows NT\s+(\d+(?:\.\d+)?)", re.I)),
    ("macOS",    re.compile(r"\bMac OS X\s+(\d+(?:[._]\d+)?)", re.I)),
    ("Linux",    re.compile(r"\bLinux\b", re.I)),
)

# Map Windows NT major.minor -> consumer-facing Windows version.
_WINDOWS_NT_TO_NAME = {
    "10.0": "10/11",  # Win11 still reports NT 10.0, can't disambiguate
    "6.3":  "8.1",
    "6.2":  "8",
    "6.1":  "7",
}

# Browser / app markers, also in priority order. We check Edge / OPR /
# our own BetterTroveTools BEFORE Chrome because all three Chromium
# children carry "Chrome/X" in their UA - Chrome-first would steal them.
_CLIENT_RULES: tuple[tuple[str, re.Pattern[str]], ...] = (
    ("BetterTroveTools",
     re.compile(r"\bBetterTroveTools/(\d+(?:\.\d+){1,3})", re.I)),
    ("Edge",     re.compile(r"\bEdg(?:e|A|iOS)?/(\d+(?:\.\d+)?)", re.I)),
    ("Opera",    re.compile(r"\bOPR/(\d+(?:\.\d+)?)", re.I)),
    ("Brave",    re.compile(r"\bBrave/(\d+(?:\.\d+)?)", re.I)),
    ("Firefox",  re.compile(r"\bFirefox/(\d+(?:\.\d+)?)", re.I)),
    ("Chrome",   re.compile(r"\bChrome/(\d+(?:\.\d+)?)", re.I)),
    ("Safari",   re.compile(r"\bVersion/(\d+(?:\.\d+)?).*?Safari", re.I)),
)


def parse_user_agent(ua: str | None) -> tuple[str | None, str | None]:
    """Return ``(os_label, client_label)`` extracted from a user-agent
    string. Either component falls back to ``None`` if no rule matched."""
    if not ua:
        return None, None
    s = ua[:500]  # cap before regex - long UAs are usually trash

    os_label: str | None = None
    for name, pattern in _OS_RULES:
        m = pattern.search(s)
        if not m:
            continue
        # Capture group 1 = version when present. Linux rule has no group;
        # Android/iOS rules have an OPTIONAL group, which may not capture
        # anything for terse UAs like "BetterTroveTools/X.Y.Z (Android)".
        # Inspect .groups() length so we never IndexError on the Linux rule.
        ver = m.group(1) if m.groups() else None
        if name == "Windows":
            friendly = _WINDOWS_NT_TO_NAME.get(ver, ver) if ver else None
            os_label = f"Windows {friendly}" if friendly else "Windows"
        elif name == "macOS":
            os_label = f"macOS {ver.replace('_', '.')}" if ver else "macOS"
        elif name == "iOS":
            os_label = f"iOS {ver.replace('_', '.')}" if ver else "iOS"
        elif name == "Android":
            os_label = f"Android {ver}" if ver else "Android"
        else:
            os_label = name
        break

    client_label: str | None = None
    for name, pattern in _CLIENT_RULES:
        m = pattern.search(s)
        if not m:
            continue
        client_label = f"{name} {m.group(1)}"
        break
    return os_label, client_label


async def insert_feedback(
    message: str,
    contact: str | None,
    category: str,
    app_version: str | None,
    user_agent: str | None,
    attachments: list[dict] | None,
) -> FeedbackEntry:
    """Persist one feedback submission. Inputs are already validated by
    the router (length, file count, MIME types). We do one extra trim
    here because Pydantic's ``max_length`` doesn't strip whitespace, and
    a ``"   \\n\\n   "`` message would otherwise survive the >=5-char
    minimum check via padding."""
    # Lazy import: defer Beanie Document classes until Mongo is initialised.
    from app.trove.models import FeedbackAttachmentInfo, FeedbackEntry

    msg = message.strip()
    if len(msg) < 5:
        raise MiscError("Message must be at least 5 non-whitespace characters.")
    contact_clean = (contact.strip() if contact else None) or None
    app_version_clean = (app_version.strip() if app_version else None) or None
    ua = (user_agent or "")[:300] or None  # raw fallback
    os_label, client_label = parse_user_agent(ua)
    attachment_infos = [
        FeedbackAttachmentInfo(
            filename=a["filename"], content_type=a["content_type"], size=a["size"],
        )
        for a in (attachments or [])
    ]
    doc = FeedbackEntry(
        message=msg,
        contact=contact_clean,
        category=category,
        app_version=app_version_clean,
        os=os_label,
        client=client_label,
        user_agent=ua,
        attachments=attachment_infos,
    )
    return await doc.insert()
