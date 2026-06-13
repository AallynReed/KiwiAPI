"""Discord embed builders for Kiwi slash commands.

An "embed" is a plain dict matching the Discord API embed object, returned inside
an interaction response. Builders that only read computed/in-process data are
sync; those that hit Mongo (activity, chaos, challenge, giveaways) are async.

Timestamps use Discord's ``<t:unix:style>`` markup so every viewer sees them in
their own local time (styles: f = date+time, F = full, R = relative, t = time,
d = date). Trove itself runs on a UTC-11 frame.
"""
from __future__ import annotations

import time
from datetime import datetime, timezone

from app.trove.btt_releases import PLATFORMS, get_changelog, latest_per_platform
from app.trove.captures import get_current_challenge
from app.trove.chaos import get_chaos_chest
from app.trove.leaderboards.activity import activity_series, estimate_active_players
from app.trove.news import latest_news
from app.trove.rotations import biome_rotation, stampy, wild_mana
from app.trove.server_time import (
    corruxion,
    daily_buffs,
    fluxion,
    server_time,
    trove_now,
    weekly_buffs,
)
from app.trove.status import get_status_shared

SITE = "https://trove.aallyn.net"


def _ts(unix, style: str = "f") -> str:
    """A Discord timestamp tag - renders in the viewer's local timezone."""
    return f"<t:{int(unix)}:{style}>"


def _unix(dt: datetime) -> int:
    """Unix seconds from a datetime, treating naive values as UTC."""
    if dt.tzinfo is None:
        dt = dt.replace(tzinfo=timezone.utc)
    return int(dt.timestamp())


# ── /status ────────────────────────────────────────────────────────────────

# Binary online/down. "maintenance" is a legacy value (the prober no longer emits
# it) kept only so a stale snapshot still renders - as red "Down".
_COLOR = {"online": 0x46D39A, "maintenance": 0xF0556A, "down": 0xF0556A, "unknown": 0x8A93A3}
_DOT = {
    "online": "\U0001F7E2", "maintenance": "\U0001F534",
    "down": "\U0001F534", "unknown": "⚪",
}
_OVERALL = {
    "online": "All systems operational", "maintenance": "Major outage",
    "down": "Major outage", "unknown": "Status unknown",
}
_ENV_LABEL = {"online": "Online", "maintenance": "Down", "down": "Down", "unknown": "Unknown"}
_REGIONS = (
    ("eu", "\U0001F1EA\U0001F1FA EU Live"),
    ("us", "\U0001F1FA\U0001F1F8 US Live"),
    ("pts", "\U0001F9EA PTS"),
)


def _dot(status: str) -> str:
    return _DOT.get(status, _DOT["unknown"])


async def status_embed() -> dict:
    """Latest Trove server status (cross-process: reads the prober's shared snapshot,
    so it works in the bot process too, not just the API)."""
    snap = await get_status_shared()
    overall = snap.get("overall", "unknown")
    envs = snap.get("environments") or {}

    fields = []
    for key, label in _REGIONS:
        env = envs.get(key) or {}
        st = env.get("status", "unknown")
        value = f"{_dot(st)} {_ENV_LABEL.get(st, 'Unknown')}"
        latency = (env.get("game") or {}).get("latency_ms")
        if st == "online" and isinstance(latency, (int, float)):
            value += f" · {round(latency)} ms"
        fields.append({"name": label, "value": value, "inline": True})

    auth = snap.get("auth")
    if auth is None:
        auth_value = f"{_dot('unknown')} Checking…"
    elif auth.get("online"):
        auth_value = f"{_dot('online')} Reachable"
    else:
        auth_value = f"{_dot('down')} Unreachable"
    fields.append({"name": "Account auth", "value": auth_value, "inline": False})

    # "down" overall → "Partial outage" when some Live region is still up.
    if overall == "down" and any(
        (envs.get(k) or {}).get("status") == "online" for k in ("eu", "us")
    ):
        overall_text = "Partial outage"
    else:
        overall_text = _OVERALL.get(overall, _OVERALL["unknown"])
    embed = {
        "title": "Trove server status",
        "url": f"{SITE}/status",
        "color": _COLOR.get(overall, _COLOR["unknown"]),
        "description": f"{_dot(overall)}  **{overall_text}**",
        "fields": fields,
        "footer": {"text": "trove.aallyn.net/status"},
    }
    checked = snap.get("checked_at")
    if checked:
        embed["timestamp"] = datetime.fromtimestamp(checked, tz=timezone.utc).isoformat()
    return embed


# ── /activity ──────────────────────────────────────────────────────────────

ACTIVITY_LABELS = {
    "1d": "Last 24 hours", "7d": "Last 7 days", "1m": "Last month",
    "3m": "Last 3 months", "6m": "Last 6 months", "1y": "Last year", "all": "All time",
}


def _num(v) -> str:
    return f"{v:,}" if isinstance(v, int) else "—"


async def activity_embed(period: str = "7d") -> dict:
    """Player-activity headline numbers + the rendered trend chart for ``period``."""
    if period not in ACTIVITY_LABELS:
        period = "7d"
    label = ACTIVITY_LABELS[period]
    live = await estimate_active_players()
    series = await activity_series(period)

    fields = [
        {"name": "Active now", "value": _num(live.get("estimate")), "inline": True},
        {"name": "Last 24h", "value": _num(live.get("estimate_24h")), "inline": True},
        {"name": "Last 7d", "value": _num(live.get("estimate_7d")), "inline": True},
    ]
    peak = series.get("peak") or {}
    if isinstance(peak.get("active"), (int, float)):
        when = f" ({_ts(peak['t'], 'd')})" if peak.get("t") else ""
        fields.append({"name": f"Peak · {label.lower()}", "value": f"{round(peak['active']):,}/h{when}", "inline": True})
    if isinstance(series.get("average"), (int, float)):
        fields.append({"name": "Average", "value": f"{round(series['average']):,}/h", "inline": True})

    # Cache-bust the chart image by the data's compute time so Discord refreshes
    # it when new captures land (it caches embed images by URL otherwise).
    cb = live.get("computed_at") or series.get("window_end") or 0
    return {
        "title": f"Player activity — {label}",
        "url": f"{SITE}/activity?period={period}",
        "color": 0x4CC9F0,
        "description": "Estimated distinct active players, from hourly leaderboard captures.",
        "fields": fields,
        "image": {"url": f"{SITE}/activity/og.png?period={period}&v={cb}"},
        "footer": {"text": "trove.aallyn.net/activity · Trove server time (UTC−11)"},
    }


# ── /chaos ─────────────────────────────────────────────────────────────────

async def chaos_embed() -> dict:
    """This week's Chaos Chest featured item + the weekly window."""
    c = await get_chaos_chest()
    item = c.get("item") or {}
    name = item.get("name")
    desc = (f"This week's featured item: **{name}**" if name
            else "No item has been relayed for this week's chest yet.")
    return {
        "title": "🎁 Chaos Chest",
        "color": 0xB794F6,
        "description": desc,
        "fields": [
            {"name": "Rotates", "value": f"{_ts(c['ends_at'], 'R')}\n{_ts(c['ends_at'], 'f')}", "inline": True},
            {"name": "This week", "value": f"{_ts(c['starts_at'], 'd')} → {_ts(c['ends_at'], 'd')}", "inline": True},
        ],
        "footer": {"text": "Trove Chaos Chest · rotates weekly (Tue 11:00 UTC)"},
    }


# ── /servertime ────────────────────────────────────────────────────────────

def servertime_embed() -> dict:
    """Current Trove server time (UTC-11) plus the viewer's local time + resets."""
    s = server_time()
    now = s["now_unix"]
    server_str = trove_now().strftime("%A, %d %b %Y · %H:%M")
    return {
        "title": "🕙 Trove server time",
        "color": 0x5EC6FF,
        "description": f"Trove runs on **UTC−11**. Right now it's **{server_str}** in server time.",
        "fields": [
            {"name": "Your local time", "value": f"{_ts(now, 'F')}\n{_ts(now, 'R')}", "inline": False},
            {"name": "Daily reset", "value": f"{_ts(s['daily_reset_at'], 'R')}\n{_ts(s['daily_reset_at'], 'f')}", "inline": True},
            {"name": "Weekly reset", "value": f"{_ts(s['weekly_reset_at'], 'R')}\n{_ts(s['weekly_reset_at'], 'f')}", "inline": True},
        ],
        "footer": {"text": "Daily reset 11:00 UTC · weekly Monday 11:00 UTC"},
    }


# ── /bonuses ───────────────────────────────────────────────────────────────

def _bullets(items) -> str:
    return "\n".join(f"• {b}" for b in (items or [])) or "—"


def bonuses_embed() -> dict:
    """Today's daily bonus + this week's weekly bonus."""
    d = (daily_buffs() or {}).get("current") or {}
    w = (weekly_buffs() or {}).get("current") or {}

    daily_val = _bullets(d.get("normal_buffs"))
    if d.get("premium_buffs"):
        daily_val += f"\n*Patron:* {', '.join(d['premium_buffs'])}"

    color = 0xFFB86B
    try:
        if d.get("color"):
            color = int(d["color"], 16)
    except (ValueError, TypeError):
        pass

    embed = {
        "title": "✨ Trove bonuses",
        "color": color,
        "fields": [
            {"name": f"{d.get('emoji', '📅')} Daily — {d.get('name', '?')} ({d.get('weekday', '')})",
             "value": daily_val, "inline": False},
            {"name": f"{w.get('emoji', '🗓️')} Weekly — {w.get('name', '?')}",
             "value": _bullets(w.get("buffs")), "inline": False},
        ],
        "footer": {"text": "Daily resets 11:00 UTC · weekly Monday"},
    }
    if d.get("icon"):
        embed["thumbnail"] = {"url": d["icon"]}
    return embed


# ── /challenge ─────────────────────────────────────────────────────────────

async def challenge_embed() -> dict:
    """The current hourly challenge."""
    cur = await get_current_challenge()

    if cur.get("name"):
        cur_val = f"**{cur['name']}**"
        if cur.get("type"):
            cur_val += f" · {cur['type'].title()}"
        cur_val += (f"\nEnds {_ts(cur['ends_at'], 'R')}" if cur.get("active")
                    else f"\nNext window {_ts(cur['starts_at'], 'R')}")
    else:
        cur_val = f"No challenge captured for this window.\nWindow {_ts(cur['starts_at'], 't')} → {_ts(cur['ends_at'], 't')}"

    return {
        "title": "⚔️ Hourly challenge",
        "color": 0xFF9800,
        "description": "Trove's hourly challenge - 20 minutes at the top of each hour.",
        "fields": [{"name": "Current", "value": cur_val, "inline": False}],
        "footer": {"text": "Times shown in your local zone"},
    }


# ── /longshade (Depth-15 delve biomes) ─────────────────────────────────────

def _biomes_str(rot: dict) -> str:
    return "\n".join(f"• {b.get('final_name') or b.get('name')}" for b in (rot.get("biomes") or [])) or "—"


def longshade_embed() -> dict:
    """The current Depth-15 adventure biome rotation, plus what's up next."""
    r = biome_rotation()
    cur = r.get("current") or {}
    upcoming = r.get("upcoming") or []
    nxt = upcoming[0] if upcoming else None

    fields = [
        {"name": "Current biomes", "value": _biomes_str(cur), "inline": True},
        {"name": "Rotates", "value": _ts(cur.get("ends_at", 0), "R"), "inline": True},
    ]
    if nxt:
        fields.append({"name": f"Up next ({_ts(nxt['starts_at'], 't')})", "value": _biomes_str(nxt), "inline": False})

    return {
        "title": "🌑 Depth 15 biomes",
        "color": 0x9B8AFB,
        "description": "Current Depth-15 adventure biome rotation (long shade).",
        "fields": fields,
        "footer": {"text": "Rotates every 3 hours · Trove server time (UTC−11)"},
    }


# ── /giveaways ─────────────────────────────────────────────────────────────

def _gw_section(items, fmt, empty: str, cap: int = 5) -> str:
    if not items:
        return empty
    out = "\n\n".join(fmt(g) for g in items[:cap])
    if len(items) > cap:
        out += f"\n…and {len(items) - cap} more"
    return out


async def giveaways_embed() -> dict:
    """Current, upcoming, and recently-ended (last 7 days) giveaways."""
    from app.giveaways import service

    ongoing = await service.list_ongoing()
    upcoming = await service.list_upcoming()
    ended = await service.list_ended(days=7)

    def fmt_open(g):
        return f"**{g.title}** — {g.prize_name}\nEnds {_ts(_unix(g.ends_at), 'R')} · {g.entry_count} entered"

    def fmt_upcoming(g):
        return f"**{g.title}** — {g.prize_name}\nStarts {_ts(_unix(g.starts_at), 'R')}"

    def fmt_ended(g):
        won = f" · won by {g.winner_username}" if g.winner_username else ""
        return f"**{g.title}** — {g.prize_name}\nEnded {_ts(_unix(g.ends_at), 'R')}{won}"

    return {
        "title": "🎉 Giveaways",
        "url": f"{SITE}/giveaways",
        "color": 0xFF5C80,
        "fields": [
            {"name": f"🟢 Open now ({len(ongoing)})",
             "value": _gw_section(ongoing, fmt_open, "None right now."), "inline": False},
            {"name": f"🔜 Upcoming ({len(upcoming)})",
             "value": _gw_section(upcoming, fmt_upcoming, "Nothing scheduled."), "inline": False},
            {"name": f"🏁 Ended · last 7 days ({len(ended)})",
             "value": _gw_section(ended, fmt_ended, "None in the last 7 days."), "inline": False},
        ],
        "footer": {"text": "trove.aallyn.net/giveaways"},
    }


# ── /corruxion ─────────────────────────────────────────────────────────────

def corruxion_embed() -> dict:
    """The Corruxion merchant: whether it's here now, or when it next arrives."""
    c = corruxion()
    if c["active"]:
        desc = f"**Corruxion is here!**\nLeaves {_ts(c['ends_at'], 'R')} · {_ts(c['ends_at'], 'f')}"
    else:
        desc = (f"Corruxion is away.\nArrives {_ts(c['starts_at'], 'R')} · {_ts(c['starts_at'], 'f')}\n"
                f"Stays until {_ts(c['ends_at'], 'f')}")
    fields = []
    upcoming = (c.get("schedule") or [])[1:5]
    if upcoming:
        lines = [f"{_ts(w['starts_at'], 'd')} → {_ts(w['ends_at'], 'd')}" for w in upcoming]
        fields.append({"name": "Upcoming visits", "value": "\n".join(lines), "inline": False})
    return {
        "title": "🐲 Corruxion",
        "color": 0x9B5DE5,
        "description": desc,
        "fields": fields,
        "footer": {"text": "Corruxion merchant · 14-day cycle, 3-day visits"},
    }


# ── /fluxion ───────────────────────────────────────────────────────────────

def fluxion_embed() -> dict:
    """The Fluxion merchant: its current voting/selling stage, or the next one."""
    f = fluxion()
    if f["active"]:
        desc = (f"**Fluxion is here** — currently in the **{f['state']}** stage.\n"
                f"Leaves {_ts(f['ends_at'], 'R')} · {_ts(f['ends_at'], 'f')}")
    else:
        nxt = (f.get("schedule") or [{}])[0]
        stage = nxt.get("state", "?")
        desc = (f"Fluxion is away.\nNext up: the **{stage}** stage, {_ts(f['starts_at'], 'R')} · "
                f"{_ts(f['starts_at'], 'f')}\nRuns until {_ts(f['ends_at'], 'f')}")
    fields = []
    upcoming = (f.get("schedule") or [])[1:5]
    if upcoming:
        lines = [f"{_ts(w['starts_at'], 'd')} → {_ts(w['ends_at'], 'd')} · {w['state']}" for w in upcoming]
        fields.append({"name": "Upcoming stages", "value": "\n".join(lines), "inline": False})
    return {
        "title": "💧 Fluxion",
        "color": 0x4CC9F0,
        "description": desc,
        "fields": fields,
        "footer": {"text": "Fluxion merchant · alternates voting / selling"},
    }


# ── /stampy ────────────────────────────────────────────────────────────────

def stampy_embed() -> dict:
    """The Stampy event: the current or next window and its biome."""
    s = stampy()
    cur = s.get("current")
    if not cur:
        return {"title": "🐘 Stampy", "color": 0xFFB86B,
                "description": "No upcoming Stampy event scheduled.",
                "footer": {"text": "Stampy event · weekly, 48-hour window"}}
    now = int(time.time())
    biome = (cur.get("biomes") or [{}])[0]
    bname = biome.get("final_name") or biome.get("name") or "?"
    if cur["starts_at"] <= now < cur["ends_at"]:
        state = f"**Active now** — ends {_ts(cur['ends_at'], 'R')}"
    else:
        state = f"Starts {_ts(cur['starts_at'], 'R')} · {_ts(cur['starts_at'], 'f')}"
    fields = [
        {"name": "Biome", "value": bname, "inline": True},
        {"name": "Window", "value": f"{_ts(cur['starts_at'], 'd')} → {_ts(cur['ends_at'], 'd')}", "inline": True},
    ]
    return {
        "title": "🐘 Stampy",
        "color": 0xFFB86B,
        "description": state,
        "fields": fields,
        "footer": {"text": "Stampy event · 48-hour window"},
    }


# ── /wild_mana ─────────────────────────────────────────────────────────────

def wild_mana_embed() -> dict:
    """The weekly Wild Mana biome rotation: this week + next."""
    w = wild_mana()
    cur = w.get("current") or {}
    upcoming = w.get("upcoming") or []
    nxt = upcoming[0] if upcoming else None
    fields = [
        {"name": "This week's biomes", "value": _biomes_str(cur), "inline": True},
        {"name": "Rotates", "value": _ts(cur.get("ends_at", 0), "R"), "inline": True},
    ]
    if nxt:
        fields.append({"name": f"Next week ({_ts(nxt['starts_at'], 'd')})", "value": _biomes_str(nxt), "inline": False})
    return {
        "title": "🌿 Wild Mana",
        "color": 0x46D39A,
        "description": "Weekly Wild Mana biome rotation.",
        "fields": fields,
        "footer": {"text": "Rotates weekly · Trove server time (UTC−11)"},
    }


# ── /trove_news ────────────────────────────────────────────────────────────

async def trove_news_embed() -> dict:
    """The latest Trove news (relayed from the official feed)."""
    items = await latest_news(5)
    if not items:
        return {"title": "📰 Trove news", "color": 0x5EC6FF,
                "description": "No news cached yet — check back shortly."}
    top = items[0]
    fields = []
    for n in items[1:5]:
        when = f" · {_ts(_unix(n.published_at), 'R')}" if n.published_at else ""
        fields.append({"name": n.title[:250], "value": f"[Read more]({n.url}){when}", "inline": False})
    embed = {
        "title": f"📰 {top.title}"[:256],
        "url": top.url,
        "color": 0x5EC6FF,
        "description": (top.summary or "")[:600],
        "fields": fields,
        "footer": {"text": "Latest from trovegame.com"},
    }
    if getattr(top, "image", None):
        embed["image"] = {"url": top.image}
    if top.published_at:
        embed["timestamp"] = top.published_at.isoformat()
    return embed


# ── /download ──────────────────────────────────────────────────────────────

_PLATFORM_LABEL = {"windows": "🪟 Windows", "linux": "🐧 Linux", "android": "🤖 Android"}


async def download_embed() -> dict:
    """Latest Better Trove Tools download per platform (release channel)."""
    latest = await latest_per_platform("release")
    fields = []
    releases_page = None
    for plat in PLATFORMS:
        entry = latest.get(plat)
        label = _PLATFORM_LABEL.get(plat, plat.title())
        if not entry:
            fields.append({"name": label, "value": "—", "inline": True})
            continue
        rel, assets = entry
        releases_page = releases_page or rel.html_url
        url = assets[0]["url"] if assets else rel.html_url
        fields.append({"name": label, "value": f"[{rel.tag_name}]({url})", "inline": True})
    return {
        "title": "⬇️ Download Better Trove Tools",
        "url": releases_page or "https://github.com/AallynReed/BetterTroveTools/releases",
        "color": 0x4CC9F0,
        "description": "Latest desktop build per platform. Prefer the browser? Use the web app: "
                       "**https://btt.aallyn.net**",
        "fields": fields,
        "footer": {"text": "github.com/AallynReed/BetterTroveTools/releases"},
    }


# ── /web ───────────────────────────────────────────────────────────────────

def web_embed() -> dict:
    """Where to use Better Trove Tools in the browser."""
    return {
        "title": "🌐 Better Trove Tools — Web App",
        "url": "https://btt.aallyn.net",
        "color": 0x4CC9F0,
        "description": (
            "Use Better Trove Tools right in your browser — no install needed.\n\n"
            "**→ https://btt.aallyn.net**\n\n"
            "Gem builds, star chart, calculators, codexes and more. For the full suite "
            "(mod manager, file explorer), grab the desktop app with `/download`."
        ),
        "footer": {"text": "btt.aallyn.net"},
    }


# ── /change_log ────────────────────────────────────────────────────────────

async def changelog_embed() -> dict:
    """Recent Better Trove Tools changes, grouped by version."""
    cl = await get_changelog()
    if cl is None or not cl.groups:
        return {"title": "📝 BTT changelog", "color": 0x8A93A3,
                "description": "No changelog cached yet."}
    fields = []
    for group in cl.groups:
        commits = group.get("commits") or []
        if not commits:
            continue
        lines = [f"• {(c.get('message') or '').splitlines()[0][:100]}" for c in commits[:6]]
        if len(commits) > 6:
            lines.append(f"…and {len(commits) - 6} more")
        fields.append({"name": group.get("version") or "Unreleased", "value": "\n".join(lines), "inline": False})
        if len(fields) >= 4:
            break
    embed = {
        "title": "📝 Better Trove Tools — changelog",
        "url": "https://github.com/AallynReed/BetterTroveTools/releases",
        "color": 0xB794F6,
        "fields": fields,
        "footer": {"text": "Recent changes · github.com/AallynReed/BetterTroveTools"},
    }
    if cl.rate_limited:
        embed["description"] = "⚠️ GitHub rate-limited the last refresh — this may be slightly behind."
    return embed


# ── Live "Trove Now" board ──────────────────────────────────────────────────

async def live_board_embed() -> dict:
    """The live 'Trove Now' board as a single auto-updating image. The image (served
    by the API at /board.png, rendered once per minute and cached in Redis - so 100
    guilds share one render) carries all the data; the embed is a thin wrapper whose
    accent matches the current server status. The ``?v=<minute>`` cache-buster makes
    Discord refetch the image each minute."""
    overall = (await get_status_shared()).get("overall", "unknown")
    minute = int(time.time() // 60)
    return {
        "title": "🥝 Trove Now",
        "url": SITE,
        "color": _COLOR.get(overall, 0x46D39A),
        "image": {"url": f"{SITE}/board.png?v={minute}"},
        "footer": {"text": "Live · updates every minute · trove.aallyn.net"},
        "timestamp": datetime.now(timezone.utc).isoformat(),
    }


# ── /ping ──────────────────────────────────────────────────────────────────

async def ping_embed() -> dict:
    """Confirm the API is up and measure its DB + Redis round-trip in-process."""
    from app.core.database import get_db
    from app.core.redis import get_redis

    fields = []
    try:
        t = time.perf_counter()
        await get_db().command("ping")
        fields.append({"name": "MongoDB", "value": f"🟢 {(time.perf_counter() - t) * 1000:.0f} ms", "inline": True})
    except Exception:
        fields.append({"name": "MongoDB", "value": "🔴 unreachable", "inline": True})

    r = get_redis()
    if r is None:
        fields.append({"name": "Redis", "value": "⚪ not configured", "inline": True})
    else:
        try:
            t = time.perf_counter()
            await r.ping()
            fields.append({"name": "Redis", "value": f"🟢 {(time.perf_counter() - t) * 1000:.0f} ms", "inline": True})
        except Exception:
            fields.append({"name": "Redis", "value": "🔴 unreachable", "inline": True})

    return {
        "title": "🏓 Pong",
        "color": 0x46D39A,
        "description": "Kiwi API is online.",
        "fields": fields,
        "footer": {"text": "api.aallyn.net/health"},
    }
