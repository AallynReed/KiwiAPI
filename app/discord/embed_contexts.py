"""Per-announcement-type variable contexts + default templates for the bot.

A guild that customizes a type renders ``render_template(custom, context)``; a guild
that doesn't keeps calling the original ``build_embed`` in ``app/discord/embeds.py``
(the untouched, fully-localized default) - no behaviour change, no i18n regression.
Composite vars (``current_line``, ``status_lines``, …) keep the conditional/multiline
formatting in code so templates stay simple.
"""

from __future__ import annotations

import inspect
import time

from app.discord.embeds import _ASSET, SITE, _ts, _unix
from app.embed_templates import EmbedField, EmbedTemplate
from app.i18n import t

# announcement registry key -> context "kind" (the 5 challenge_* keys share one)
KIND_BY_KEY = {
    "challenge_collection": "challenge", "challenge_rampage": "challenge",
    "challenge_racing": "challenge", "challenge_target": "challenge",
    "challenge_dungeon": "challenge",
    "chaos_chest": "chaos", "daily_bonuses": "daily_bonuses",
    "longshade": "longshade", "wild_mana": "wild_mana", "stampy": "stampy",
    "corruxion": "corruxion", "fluxion": "fluxion", "trove_news": "trove_news",
    "giveaways": "giveaways", "activity": "activity",
    "server_status": "server_status", "game_update": "game_update",
}


def _biomes(rot: dict) -> str:
    return "\n".join(f"• {b.get('final_name') or b.get('name')}"
                     for b in (rot.get("biomes") or [])) or "—"


# ── challenge ────────────────────────────────────────────────────────────────

async def _ctx_challenge() -> dict:
    from app.trove.captures import get_current_challenge
    c = await get_current_challenge()
    name, ctype = c.get("name"), (c.get("type") or "")
    if name:
        line = f"**{name}**" + (f" · {ctype.title()}" if ctype else "")
        line += "\n" + (t("Ends {when}", when=_ts(c["ends_at"], "R")) if c.get("active")
                        else t("Next window {when}", when=_ts(c["starts_at"], "R")))
    else:
        line = t("No challenge captured for this window.")
    return {"name": name or "", "type": ctype, "type_label": ctype.title(),
            "starts_at": c.get("starts_at"), "ends_at": c.get("ends_at"),
            "status": "live" if c.get("active") else "upcoming", "current_line": line}


def _default_challenge() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("⚔️ Hourly challenge"), color="#FF9800",
        description=t("Trove's hourly challenge - 20 minutes at the top of each hour."),
        fields=[EmbedField(name=t("Current"), value="{current_line}", inline=False)],
        footer=t("Times shown in your local zone"), show_image=False)


def _sample_challenge() -> dict:
    return {"name": "Racing Challenge", "type": "racing", "type_label": "Racing",
            "starts_at": 1718200000, "ends_at": 1718201200, "status": "live",
            "current_line": "**Racing Challenge** · Racing\nEnds in 18 minutes"}


# ── chaos ────────────────────────────────────────────────────────────────────

async def _ctx_chaos() -> dict:
    from app.trove.chaos import get_chaos_chest
    c = await get_chaos_chest()
    name = (c.get("item") or {}).get("name")
    return {"item_name": name or "",
            "desc_line": (t("This week's featured item: **{name}**", name=name) if name
                          else t("No item has been relayed for this week's chest yet.")),
            "starts_at": c.get("starts_at"), "ends_at": c.get("ends_at")}


def _default_chaos() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🎁 Chaos Chest"), color="#B794F6", description="{desc_line}",
        fields=[
            EmbedField(name=t("Rotates"), value="{ends_at:R}\n{ends_at:f}", inline=True),
            EmbedField(name=t("This week"), value="{starts_at:d} → {ends_at:d}", inline=True),
        ],
        footer=t("Trove Chaos Chest · rotates weekly (Tue 11:00 UTC)"), show_image=False)


def _sample_chaos() -> dict:
    return {"item_name": "Radiant Mount", "desc_line": "This week's featured item: **Radiant Mount**",
            "starts_at": 1718000000, "ends_at": 1718600000}


# ── daily_bonuses ────────────────────────────────────────────────────────────

def _ctx_daily() -> dict:
    from app.trove.rotations import daily_buffs, weekly_buffs
    d = (daily_buffs() or {}).get("current") or {}
    w = (weekly_buffs() or {}).get("current") or {}
    daily_val = "\n".join(f"• {b}" for b in (d.get("normal_buffs") or [])) or "—"
    if d.get("premium_buffs"):
        daily_val += "\n" + t("*Patron:* {buffs}", buffs=", ".join(d["premium_buffs"]))
    return {"daily_name": d.get("name", "?"), "daily_weekday": d.get("weekday", ""),
            "daily_emoji": d.get("emoji", "📅"), "daily_buffs": daily_val,
            "weekly_name": w.get("name", "?"), "weekly_emoji": w.get("emoji", "🗓️"),
            "weekly_buffs": "\n".join(f"• {b}" for b in (w.get("buffs") or [])) or "—"}


def _default_daily() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("✨ Trove bonuses"), color="#FFB86B",
        fields=[
            EmbedField(name="{daily_emoji} " + t("Daily — {name} ({weekday})",
                       name="{daily_name}", weekday="{daily_weekday}"),
                       value="{daily_buffs}", inline=False),
            EmbedField(name="{weekly_emoji} " + t("Weekly — {name}", name="{weekly_name}"),
                       value="{weekly_buffs}", inline=False),
        ],
        footer=t("Daily resets 11:00 UTC · weekly Monday"), show_image=False)


def _sample_daily() -> dict:
    return {"daily_name": "Combat", "daily_weekday": "Friday", "daily_emoji": "⚔️",
            "daily_buffs": "• +50% Combat XP\n• +25% Loot", "weekly_name": "Geode",
            "weekly_emoji": "🗓️", "weekly_buffs": "• +Geode topside drops"}


# ── longshade / wild_mana (shared biome shape) ───────────────────────────────

def _ctx_biome_rotation(fetch) -> dict:
    r = fetch()
    cur = r.get("current") or {}
    nxt = (r.get("upcoming") or [None])[0] or {}
    return {"current_biomes": _biomes(cur), "rotates_at": cur.get("ends_at"),
            "next_biomes": _biomes(nxt) if nxt else "—", "next_at": nxt.get("starts_at")}


def _ctx_longshade() -> dict:
    from app.trove.rotations import biome_rotation
    return _ctx_biome_rotation(biome_rotation)


def _default_longshade() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🌑 Depth 15 biomes"), color="#9B8AFB",
        description=t("Current Depth-15 adventure biome rotation (long shade)."),
        fields=[
            EmbedField(name=t("Current biomes"), value="{current_biomes}", inline=True),
            EmbedField(name=t("Rotates"), value="{rotates_at:R}", inline=True),
            EmbedField(name=t("Up next"), value="{next_biomes}", inline=False),
        ],
        footer=t("Rotates every 3 hours · Trove server time (UTC−11)"), show_image=False)


def _ctx_wild_mana() -> dict:
    from app.trove.rotations import wild_mana
    return _ctx_biome_rotation(wild_mana)


def _default_wild_mana() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🌿 Wild Mana"), color="#46D39A",
        description=t("Weekly Wild Mana biome rotation."),
        fields=[
            EmbedField(name=t("This week's biomes"), value="{current_biomes}", inline=True),
            EmbedField(name=t("Rotates"), value="{rotates_at:R}", inline=True),
            EmbedField(name=t("Next week"), value="{next_biomes}", inline=False),
        ],
        footer=t("Rotates weekly · Trove server time (UTC−11)"), show_image=False)


def _sample_biome() -> dict:
    return {"current_biomes": "• Permafrost\n• Cursed Vale\n• Jurassic Jungle",
            "rotates_at": 1718200000, "next_biomes": "• Fae Forest\n• Neon City",
            "next_at": 1718210000}


# ── stampy ───────────────────────────────────────────────────────────────────

def _ctx_stampy() -> dict:
    from app.trove.rotations import stampy
    s = stampy()
    cur = s.get("current") or {}
    biome = (cur.get("biomes") or [{}])[0]
    now = int(time.time())
    if cur and cur.get("starts_at", 0) <= now < cur.get("ends_at", 0):
        state = t("**Active now** — ends {when}", when=_ts(cur["ends_at"], "R"))
    elif cur:
        state = t("Starts {when}", when=_ts(cur["starts_at"], "R"))
    else:
        state = t("No upcoming Stampy event scheduled.")
    return {"biome": biome.get("final_name") or biome.get("name") or "?",
            "starts_at": cur.get("starts_at"), "ends_at": cur.get("ends_at"),
            "state_line": state}


def _default_stampy() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🐘 Stampy"), color="#FFB86B", description="{state_line}",
        fields=[
            EmbedField(name=t("Biome"), value="{biome}", inline=True),
            EmbedField(name=t("Window"), value="{starts_at:d} → {ends_at:d}", inline=True),
        ],
        footer=t("Stampy event · 48-hour window"), show_image=False)


def _sample_stampy() -> dict:
    return {"biome": "Candoria", "starts_at": 1718200000, "ends_at": 1718372800,
            "state_line": "**Active now** — ends in 1 day"}


# ── corruxion / fluxion (merchants) ──────────────────────────────────────────

def _ctx_corruxion() -> dict:
    from app.trove.rotations import corruxion
    c = corruxion()
    if c.get("active"):
        desc = t("**Corruxion is here!**") + "\n" + t("Leaves {when}", when=_ts(c["ends_at"], "R"))
    else:
        desc = t("Corruxion is away.") + "\n" + t("Arrives {when}", when=_ts(c["starts_at"], "R"))
    up = (c.get("schedule") or [])[1:5]
    return {"status": "here" if c.get("active") else "away",
            "starts_at": c.get("starts_at"), "ends_at": c.get("ends_at"),
            "desc_line": desc,
            "upcoming": "\n".join(f"{_ts(w['starts_at'], 'd')} → {_ts(w['ends_at'], 'd')}" for w in up) or "—"}


def _default_corruxion() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🐲 Corruxion"), color="#9B5DE5", description="{desc_line}",
        fields=[EmbedField(name=t("Upcoming visits"), value="{upcoming}", inline=False)],
        footer=t("Corruxion merchant · 14-day cycle, 3-day visits"), show_image=False)


def _ctx_fluxion() -> dict:
    from app.trove.rotations import fluxion
    f = fluxion()
    if f.get("active"):
        desc = (t("**Fluxion is here** — currently in the **{state}** stage.", state=f.get("state", "?"))
                + "\n" + t("Leaves {when}", when=_ts(f["ends_at"], "R")))
    else:
        stage = (f.get("schedule") or [{}])[0].get("state", "?")
        desc = t("Fluxion is away.") + "\n" + t("Next up: the **{stage}** stage, {when}",
                                                stage=stage, when=_ts(f["starts_at"], "R"))
    up = (f.get("schedule") or [])[1:5]
    return {"status": "here" if f.get("active") else "away", "state": f.get("state", ""),
            "starts_at": f.get("starts_at"), "ends_at": f.get("ends_at"), "desc_line": desc,
            "upcoming": "\n".join(f"{_ts(w['starts_at'], 'd')} → {_ts(w['ends_at'], 'd')} · {w.get('state', '')}" for w in up) or "—"}


def _default_fluxion() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("💧 Fluxion"), color="#4CC9F0", description="{desc_line}",
        fields=[EmbedField(name=t("Upcoming stages"), value="{upcoming}", inline=False)],
        footer=t("Fluxion merchant · alternates voting / selling"), show_image=False)


def _sample_merchant() -> dict:
    return {"status": "here", "state": "selling", "starts_at": 1718200000, "ends_at": 1718459200,
            "desc_line": "**Here now!**\nLeaves in 2 days",
            "upcoming": "Jun 20 → Jun 23\nJul 04 → Jul 07"}


# ── trove_news ───────────────────────────────────────────────────────────────

async def _ctx_news() -> dict:
    from app.trove.news import latest_news
    items = await latest_news(5)
    if not items:
        return {"title": "", "url": SITE, "summary": "", "more": "—", "image_url": None}
    top = items[0]
    more = "\n".join(f"• [{n.title}]({n.url})" for n in items[1:5]) or "—"
    return {"title": top.title, "url": top.url, "summary": (top.summary or "")[:600],
            "more": more, "image_url": getattr(top, "image", None)}


def _default_news() -> EmbedTemplate:
    return EmbedTemplate(
        title="📰 {title}", url="{url}", color="#5EC6FF", description="{summary}",
        fields=[EmbedField(name=t("More"), value="{more}", inline=False)],
        footer=t("Latest from trovegame.com"), show_image=True)


def _sample_news() -> dict:
    return {"title": "Summer Update is live!", "url": f"{SITE}", "image_url": None,
            "summary": "Patch notes, new mounts, and a seasonal event.",
            "more": "• [Dev blog](https://trovegame.com)\n• [Known issues](https://trovegame.com)"}


# ── giveaways ────────────────────────────────────────────────────────────────

async def _ctx_giveaways() -> dict:
    from app.giveaways import service
    ongoing = await service.list_ongoing()
    upcoming = await service.list_upcoming()
    ended = await service.list_ended(days=7)

    def _open(g):
        return f"**{g.title}** — {g.prize_name}\n" + t("Ends {when} · {n} entered",
                                                       when=_ts(_unix(g.ends_at), "R"), n=g.entry_count)
    return {"open_count": len(ongoing), "upcoming_count": len(upcoming), "ended_count": len(ended),
            "open_lines": "\n\n".join(_open(g) for g in ongoing[:5]) or t("None right now."),
            "upcoming_lines": "\n".join(f"**{g.title}** — {g.prize_name}" for g in upcoming[:5]) or t("Nothing scheduled."),
            "ended_lines": "\n".join(f"**{g.title}** — {g.prize_name}" for g in ended[:5]) or t("None in the last 7 days.")}


def _default_giveaways() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🎉 Giveaways"), url=f"{SITE}/giveaways", color="#FF5C80",
        fields=[
            EmbedField(name=t("🟢 Open now"), value="{open_lines}", inline=False),
            EmbedField(name=t("🔜 Upcoming"), value="{upcoming_lines}", inline=False),
            EmbedField(name=t("🏁 Ended · last 7 days"), value="{ended_lines}", inline=False),
        ],
        footer="trove.aallyn.net/giveaways", show_image=False)


def _sample_giveaways() -> dict:
    return {"open_count": 1, "upcoming_count": 1, "ended_count": 2,
            "open_lines": "**Holiday code drop** — Mount Code\nEnds in 3 days · 42 entered",
            "upcoming_lines": "**Next week** — Mystery Prize", "ended_lines": "**Last draw** — Glim Pack"}


# ── activity ─────────────────────────────────────────────────────────────────

async def _ctx_activity() -> dict:
    from app.trove.leaderboards.activity import activity_series, estimate_active_players
    live = await estimate_active_players()
    series = await activity_series("7d")
    peak = series.get("peak") or {}
    cb = live.get("computed_at") or series.get("window_end") or 0
    return {"label": "Last 7 days",
            "active_now": f"{live.get('estimate', 0):,}", "last_24h": f"{live.get('estimate_24h', 0):,}",
            "last_7d": f"{live.get('estimate_7d', 0):,}",
            "peak": f"{round(peak['active']):,}/h" if isinstance(peak.get("active"), (int, float)) else "—",
            "average": f"{round(series['average']):,}/h" if isinstance(series.get("average"), (int, float)) else "—",
            "image_url": f"{_ASSET}/activity/og.png?period=7d&v={cb}"}


def _default_activity() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("Player activity — {label}", label="{label}"), url=f"{SITE}/activity",
        color="#4CC9F0",
        description=t("Estimated distinct active players, from hourly leaderboard captures."),
        fields=[
            EmbedField(name=t("Active now"), value="{active_now}", inline=True),
            EmbedField(name=t("Last 24h"), value="{last_24h}", inline=True),
            EmbedField(name=t("Last 7d"), value="{last_7d}", inline=True),
            EmbedField(name=t("Peak"), value="{peak}", inline=True),
            EmbedField(name=t("Average"), value="{average}", inline=True),
        ],
        footer="trove.aallyn.net/activity · Trove server time (UTC−11)", show_image=True)


def _sample_activity() -> dict:
    return {"label": "Last 7 days", "active_now": "4,231", "last_24h": "18,764",
            "last_7d": "52,310", "peak": "5,120/h", "average": "3,480/h", "image_url": None}


# ── server_status ────────────────────────────────────────────────────────────

async def _ctx_status() -> dict:
    from app.discord.embeds import _OVERALL, _dot
    from app.trove.status import get_status_shared
    snap = await get_status_shared()
    overall = snap.get("overall", "unknown")
    envs = snap.get("environments") or {}

    def _line(k):
        env = envs.get(k) or {}
        st = env.get("status", "unknown")
        return f"{_dot(st)} {st.title()}"
    auth = snap.get("auth")
    auth_line = (t("Checking…") if auth is None
                 else (t("Reachable") if auth.get("online") else t("Unreachable")))
    if overall == "down" and any((envs.get(k) or {}).get("status") == "online" for k in ("eu", "us")):
        overall_text = t("Partial outage")
    else:
        overall_text = t(_OVERALL.get(overall, _OVERALL["unknown"]))
    return {"overall": overall, "overall_text": overall_text, "overall_dot": _dot(overall),
            "eu_line": _line("eu"), "us_line": _line("us"), "pts_line": _line("pts"),
            "auth_line": auth_line}


def _default_status() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("Trove server status"), url=f"{SITE}/status", color="#4CC9F0",
        description="{overall_dot} **{overall_text}**",
        fields=[
            EmbedField(name="EU", value="{eu_line}", inline=True),
            EmbedField(name="US", value="{us_line}", inline=True),
            EmbedField(name="PTS", value="{pts_line}", inline=True),
            EmbedField(name=t("Account auth"), value="{auth_line}", inline=False),
        ],
        footer="trove.aallyn.net/status", show_image=False)


def _sample_status() -> dict:
    return {"overall": "online", "overall_text": "Online", "overall_dot": "🟢",
            "eu_line": "🟢 Online", "us_line": "🟢 Online", "pts_line": "🟡 Maintenance",
            "auth_line": "Reachable"}


# ── game_update ──────────────────────────────────────────────────────────────

async def _ctx_game_update() -> dict:
    from app.trove.updates import read as updates_read
    v = await updates_read.latest_version("live-us")
    if v is None:
        return {"version_tag": "new build", "files_added": "0", "files_modified": "0",
                "files_removed": "0", "ordinal": None, "image_url": None}
    return {"version_tag": v.version_tag, "files_added": f"{v.files_added:,}",
            "files_modified": f"{v.files_modified:,}", "files_removed": f"{v.files_removed:,}",
            "ordinal": v.ordinal, "image_url": f"{_ASSET}/announce.png?kind=game_update&v={v.ordinal}"}


def _default_game_update() -> EmbedTemplate:
    return EmbedTemplate(
        title=t("🧩 New Trove update — {version}", version="{version_tag}"),
        url=f"{SITE}/updates", color="#5EC6FF",
        description=t("A new build is live on the US servers. See what changed:"),
        fields=[
            EmbedField(name=t("Added"), value="{files_added}", inline=True),
            EmbedField(name=t("Modified"), value="{files_modified}", inline=True),
            EmbedField(name=t("Removed"), value="{files_removed}", inline=True),
        ],
        footer=t("Browse the changed files at trove.aallyn.net/updates"), show_image=True)


def _sample_game_update() -> dict:
    return {"version_tag": "2024.06.01", "files_added": "12", "files_modified": "8",
            "files_removed": "1", "ordinal": 142, "image_url": None}


# ── registry ─────────────────────────────────────────────────────────────────

# kind -> (context_fn, default_template_fn, sample_fn). Image-bearing kinds let the
# editor toggle the banner.
_KINDS = {
    "challenge": (_ctx_challenge, _default_challenge, _sample_challenge),
    "chaos": (_ctx_chaos, _default_chaos, _sample_chaos),
    "daily_bonuses": (_ctx_daily, _default_daily, _sample_daily),
    "longshade": (_ctx_longshade, _default_longshade, _sample_biome),
    "wild_mana": (_ctx_wild_mana, _default_wild_mana, _sample_biome),
    "stampy": (_ctx_stampy, _default_stampy, _sample_stampy),
    "corruxion": (_ctx_corruxion, _default_corruxion, _sample_merchant),
    "fluxion": (_ctx_fluxion, _default_fluxion, _sample_merchant),
    "trove_news": (_ctx_news, _default_news, _sample_news),
    "giveaways": (_ctx_giveaways, _default_giveaways, _sample_giveaways),
    "activity": (_ctx_activity, _default_activity, _sample_activity),
    "server_status": (_ctx_status, _default_status, _sample_status),
    "game_update": (_ctx_game_update, _default_game_update, _sample_game_update),
}
IMAGE_KINDS = {"trove_news", "activity", "game_update"}

# Bindable "kinds" + display labels, for the image studio's variable binding.
KINDS: tuple[str, ...] = tuple(_KINDS)
KIND_LABELS = {
    "challenge": "Hourly challenge", "chaos": "Chaos Chest",
    "daily_bonuses": "Daily bonuses", "longshade": "Depth-15 biomes",
    "wild_mana": "Wild Mana", "stampy": "Stampy", "corruxion": "Corruxion",
    "fluxion": "Fluxion", "trove_news": "Trove news", "giveaways": "Giveaways",
    "activity": "Player activity", "server_status": "Server status",
    "game_update": "Game update",
}


def _kind(key: str | None) -> str | None:
    """Resolve a kind from either an announcement key (``challenge_racing``) or a bare
    kind (``challenge``)."""
    if key in _KINDS:
        return key
    return KIND_BY_KEY.get(key)


def has_customization(key: str) -> bool:
    return key in KIND_BY_KEY


def is_bindable(key: str | None) -> bool:
    """True if ``key`` (an announcement key or a bare kind) resolves to a context."""
    return _kind(key) is not None


async def context(key: str) -> dict:
    """Live variables for ``key`` (awaits the source if it's async)."""
    spec = _KINDS.get(_kind(key))
    if spec is None:
        return {}
    out = spec[0]()
    return await out if inspect.isawaitable(out) else out


def default_template(key: str) -> EmbedTemplate | None:
    spec = _KINDS.get(_kind(key))
    return spec[1]() if spec else None


def sample_context(key: str) -> dict:
    spec = _KINDS.get(_kind(key))
    return spec[2]() if spec else {}


def variables(key: str) -> list[str]:
    return [k for k in sample_context(key) if k != "image_url"]


def has_image(key: str) -> bool:
    return _kind(key) in IMAGE_KINDS
