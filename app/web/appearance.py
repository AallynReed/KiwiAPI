"""Visitor appearance preferences, read from two first-party cookies.

``btt_design`` picks the look: ``voxel`` (the default) or ``classic`` (the previous
dark design). ``btt_theme`` picks the Voxel theme: ``auto`` follows Trove server
time from morning to night, ``light`` and ``dark`` pin it. The visitor sets both
from the navbar's Appearance menu (``site/static/appearance.js``); nothing about
the choice is stored server-side.

Resolved here rather than in the browser so the first paint is already right: the
homepage template, the navbar skin and the time-of-day palette all come out of the
server matching the cookie, with no flash of the other look.
"""
from collections.abc import Mapping
from datetime import UTC, datetime, timedelta

from fastapi import Request

DESIGN_COOKIE = "btt_design"
THEME_COOKIE = "btt_theme"
DESIGNS = ("voxel", "classic")
THEMES = ("auto", "light", "dark")


def time_of_day(hour: int) -> str:
    """Trove-server-hour bands; ``site/static/home_voxel.js`` uses the same ones."""
    if 5 <= hour < 9:
        return "morning"
    if 9 <= hour < 17:
        return "day"
    if 17 <= hour < 20:
        return "dusk"
    return "night"


def resolve(cookies: Mapping[str, str], now: datetime | None = None) -> dict:
    design = cookies.get(DESIGN_COOKIE, "")
    theme = cookies.get(THEME_COOKIE, "")
    design = design if design in DESIGNS else DESIGNS[0]
    theme = theme if theme in THEMES else THEMES[0]
    if theme == "light":
        tod = "day"
    elif theme == "dark":
        tod = "night"
    else:
        server = (now or datetime.now(UTC)) - timedelta(hours=11)
        tod = time_of_day(server.hour)
    return {"design": design, "theme": theme, "vx_tod": tod}


def design(request: Request) -> str:
    return resolve(request.cookies)["design"]


def context(request: Request) -> dict:
    """Jinja context processor: ``design``, ``theme`` and ``vx_tod`` on every page."""
    return resolve(request.cookies)
