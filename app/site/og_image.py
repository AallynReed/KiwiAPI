"""Render the Player Activity 1D chart to a PNG for OG / Twitter cards.

Drawn with Pillow to echo the ``/activity`` page: dark canvas, green area +
line, the daily reset marker (00:00 Trove server time = 11:00 UTC), a big live
active-player count, and branding. 1200x630 - the size Twitter / Discord /
Facebook expect for ``summary_large_image``.

Fonts: DejaVu (installed via the Dockerfile) in the container, with fallbacks to
common desktop fonts so the renderer also works in local dev, and finally
Pillow's bitmap default so it never hard-fails.
"""
from __future__ import annotations

import base64
import io
import logging
import time
from datetime import datetime, timezone

from PIL import Image, ImageDraw, ImageFont

logger = logging.getLogger(__name__)

VALID_PERIODS = ("1d", "7d", "1m", "3m", "6m", "1y", "all")
_PERIOD_LABEL = {
    "1d": "Last 24 hours", "7d": "Last 7 days", "1m": "Last 30 days",
    "3m": "Last 3 months", "6m": "Last 6 months", "1y": "Last 12 months",
    "all": "All time",
}


def _axis_fmt(period: str) -> str:
    if period == "1d":
        return "%H:%M"
    if period == "7d":
        return "%a"
    if period in ("1m", "3m"):
        return "%b %d"
    return "%b '%y"


# In-process render cache: dedupe scraper / spam hits so each card renders at
# most once per TTL regardless of the HTTP/CDN cache in front of us.
_CACHE: dict[str, tuple[float, bytes]] = {}
_CACHE_TTL = 600


def _cache_get(key: str, ttl: float = _CACHE_TTL) -> bytes | None:
    hit = _CACHE.get(key)
    if hit is not None and (time.time() - hit[0]) < ttl:
        return hit[1]
    return None


def _cache_put(key: str, png: bytes) -> None:
    _CACHE[key] = (time.time(), png)
    if len(_CACHE) > 32:   # only ~8 keys ever exist; this is just a safety cap
        _CACHE.pop(min(_CACHE, key=lambda k: _CACHE[k][0]), None)

W, H = 1200, 630
BG = (13, 17, 23)           # #0d1117
GREEN = (93, 208, 120)      # #5dd078 - line
GREEN_HI = (142, 229, 160)  # #8ee5a0 - numbers
MUTE = (154, 164, 178)      # --text-mute
TEXT = (230, 237, 243)      # --text
GRID = (40, 48, 58)
AREA = (63, 185, 80, 70)    # translucent green fill
RESET_C = (230, 237, 243, 95)

TROVE_OFFSET = -11 * 3600   # Trove server time is a fixed UTC-11

_REG = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans.ttf",
    "C:/Windows/Fonts/segoeui.ttf",
    "C:/Windows/Fonts/arial.ttf",
]
_BOLD = [
    "/usr/share/fonts/truetype/dejavu/DejaVuSans-Bold.ttf",
    "C:/Windows/Fonts/segoeuib.ttf",
    "C:/Windows/Fonts/arialbd.ttf",
]


def _font(size: int, bold: bool = False):
    for path in (_BOLD if bold else _REG):
        try:
            return ImageFont.truetype(path, size)
        except Exception:  # noqa: BLE001 - try the next candidate
            continue
    return ImageFont.load_default()


def _trove(ts: float, fmt: str) -> str:
    """Format a unix instant in Trove server time (UTC-11)."""
    return datetime.fromtimestamp(ts + TROVE_OFFSET, tz=timezone.utc).strftime(fmt)


def _resets(lo: float, hi: float) -> list[float]:
    """Daily reset instants (11:00 UTC = 00:00 Trove) inside [lo, hi]."""
    d = datetime.fromtimestamp(lo, tz=timezone.utc)
    r = datetime(d.year, d.month, d.day, 11, 0, 0, tzinfo=timezone.utc).timestamp()
    if r < lo:
        r += 86400
    out = []
    while r <= hi:
        out.append(r)
        r += 86400
    return out


def _w(draw, text, font) -> int:
    b = draw.textbbox((0, 0), text, font=font)
    return b[2] - b[0]


def _truncate_px(draw, text: str, font, max_w: int) -> str:
    """Trim ``text`` (adding an ellipsis) until it fits within ``max_w`` pixels."""
    if not text or _w(draw, text, font) <= max_w:
        return text
    while text and _w(draw, text + "…", font) > max_w:
        text = text[:-1]
    return text + "…"


def _rel(ts) -> str:
    """A short 'in 2h 14m' / 'now' from a future unix instant (rendered statically)."""
    if not ts:
        return "—"
    delta = int(ts) - int(time.time())
    if delta <= 0:
        return "now"
    days, rem = divmod(delta, 86400)
    hours, rem = divmod(rem, 3600)
    mins = rem // 60
    if days:
        return f"in {days}d {hours}h"
    if hours:
        return f"in {hours}h {mins}m"
    return f"in {mins}m"


async def render_activity_og(period: str = "1d") -> bytes:
    """Fetch the ``period`` series + live estimate and render the card PNG.
    Cached in-process for ``_CACHE_TTL`` per period."""
    period = (period or "1d").lower()
    if period not in VALID_PERIODS:
        period = "1d"
    cached = _cache_get(f"activity:{period}")
    if cached is not None:
        return cached
    from app.trove.leaderboards import activity as lb_activity
    series = await lb_activity.activity_series(period=period)
    live = await lb_activity.estimate_active_players()
    png = _draw(series, live, period)
    _cache_put(f"activity:{period}", png)
    return png


def _draw(series: dict, live: dict, period: str = "1d") -> bytes:
    img = Image.new("RGBA", (W, H), BG + (255,))
    d = ImageDraw.Draw(img, "RGBA")
    M = 64

    f_eyebrow = _font(26, bold=True)
    f_big = _font(104, bold=True)
    f_sub = _font(33)
    f_stat = _font(30, bold=True)
    f_statlbl = _font(19, bold=True)
    f_axis = _font(22)
    f_foot = _font(24)

    d.rectangle([0, 0, W, 6], fill=GREEN + (255,))   # top accent

    # ── header: eyebrow + big live count ──────────────────────────────
    cy = 82
    d.ellipse([M, cy - 8, M + 16, cy + 8], fill=GREEN_HI + (255,))
    d.text((M + 28, cy - 16), "PLAYER ACTIVITY", font=f_eyebrow, fill=MUTE + (255,))

    num = live.get("estimate")
    if num is None and series.get("latest"):
        num = round(series["latest"])
    num_txt = ("~" + f"{int(num):,}") if num is not None else "—"
    d.text((M, 116), num_txt, font=f_big, fill=GREEN_HI + (255,))
    after = M + _w(d, num_txt, f_big) + 24
    d.text((after, 150), "active players", font=f_sub, fill=TEXT + (255,))
    d.text((after, 192), "in the last hour", font=f_sub, fill=MUTE + (255,))

    # right-aligned 24h / 7d rollups
    rx, ry = W - M, 92
    for label, val in (("ACTIVE · 24H", live.get("estimate_24h")),
                       ("ACTIVE · 7D", live.get("estimate_7d"))):
        if val is None:
            continue
        vt = f"~{int(val):,}"
        w = max(_w(d, vt, f_stat), _w(d, label, f_statlbl))
        d.text((rx - w, ry), label, font=f_statlbl, fill=MUTE + (255,))
        d.text((rx - w, ry + 24), vt, font=f_stat, fill=GREEN_HI + (255,))
        ry += 78

    # ── chart ─────────────────────────────────────────────────────────
    px0, px1, py0, py1 = M, W - M, 322, 540
    points = series.get("points") or []
    if len(points) >= 2:
        ws, we = series["window_start"], series["window_end"]
        xs = [p["t"] for p in points]
        xmin, xmax = min(ws, xs[0]), max(we, xs[-1])
        xr = max(1, xmax - xmin)
        ymax = (max(p["active"] for p in points) or 1) * 1.15

        def fx(t):
            return px0 + (t - xmin) / xr * (px1 - px0)

        def fy(v):
            return py1 - (v / ymax) * (py1 - py0)

        line = [(fx(p["t"]), fy(p["active"])) for p in points]
        d.polygon(line + [(line[-1][0], py1), (line[0][0], py1)], fill=AREA)
        d.line([(px0, py1), (px1, py1)], fill=GRID + (255,), width=2)  # baseline

        # Reset markers only on the short ranges (daily/weekly rhythm); on
        # longer ranges they'd be a forest of lines. Labelled only on 1d.
        if period in ("1d", "7d"):
            for r in _resets(xmin, xmax):
                x = fx(r)
                if px0 - 1 <= x <= px1 + 1:
                    y = py0
                    while y < py1:                   # dashed vertical
                        d.line([(x, y), (x, min(y + 9, py1))], fill=RESET_C, width=2)
                        y += 15
                    if period == "1d":
                        d.text((x + 7, py0 - 1), _trove(r, "%H:%M"),
                               font=f_axis, fill=MUTE + (210,))

        d.line(line, fill=GREEN + (255,), width=5, joint="curve")
        lx, ly = line[-1]
        d.ellipse([lx - 7, ly - 7, lx + 7, ly + 7], fill=GREEN_HI + (255,),
                  outline=BG + (255,), width=3)

        # x-axis ticks (Trove time, period-appropriate): start / mid / end
        afmt = _axis_fmt(period)
        for frac in (0.0, 0.5, 1.0):
            t = xmin + frac * xr
            lab = _trove(t, afmt)
            w = _w(d, lab, f_axis)
            tx = fx(t) - (0 if frac == 0 else (w / 2 if frac == 0.5 else w))
            d.text((tx, py1 + 12), lab, font=f_axis, fill=MUTE + (255,))
        d.text((px0, py0 - 32), f"{int(ymax / 1.15):,}", font=f_axis, fill=MUTE + (170,))
    else:
        d.text((M, 400), "Activity data warming up…", font=f_sub, fill=MUTE + (255,))

    # ── footer ────────────────────────────────────────────────────────
    d.text((M, H - 54), "trove.aallyn.net/activity", font=f_foot, fill=MUTE + (255,))
    fr = f"{_PERIOD_LABEL.get(period, 'Last 24 hours')} · Trove server time (UTC−11)"
    d.text((W - M - _w(d, fr, f_foot), H - 54), fr, font=f_foot, fill=MUTE + (210,))

    out = io.BytesIO()
    img.convert("RGB").save(out, format="PNG", optimize=True)
    return out.getvalue()


# ── Status card ────────────────────────────────────────────────────────────
# Binary online/down. "maintenance" is a legacy value kept only so old/stale
# snapshots still render (as red "Down").
_ST_COLOR = {
    "online": (93, 208, 120),
    "down": (248, 81, 73), "maintenance": (248, 81, 73),
    "unknown": (154, 164, 178),
}
_ST_LABEL = {
    "online": "Online", "down": "Down", "maintenance": "Down",
    "unknown": "Unknown",
}
_ST_OVERALL = {
    "online": "All systems online", "down": "Servers down",
    "maintenance": "Servers down", "unknown": "Status unknown",
}


_UPTIME_DAYS = 30


async def render_status_og() -> bytes:
    """Live Trove server-status card with per-region uptime. Cached ~45s."""
    cached = _cache_get("status", ttl=45)
    if cached is not None:
        return cached
    from app.trove import status as trove_status
    payload = trove_status.get_status()
    uptimes: dict[str, float | None] = {}
    for env in ("eu", "us", "pts"):
        try:
            uptimes[env] = (await trove_status.get_history(env, _UPTIME_DAYS)).get("uptime")
        except Exception:  # noqa: BLE001 - uptime is a bonus; never fail the card
            uptimes[env] = None
    png = _draw_status(payload, uptimes)
    _cache_put("status", png)
    return png


def _draw_status(payload: dict, uptimes: dict | None = None) -> bytes:
    uptimes = uptimes or {}
    img = Image.new("RGBA", (W, H), BG + (255,))
    d = ImageDraw.Draw(img, "RGBA")
    M = 64
    overall = payload.get("overall", "unknown")
    oc = _ST_COLOR.get(overall, _ST_COLOR["unknown"])

    f_eyebrow = _font(26, bold=True)
    f_big = _font(86, bold=True)
    f_region = _font(40, bold=True)
    f_status = _font(30)
    f_up = _font(23)
    f_foot = _font(24)

    d.rectangle([0, 0, W, 6], fill=oc + (255,))   # top accent in the status colour

    cy = 82
    d.ellipse([M, cy - 8, M + 16, cy + 8], fill=oc + (255,))
    d.text((M + 28, cy - 16), "TROVE PC SERVER STATUS", font=f_eyebrow, fill=MUTE + (255,))

    environments = payload.get("environments") or {}
    # "down" overall splits into partial (some Live region up) vs full outage.
    if overall == "down" and any(
        (environments.get(k) or {}).get("status") == "online" for k in ("eu", "us")
    ):
        overall_text = "Partial outage"
    else:
        overall_text = _ST_OVERALL.get(overall, "Status unknown")
    d.text((M, 118), overall_text, font=f_big, fill=oc + (255,))

    # EU / US / PTS chips
    envs = (("EU", "eu"), ("US", "us"), ("PTS", "pts"))
    gap, y0, ch = 28, 330, 170
    cw = (W - 2 * M - 2 * gap) // 3
    for i, (label, key) in enumerate(envs):
        x0 = M + i * (cw + gap)
        st = (environments.get(key) or {}).get("status", "unknown")
        sc = _ST_COLOR.get(st, _ST_COLOR["unknown"])
        d.rounded_rectangle([x0, y0, x0 + cw, y0 + ch], radius=20,
                            fill=(22, 27, 34, 255), outline=sc + (130,), width=2)
        d.ellipse([x0 + 30, y0 + 32, x0 + 52, y0 + 54], fill=sc + (255,))
        d.text((x0 + 66, y0 + 22), label, font=f_region, fill=TEXT + (255,))
        d.text((x0 + 30, y0 + 80), _ST_LABEL.get(st, "Unknown"), font=f_status, fill=sc + (255,))
        u = uptimes.get(key)
        up_txt = (f"{u * 100:.2f}% · {_UPTIME_DAYS}d uptime" if u is not None
                  else f"uptime n/a · {_UPTIME_DAYS}d")
        d.text((x0 + 30, y0 + 124), up_txt, font=f_up, fill=MUTE + (255,))

    d.text((M, H - 54), "trove.aallyn.net/status", font=f_foot, fill=MUTE + (255,))
    ca = payload.get("checked_at")
    if ca:
        ago = max(0, int(time.time()) - int(ca))
        rel = f"{ago}s ago" if ago < 60 else (f"{ago // 60}m ago" if ago < 3600 else f"{ago // 3600}h ago")
        fr = f"Checked {rel}"
    else:
        fr = "Live status"
    d.text((W - M - _w(d, fr, f_foot), H - 54), fr, font=f_foot, fill=MUTE + (210,))

    out = io.BytesIO()
    img.convert("RGB").save(out, format="PNG", optimize=True)
    return out.getvalue()


# ── Live "Trove Now" board card ──────────────────────────────────────────────
# One image holding the whole board (status + challenge + chaos + biomes +
# merchants + resets). Served at GET /board.png and referenced by the Discord
# bot's board feature, so it's rendered at most ONCE per minute and shared via
# Redis - 100 guilds (and every API worker) reuse the same render.

_BOARD_TTL = 120


async def render_board_image() -> bytes:
    """The live 'Trove Now' board as a 1200x630 PNG. Cached in Redis keyed by the
    minute, so every guild + every API worker shares one render per minute."""
    minute = int(time.time() // 60)
    key = f"board:img:{minute}"
    from app.core.redis import get_redis
    redis = get_redis()
    if redis is not None:
        try:
            cached = await redis.get(key)        # base64 str (client decodes responses)
            if cached:
                return base64.b64decode(cached)
        except Exception:  # noqa: BLE001 - cache is best-effort
            logger.warning("board image: Redis read failed", exc_info=True)

    from app.trove import status as trove_status
    from app.trove.captures import get_current_challenge
    from app.trove.chaos import get_chaos_chest
    from app.trove.rotations import biome_rotation
    from app.trove.server_time import corruxion, fluxion, server_time

    data = {
        "challenge": await get_current_challenge(),
        "chaos": await get_chaos_chest(),
        "biomes": biome_rotation().get("current") or {},
        "corruxion": corruxion(),
        "fluxion": fluxion(),
        "server": server_time(),
        "status": trove_status.get_status(),     # in-process cache (API process)
    }
    png = _draw_board(data)

    if redis is not None:
        try:
            await redis.set(key, base64.b64encode(png).decode(), ex=_BOARD_TTL)
        except Exception:  # noqa: BLE001
            logger.warning("board image: Redis write failed", exc_info=True)
    return png


# Per-card accent colours (the board has no emoji - DejaVu can't render them; the
# left accent bar carries the visual coding instead).
_BOARD_CARDS = (
    ("HOURLY CHALLENGE", (255, 152, 0)),
    ("CHAOS CHEST", (183, 148, 246)),
    ("DEPTH-15 BIOMES", (155, 138, 251)),
    ("RESETS", (94, 198, 255)),
    ("CORRUXION", (155, 93, 229)),
    ("FLUXION", (76, 201, 240)),
)


def _merchant_lines(m: dict) -> tuple[str, str]:
    if m.get("active"):
        stage = m.get("state")
        head = "Here now" + (f" · {stage}" if stage and stage != "away" else "")
        return head, f"Leaves {_rel(m.get('ends_at'))}"
    return "Away", f"Arrives {_rel(m.get('starts_at'))}"


def _draw_board(data: dict) -> bytes:
    img = Image.new("RGBA", (W, H), BG + (255,))
    d = ImageDraw.Draw(img, "RGBA")
    M = 56
    overall = (data["status"] or {}).get("overall", "unknown")
    oc = _ST_COLOR.get(overall, _ST_COLOR["unknown"])

    f_eyebrow = _font(24, bold=True)
    f_head = _font(60, bold=True)
    f_title = _font(20, bold=True)
    f_val = _font(33, bold=True)
    f_sub = _font(23)
    f_foot = _font(23)

    d.rectangle([0, 0, W, 6], fill=oc + (255,))

    # header: status headline (colours the whole card)
    d.ellipse([M, 54, M + 16, 70], fill=oc + (255,))
    d.text((M + 30, 50), "TROVE NOW · LIVE", font=f_eyebrow, fill=MUTE + (255,))
    d.text((M, 84), _ST_OVERALL.get(overall, "Status unknown"), font=f_head, fill=oc + (255,))

    # values per card
    cur = data["challenge"]
    challenge_on = bool(cur.get("active") and cur.get("name"))
    if challenge_on:
        ch_val = cur["name"]
        ch_sub = f"Ends {_rel(cur.get('ends_at'))}"
    elif cur.get("active"):
        ch_val, ch_sub = "No Challenge", "Starting…"   # window open, not captured yet
    else:                                              # the gap between challenges
        interval = 1800 if cur.get("is_friday_window") else 3600
        nxt = (cur.get("starts_at") or 0) + interval
        ch_val, ch_sub = "No Challenge", (f"Next {_rel(nxt)}" if nxt else "")

    item = (data["chaos"].get("item") or {}).get("name")
    chaos_val = item or "—"
    chaos_sub = f"Rotates {_rel(data['chaos'].get('ends_at'))}" if item else "Not relayed yet"

    bnames = [
        n for n in (b.get("final_name") or b.get("name")
                    for b in (data["biomes"].get("biomes") or []))
        if n
    ]
    biome_rotate = f"rotates {_rel(data['biomes'].get('ends_at'))}"

    s = data["server"]
    reset_val = f"Daily {_rel(s['daily_reset_at'])}"
    reset_sub = f"Weekly {_rel(s['weekly_reset_at'])}"

    cx_val, cx_sub = _merchant_lines(data["corruxion"])
    fx_val, fx_sub = _merchant_lines(data["fluxion"])

    # (value, sub) per card; the biomes card (index 2) is a stacked list, drawn below.
    values = [
        (ch_val, ch_sub), (chaos_val, chaos_sub), None,
        (reset_val, reset_sub), (cx_val, cx_sub), (fx_val, fx_sub),
    ]

    f_biome = _font(22, bold=True)
    grey = (90, 96, 106)        # dimmed accent for the "No Challenge" card
    gap = 20
    cw = (W - 2 * M - gap) // 2
    ch_h, y0 = 118, 172
    for i, (title, accent) in enumerate(_BOARD_CARDS):
        muted = i == 0 and not challenge_on        # grey out the hourly-challenge card
        accent = grey if muted else accent
        x = M + (i % 2) * (cw + gap)
        y = y0 + (i // 2) * (ch_h + gap)
        d.rounded_rectangle([x, y, x + cw, y + ch_h], radius=18,
                            fill=(22, 27, 34, 255), outline=accent + (90,), width=2)
        d.rounded_rectangle([x, y, x + 6, y + ch_h], radius=3, fill=accent + (255,))
        d.text((x + 26, y + 16), title, font=f_title, fill=MUTE + (255,))
        if values[i] is None:        # Depth-15 biomes: list all three + rotate time
            d.text((x + cw - 26 - _w(d, biome_rotate, f_title), y + 16),
                   biome_rotate, font=f_title, fill=MUTE + (255,))
            for j, name in enumerate(bnames[:3] or ["—"]):
                d.text((x + 26, y + 44 + j * 23),
                       _truncate_px(d, name, f_biome, cw - 52), font=f_biome, fill=TEXT + (255,))
        else:
            val, sub = values[i]
            d.text((x + 26, y + 44), _truncate_px(d, val, f_val, cw - 52), font=f_val,
                   fill=(MUTE if muted else TEXT) + (255,))
            d.text((x + 26, y + 84), _truncate_px(d, sub, f_sub, cw - 52), font=f_sub, fill=MUTE + (255,))

    # footer
    d.text((M, H - 46), "trove.aallyn.net", font=f_foot, fill=MUTE + (255,))
    fr = "Updated " + datetime.now(timezone.utc).strftime("%H:%M UTC")
    d.text((W - M - _w(d, fr, f_foot), H - 46), fr, font=f_foot, fill=MUTE + (210,))

    out = io.BytesIO()
    img.convert("RGB").save(out, format="PNG", optimize=True)
    return out.getvalue()


# ── Per-announcement card (the image announcements embed) ────────────────────
# One banner per announcement kind, rendered API-side and referenced by the bot's
# image announcements. Cached in Redis per (kind, minute) so 100 guilds share one
# render per minute. No emoji (DejaVu can't); the accent bar carries the colour.

_ANN_W, _ANN_H = 1200, 420
_ANN_TITLE = {
    "hourly_challenge": "Hourly Challenge", "chaos_chest": "Chaos Chest",
    "daily_bonuses": "Daily Bonus", "longshade": "Depth-15 Biomes",
    "wild_mana": "Wild Mana", "stampy": "Stampy Event",
    "corruxion": "Corruxion Merchant", "fluxion": "Fluxion Merchant",
    "server_status": "Trove Server Status",
}
_ANN_ACCENT = {
    "hourly_challenge": (255, 152, 0), "chaos_chest": (183, 148, 246),
    "daily_bonuses": (255, 184, 107), "longshade": (155, 138, 251),
    "wild_mana": (70, 211, 154), "stampy": (255, 184, 107),
    "corruxion": (155, 93, 229), "fluxion": (76, 201, 240),
}


async def _announcement_content(kind: str) -> tuple[str, tuple, list[str]]:
    """(title, accent, lines) for a kind. lines[0] is the headline; the rest are
    sub-lines. Plain strings computed now (no Discord markup)."""
    title = _ANN_TITLE.get(kind, kind.replace("_", " ").title())
    accent = _ANN_ACCENT.get(kind, GREEN)
    lines: list[str] = []

    if kind == "hourly_challenge":
        from app.trove.captures import get_current_challenge
        c = await get_current_challenge()
        sub = (f"Ends {_rel(c.get('ends_at'))}" if c.get("active")
               else f"Next window {_rel(c.get('starts_at'))}")
        lines = [c.get("name") or "No capture yet"]
        if c.get("type"):
            lines.append(c["type"].title() + " challenge")
        lines.append(sub)
    elif kind == "chaos_chest":
        from app.trove.chaos import get_chaos_chest
        c = await get_chaos_chest()
        item = (c.get("item") or {}).get("name")
        lines = [item or "Not relayed yet"]
        if item:
            lines.append(f"Rotates {_rel(c.get('ends_at'))}")
    elif kind == "daily_bonuses":
        from app.trove.server_time import daily_buffs, server_time
        d = (daily_buffs() or {}).get("current") or {}
        lines = [d.get("name") or "Daily bonus"]
        lines += (d.get("normal_buffs") or [])[:2]
        lines.append(f"Resets {_rel(server_time()['daily_reset_at'])}")
    elif kind in ("longshade", "wild_mana"):
        from app.trove.rotations import biome_rotation, wild_mana
        cur = (biome_rotation() if kind == "longshade" else wild_mana()).get("current") or {}
        names = [b.get("final_name") or b.get("name") for b in (cur.get("biomes") or [])]
        lines = [n for n in names if n] or ["—"]
        lines.append(f"Rotates {_rel(cur.get('ends_at'))}")
    elif kind == "stampy":
        from app.trove.rotations import stampy
        cur = stampy().get("current") or {}
        b = (cur.get("biomes") or [{}])[0]
        lines = [b.get("final_name") or b.get("name") or "Stampy event",
                 f"Ends {_rel(cur.get('ends_at'))}"]
    elif kind in ("corruxion", "fluxion"):
        from app.trove.server_time import corruxion, fluxion
        m = corruxion() if kind == "corruxion" else fluxion()
        if m.get("active"):
            stage = m.get("state")
            lines = ["Here now" + (f" · {stage}" if stage and stage != "away" else ""),
                     f"Leaves {_rel(m.get('ends_at'))}"]
        else:
            lines = ["Away", f"Arrives {_rel(m.get('starts_at'))}"]
    elif kind == "server_status":
        from app.trove import status as trove_status
        s = trove_status.get_status()
        overall = s.get("overall", "unknown")
        accent = _ST_COLOR.get(overall, _ST_COLOR["unknown"])
        envs = s.get("environments") or {}
        lines = [_ST_OVERALL.get(overall, "Status unknown")]
        lines += [f"{lbl}: {_ST_LABEL.get((envs.get(k) or {}).get('status', 'unknown'), 'Unknown')}"
                  for lbl, k in (("EU", "eu"), ("US", "us"), ("PTS", "pts"))]
    return title, accent, lines


def _draw_announcement(title: str, accent: tuple, lines: list[str]) -> bytes:
    img = Image.new("RGBA", (_ANN_W, _ANN_H), BG + (255,))
    d = ImageDraw.Draw(img, "RGBA")
    M = 64
    f_eyebrow = _font(26, bold=True)
    f_head = _font(64, bold=True)
    f_line = _font(32)
    f_foot = _font(23)

    d.rectangle([0, 0, _ANN_W, 6], fill=accent + (255,))
    d.ellipse([M, 54, M + 18, 72], fill=accent + (255,))
    d.text((M + 30, 50), title.upper(), font=f_eyebrow, fill=MUTE + (255,))

    headline = lines[0] if lines else "—"
    d.text((M, 90), _truncate_px(d, headline, f_head, _ANN_W - 2 * M), font=f_head, fill=accent + (255,))
    y = 188
    for ln in lines[1:5]:
        d.text((M, y), _truncate_px(d, ln, f_line, _ANN_W - 2 * M), font=f_line, fill=TEXT + (255,))
        y += 44

    d.text((M, _ANN_H - 46), "trove.aallyn.net", font=f_foot, fill=MUTE + (255,))
    fr = "Updated " + datetime.now(timezone.utc).strftime("%H:%M UTC")
    d.text((_ANN_W - M - _w(d, fr, f_foot), _ANN_H - 46), fr, font=f_foot, fill=MUTE + (210,))

    out = io.BytesIO()
    img.convert("RGB").save(out, format="PNG", optimize=True)
    return out.getvalue()


async def render_announcement_image(kind: str) -> bytes:
    """A single announcement banner PNG for ``kind``. Cached in Redis per
    (kind, minute) so every guild + worker shares one render per minute."""
    minute = int(time.time() // 60)
    key = f"announce:img:{kind}:{minute}"
    from app.core.redis import get_redis
    redis = get_redis()
    if redis is not None:
        try:
            cached = await redis.get(key)
            if cached:
                return base64.b64decode(cached)
        except Exception:  # noqa: BLE001
            logger.warning("announcement image: Redis read failed", exc_info=True)

    title, accent, lines = await _announcement_content(kind)
    png = _draw_announcement(title, accent, lines)

    if redis is not None:
        try:
            await redis.set(key, base64.b64encode(png).decode(), ex=_BOARD_TTL)
        except Exception:  # noqa: BLE001
            logger.warning("announcement image: Redis write failed", exc_info=True)
    return png
