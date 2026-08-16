"""Yearly calendar: every recurring rotation as one flat ±365-day timeline.

A "big aggregate" ported from BetterTroveTools - it expands all the deterministic
cycles (weekly buffs, the Corruxion/Fluxion merchants, gardening windows, and the
Wild Mana / Stampy biome events) across a year either side of now. It reuses the
exact anchors/lists from ``server_time`` and ``rotations`` so each entry lines up
with its dedicated endpoint instead of duplicating constants.

**Invasion is intentionally excluded** (that feature is out of scope project-wide).
Pure + deterministic: pass an explicit ``now`` to unit-test it. Real-UTC seconds.
"""

from collections.abc import Iterator
from datetime import datetime, timedelta

from app.trove import rotations, server_time

WINDOW_DAYS = 365
WEEK = timedelta(days=7)


def _window(now: datetime | None) -> tuple[datetime, datetime, datetime]:
    real = now or server_time.real_utc_now()
    return real, real - timedelta(days=WINDOW_DAYS), real + timedelta(days=WINDOW_DAYS)


def _occurrences(
    base: datetime, interval: timedelta, win_start: datetime, win_end: datetime
) -> Iterator[tuple[int, datetime]]:
    """Yield (index, start) for every `interval`-spaced occurrence of `base` that
    could overlap the window - from one interval before it through its end."""
    total = interval.total_seconds()
    k = int((win_start - base).total_seconds() // total) - 1
    while True:
        start = base + timedelta(seconds=k * total)
        if start >= win_end:
            return
        yield k, start
        k += 1


def _ev(type_: str, name: str, start: datetime, end: datetime, **extra) -> dict:
    return {"type": type_, "name": name,
            "starts_at": int(start.timestamp()), "ends_at": int(end.timestamp()), **extra}


def _overlaps(start: datetime, end: datetime, win_start: datetime, win_end: datetime) -> bool:
    return end > win_start and start < win_end


def _biome(name: str) -> dict:
    b = rotations._parent_biome(name)
    return {"name": b["name"], "icon": b["icon"]}


# --- per-type generators (anchors reused from server_time / rotations) ------

def _weekly_buffs(ws: datetime, we: datetime) -> list[dict]:
    data = server_time._load("weekly_buffs.json")
    if not data:
        return []
    n = len(data)
    base = server_time.FIRST_WEEK_BUFF + server_time.TROVE_OFFSET  # trove anchor -> real UTC
    out = []
    for k, s in _occurrences(base, WEEK, ws, we):
        e = s + WEEK
        if _overlaps(s, e, ws, we):
            buff = data.get(str(k % n), {})
            out.append(_ev("weekly_buff", buff.get("name", "Weekly Buff"), s, e,
                           color=buff.get("color")))
    return out


def _corruxion(ws: datetime, we: datetime) -> list[dict]:
    base = server_time.FIRST_CORRUXION + server_time.TROVE_OFFSET
    out = []
    for _k, s in _occurrences(base, server_time.DRAGON_INTERVAL, ws, we):
        e = s + server_time.DRAGON_DURATION
        if _overlaps(s, e, ws, we):
            out.append(_ev("corruxion", "Corruxion", s, e))
    return out


def _fluxion(ws: datetime, we: datetime) -> list[dict]:
    base = server_time.FIRST_FLUXION + server_time.TROVE_OFFSET
    dur = server_time.DRAGON_DURATION
    out = []
    for _k, s in _occurrences(base, server_time.DRAGON_INTERVAL, ws, we):
        vote_s, vote_e = s, s + dur                                   # 3-day voting window
        sell_s, sell_e = s + server_time.FLUXION_INTERVAL, s + server_time.FLUXION_INTERVAL + dur
        if _overlaps(vote_s, vote_e, ws, we):
            out.append(_ev("fluxion", "Fluxion (Voting)", vote_s, vote_e,
                           state="voting", color="5ca8cc"))
        if _overlaps(sell_s, sell_e, ws, we):
            out.append(_ev("fluxion", "Fluxion (Selling)", sell_s, sell_e,
                           state="selling", color="02679e"))
    return out


def _gardening(ws: datetime, we: datetime) -> list[dict]:
    base = server_time.FIRST_GARDENING + server_time.TROVE_OFFSET
    out = []
    for _k, s in _occurrences(base, timedelta(days=2), ws, we):       # 2-day plants ripen day 1->2
        h_s, h_e = s + timedelta(days=1), s + timedelta(days=2)
        if _overlaps(h_s, h_e, ws, we):
            out.append(_ev("gardening_2", "2-day plants", h_s, h_e, color="8bc34a"))
    for _k, s in _occurrences(base, timedelta(days=3), ws, we):       # 3-day plants ripen day 2->3
        h_s, h_e = s + timedelta(days=2), s + timedelta(days=3)
        if _overlaps(h_s, h_e, ws, we):
            out.append(_ev("gardening_3", "3-day plants", h_s, h_e, color="4caf50"))
    return out


def _stampy(ws: datetime, we: datetime) -> list[dict]:
    biomes = rotations._STAMPY_BIOMES
    out = []
    for w, s in _occurrences(rotations._STAMPY_BASE, rotations._STAMPY_PERIOD, ws, we):
        e = s + rotations._STAMPY_DURATION
        if _overlaps(s, e, ws, we):
            out.append(_ev("stampy", "Stampy", s, e, biomes=[_biome(biomes[w % len(biomes)])]))
    return out


def _mana(ws: datetime, we: datetime) -> list[dict]:
    biomes = rotations._MANA_BIOMES
    out = []
    for w, s in _occurrences(rotations._MANA_BASE, WEEK, ws, we):
        e = s + WEEK
        if _overlaps(s, e, ws, we):
            out.append(_ev("mana", "Wild Mana", s, e,
                           biomes=[_biome(biomes[(w - i) % len(biomes)]) for i in range(3)]))
    return out


def _luxion(runs: list[int], ws: datetime, we: datetime) -> list[dict]:
    """Recorded Luxion runs, expanded into their 3-hour merchant windows.

    Unlike every other generator here, Luxion's *placement* is NOT computed - its
    start is dev-set and unpredictable, so we can only place the runs the bot has
    actually captured (past appearances + the current one); future runs can't be
    projected. The windows within a run are computed, off a global 27h grid, so we
    emit one short event per window (its own timeline row, rendered as bare
    coloured pills). ``runs`` are the captured start-DAY anchors from
    ``LuxionAppearance.started_at``; ``schedule_for`` snaps each to the grid."""
    from app.trove.luxion import schedule_for

    out = []
    for started_at in runs:
        for w in schedule_for(started_at):
            s = datetime.fromtimestamp(w["starts_at"], server_time.UTC)
            e = datetime.fromtimestamp(w["ends_at"], server_time.UTC)
            if _overlaps(s, e, ws, we):
                out.append(_ev("luxion", "Luxion", s, e))
    return out


def yearly_calendar(now: datetime | None = None,
                    luxion_runs: list[int] | None = None) -> dict:
    """All recurring events across ±365 days, as one flat list sorted by start.

    ``luxion_runs`` (run-start anchors, unix seconds) are passed in by the caller
    rather than computed - Luxion is captured, not deterministic. Omitting it just
    leaves Luxion off the timeline; everything else stays pure + unit-testable."""
    real, ws, we = _window(now)
    events: list[dict] = []
    events += _weekly_buffs(ws, we)
    events += _corruxion(ws, we)
    events += _fluxion(ws, we)
    events += _luxion(luxion_runs or [], ws, we)
    events += _gardening(ws, we)
    events += _stampy(ws, we)
    events += _mana(ws, we)
    events.sort(key=lambda e: (e["starts_at"], e["type"]))
    return {
        "starts_at": int(ws.timestamp()),
        "ends_at": int(we.timestamp()),
        "generated_at": int(real.timestamp()),
        "count": len(events),
        "events": events,
    }
