"""Announcement-type registry for the Kiwi gateway bot.

Each entry pairs a Discord embed builder (app/discord/embeds.py) with an
``anchor`` coroutine that returns the current edge-trigger token for that type -
a short string that changes exactly when there's something new to announce, or
``None`` when there's nothing to announce right now (gap window between hourly
challenges, a merchant that's away, an empty feed, ...).

The announcer (app/bot/announcer.py) posts a type's embed to a guild once per
distinct anchor; the dashboard (app/bot/router.py) lists this registry so server
admins can toggle each type, pick a channel, and choose a role to ping.

Each ``key`` is a permanent token (the GuildConfig.announcements map key); never
rename or reuse one.
"""
from __future__ import annotations

import time
from collections.abc import Awaitable, Callable
from dataclasses import dataclass

from app.core.utils import iso
from app.discord import embeds

# Embed builders may be sync or async; the announcer awaits as needed.
EmbedFn = Callable[[], "dict | Awaitable[dict]"]
AnchorFn = Callable[[], Awaitable["str | None"]]
ExpiryFn = Callable[[], Awaitable["int | None"]]


@dataclass(frozen=True)
class AnnouncementType:
    key: str            # permanent token; also the GuildConfig.announcements map key
    label: str          # dashboard row label
    description: str     # dashboard row hint
    category: str       # grouping label for the dashboard ("Rotations" / "Feeds")
    build_embed: EmbedFn
    current_anchor: AnchorFn
    # When True, posted messages are tracked (app/bot/models.TrackedAnnouncement):
    # superseded when the next one posts, and auto-deleted once irrelevant.
    auto_manage: bool = False
    # The current occurrence's end (unix), used as the auto-delete time. None means
    # "no natural end" - supersede-only (e.g. server status: replaced on each change).
    expiry: ExpiryFn | None = None


# Anchor fns import lazily (inside each fn) so importing this module stays cheap and
# free of cycles. Each returns the current anchor string, or None when there's
# nothing live to announce.

# Hourly challenges are split per category so each can post to its own channel and
# ping its own role. Categories actually offered as announcement options: Racing +
# Target are currently disabled in-game (never captured), so they're hidden from the
# dashboard to avoid dead toggles. Re-add them here if Trion turns them back on.
ANNOUNCED_CHALLENGE_CATEGORIES: tuple[str, ...] = ("collection", "rampage", "dungeon")


def _challenge_category_anchor(category: str) -> AnchorFn:
    """An anchor that fires only while the live hourly challenge is ``category``."""
    async def _anchor() -> str | None:
        from app.trove.captures import get_current_challenge
        cur = await get_current_challenge()
        if cur.get("active") and cur.get("name") and cur.get("type") == category:
            return str(cur["starts_at"])
        return None
    return _anchor


async def _chaos_anchor() -> str | None:
    from app.trove.chaos import get_chaos_chest
    c = await get_chaos_chest()
    if (c.get("item") or {}).get("name"):
        return str(c["starts_at"])
    return None


async def _daily_window_anchor() -> str | None:
    """The current daily-reset window start (next reset minus one day). Always
    present, so this fires once per daily reset (11:00 UTC)."""
    from app.trove.server_time import DAY, server_time
    return str(server_time()["daily_reset_at"] - DAY)


async def _longshade_anchor() -> str | None:
    from app.trove.rotations import biome_rotation
    cur = biome_rotation().get("current") or {}
    return str(cur["starts_at"]) if cur.get("starts_at") else None


async def _wild_mana_anchor() -> str | None:
    from app.trove.rotations import wild_mana
    cur = wild_mana().get("current") or {}
    return str(cur["starts_at"]) if cur.get("starts_at") else None


async def _stampy_anchor() -> str | None:
    from app.trove.rotations import stampy
    cur = stampy().get("current")
    now = int(time.time())
    if cur and cur["starts_at"] <= now < cur["ends_at"]:
        return str(cur["starts_at"])     # only while the event is live
    return None


async def _corruxion_anchor() -> str | None:
    from app.trove.server_time import corruxion
    c = corruxion()
    return str(c["starts_at"]) if c.get("active") else None


async def _fluxion_anchor() -> str | None:
    from app.trove.server_time import fluxion
    f = fluxion()
    return str(f["starts_at"]) if f.get("active") else None


async def _luxion_anchor() -> str | None:
    from app.trove.luxion import get_luxion
    l = await get_luxion()
    return str(l["starts_at"]) if l.get("active") else None


async def _news_anchor() -> str | None:
    from app.trove.news import latest_news
    items = await latest_news(1)
    if not items:
        return None
    top = items[0]
    url = getattr(top, "url", None)
    if url:
        return url
    return iso(getattr(top, "published_at", None))


async def _giveaway_anchor() -> str | None:
    """The newest currently-open giveaway, by creation time. Changes when a new
    giveaway opens; an older one closing doesn't re-announce."""
    from app.giveaways.models import Giveaway, GiveawayStatus
    newest = (
        await Giveaway.find(Giveaway.status == GiveawayStatus.open)
        .sort("-created_at").limit(1).to_list()
    )
    if not newest:
        return None
    return str(int(newest[0].created_at.timestamp()))


async def _activity_anchor() -> str | None:
    # A daily snapshot, anchored to the current daily window start.
    return await _daily_window_anchor()


async def _status_anchor() -> str | None:
    """Fires when Trove's overall server status changes. Reads the shared snapshot
    so the bot sees the API prober's status; skips "unknown" so we don't alert
    before the first probe lands."""
    from app.trove.status import get_status_shared
    overall = (await get_status_shared()).get("overall", "unknown")
    return None if overall == "unknown" else f"status:{overall}"


async def _game_update_anchor() -> str | None:
    """Fires when a new live (US) build is archived. Anchored to branch:ordinal so
    each new patch posts once; PTS builds don't move it."""
    from app.trove.updates import read as updates_read
    v = await updates_read.latest_version("live-us")
    return f"{v.branch}:{v.ordinal}" if v else None


# Expiry (auto-delete time): mirror the anchors but return the occurrence's END
# (unix). The announcer reads these only when it's posting (occurrence is current),
# so "current end" is right.

async def _challenge_expiry() -> int | None:
    from app.trove.captures import get_current_challenge
    return (await get_current_challenge()).get("ends_at")


async def _chaos_expiry() -> int | None:
    from app.trove.chaos import get_chaos_chest
    return (await get_chaos_chest()).get("ends_at")


async def _daily_expiry() -> int | None:
    from app.trove.server_time import server_time
    return server_time()["daily_reset_at"]            # the day's bonus is stale at the next reset


async def _longshade_expiry() -> int | None:
    from app.trove.rotations import biome_rotation
    return (biome_rotation().get("current") or {}).get("ends_at")


async def _wild_mana_expiry() -> int | None:
    from app.trove.rotations import wild_mana
    return (wild_mana().get("current") or {}).get("ends_at")


async def _stampy_expiry() -> int | None:
    from app.trove.rotations import stampy
    return (stampy().get("current") or {}).get("ends_at")


async def _corruxion_expiry() -> int | None:
    from app.trove.server_time import corruxion
    return corruxion().get("ends_at")


async def _fluxion_expiry() -> int | None:
    from app.trove.server_time import fluxion
    return fluxion().get("ends_at")


async def _luxion_expiry() -> int | None:
    from app.trove.luxion import get_luxion
    return (await get_luxion()).get("ends_at")


# One announcement type per challenge category (replaces the old single
# "hourly_challenge" type; migrate_legacy folds old configs into these). Each is
# an image announcement like the original; only the live category's anchor fires,
# so exactly one posts per window.
_CHALLENGE_LABELS: dict[str, tuple[str, str]] = {
    "collection": ("Hourly: Collection", "Post Collection hourly challenges only."),
    "rampage": ("Hourly: Rampage", "Post Rampage Alert hourly challenges only."),
    "racing": ("Hourly: Racing", "Post Racing hourly challenges only."),
    "target": ("Hourly: Target", "Post Target hourly challenges only."),
    "dungeon": ("Hourly: Dungeon", "Post Dungeon / biome hourly challenges only."),
}

_CHALLENGE_TYPES: tuple[AnnouncementType, ...] = tuple(
    AnnouncementType(
        f"challenge_{cat}", _CHALLENGE_LABELS[cat][0], _CHALLENGE_LABELS[cat][1],
        "Rotations", embeds.challenge_embed, _challenge_category_anchor(cat),
        auto_manage=True, expiry=_challenge_expiry)
    for cat in ANNOUNCED_CHALLENGE_CATEGORIES
)

ANNOUNCEMENT_TYPES: tuple[AnnouncementType, ...] = (
    *_CHALLENGE_TYPES,
    AnnouncementType(
        "chaos_chest", "Chaos Chest",
        "Post the week's featured Chaos Chest item at each Tuesday rotation.",
        "Rotations", embeds.chaos_embed, _chaos_anchor,
        auto_manage=True, expiry=_chaos_expiry),
    AnnouncementType(
        "daily_bonuses", "Daily bonuses",
        "Post the day's bonus at each daily reset (11:00 UTC).",
        "Rotations", embeds.bonuses_embed, _daily_window_anchor,
        auto_manage=True, expiry=_daily_expiry),
    AnnouncementType(
        "longshade", "Depth-15 biomes",
        "Post the Depth-15 delve biome rotation every 3 hours.",
        "Rotations", embeds.longshade_embed, _longshade_anchor,
        auto_manage=True, expiry=_longshade_expiry),
    AnnouncementType(
        "wild_mana", "Wild Mana",
        "Post the weekly Wild Mana biome rotation.",
        "Rotations", embeds.wild_mana_embed, _wild_mana_anchor,
        auto_manage=True, expiry=_wild_mana_expiry),
    AnnouncementType(
        "stampy", "Stampy event",
        "Post when the Stampy event begins each week.",
        "Rotations", embeds.stampy_embed, _stampy_anchor,
        auto_manage=True, expiry=_stampy_expiry),
    AnnouncementType(
        "corruxion", "Corruxion merchant",
        "Post when the Corruxion merchant arrives.",
        "Rotations", embeds.corruxion_embed, _corruxion_anchor,
        auto_manage=True, expiry=_corruxion_expiry),
    AnnouncementType(
        "fluxion", "Fluxion merchant",
        "Post when the Fluxion merchant arrives (voting / selling stage).",
        "Rotations", embeds.fluxion_embed, _fluxion_anchor,
        auto_manage=True, expiry=_fluxion_expiry),
    AnnouncementType(
        "luxion", "Luxion merchant",
        "Post when Luxion arrives for its visit.",
        "Rotations", embeds.luxion_embed, _luxion_anchor,
        auto_manage=True, expiry=_luxion_expiry),
    AnnouncementType(
        "trove_news", "Trove news",
        "Post each new Trove news article from the official feed.",
        "Feeds", embeds.trove_news_embed, _news_anchor),
    AnnouncementType(
        "giveaways", "Giveaways",
        "Post when a new giveaway opens on trove.aallyn.net.",
        "Feeds", embeds.giveaways_embed, _giveaway_anchor),
    AnnouncementType(
        "activity", "Player activity",
        "Post a daily player-activity snapshot at the daily reset.",
        "Feeds", embeds.activity_embed, _activity_anchor),
    AnnouncementType(
        "server_status", "Server status",
        "Post when Trove's overall server status changes (online / down).",
        "Feeds", embeds.status_embed, _status_anchor,
        auto_manage=True, expiry=None),    # supersede-only: replaced on the next change
    AnnouncementType(
        "game_update", "Game updates",
        "Post when a new Trove build goes live (file-change summary image + link).",
        "Feeds", embeds.game_update_embed, _game_update_anchor),    # rich embed; stays as a record
    )

TYPES_BY_KEY: dict[str, AnnouncementType] = {t.key: t for t in ANNOUNCEMENT_TYPES}


def catalog() -> list[dict]:
    """Registry metadata for the dashboard: [{key, label, description, category}]."""
    return [
        {"key": t.key, "label": t.label, "description": t.description, "category": t.category}
        for t in ANNOUNCEMENT_TYPES
    ]
