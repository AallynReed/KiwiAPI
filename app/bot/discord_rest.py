"""Discord REST helper for the dashboard side (the API process), via the bot token.

The gateway bot (``runner.py``) does the proactive sends; THIS module is the API's
read/preflight path: enumerate the bot's guilds, check whether a dashboard user
manages a guild, list a guild's text channels, and compute the bot's effective
channel permissions so the dashboard can warn about what's missing.

Pure httpx (no discord.py) so the API process never loads the gateway library.
The permission math is the standard Discord algorithm and is kept pure + testable.
"""
from __future__ import annotations

import logging
from urllib.parse import quote

import httpx

from app.core.config import settings

logger = logging.getLogger("kiwi.bot.rest")

_API = "https://discord.com/api/v10"

# Permission bits - https://discord.com/developers/docs/topics/permissions
PERM_ADMINISTRATOR = 1 << 3
PERM_MANAGE_GUILD = 1 << 5
PERM_VIEW_CHANNEL = 1 << 10
PERM_SEND_MESSAGES = 1 << 11
PERM_EMBED_LINKS = 1 << 14

_ALL = ~0                       # every bit set (owner / administrator shortcut)
_TEXT_CHANNEL_TYPES = {0, 5}    # GUILD_TEXT, GUILD_ANNOUNCEMENT

# Permissions the announcer needs, in the order we surface them to the dashboard.
ANNOUNCE_PERMS = (
    ("View Channel", PERM_VIEW_CHANNEL),
    ("Send Messages", PERM_SEND_MESSAGES),
    ("Embed Links", PERM_EMBED_LINKS),
)


class DiscordRestError(Exception):
    """Bot token unset, Discord unreachable, or a non-2xx response."""


def _headers() -> dict[str, str]:
    return {"Authorization": f"Bot {settings.discord_bot_token}"}


async def _get(client: httpx.AsyncClient, path: str):
    if not settings.discord_bot_token:
        raise DiscordRestError("DISCORD_BOT_TOKEN is not configured.")
    try:
        resp = await client.get(f"{_API}{path}", headers=_headers())
    except httpx.HTTPError as exc:
        raise DiscordRestError(f"Could not reach Discord: {exc}") from exc
    if resp.status_code == 404:
        return None
    if resp.status_code >= 300:
        raise DiscordRestError(f"GET {path} -> {resp.status_code}: {resp.text[:200]}")
    return resp.json()


async def _send(client: httpx.AsyncClient, method: str, path: str, *, reason: str | None = None):
    """A body-less write (PUT/DELETE) against the Discord API, with an optional
    audit-log reason. Raises ``DiscordRestError`` on any non-2xx (the message
    keeps the status so callers can branch on ``"403"`` for missing-permission)."""
    if not settings.discord_bot_token:
        raise DiscordRestError("DISCORD_BOT_TOKEN is not configured.")
    headers = _headers()
    if reason:
        headers["X-Audit-Log-Reason"] = quote(reason[:400], safe="")   # URL-encoded per Discord
    try:
        resp = await client.request(method, f"{_API}{path}", headers=headers)
    except httpx.HTTPError as exc:
        raise DiscordRestError(f"Could not reach Discord: {exc}") from exc
    if resp.status_code >= 300:
        raise DiscordRestError(f"{method} {path} -> {resp.status_code}: {resp.text[:200]}")
    return resp


# The bot's own user id, for fetching its guild member. Discord's member endpoint
# wants a real snowflake (there is no ``/members/@me``), so we resolve + cache the
# id once - it's constant for a given token.
_bot_user_id: int | None = None


async def _get_bot_user_id(client: httpx.AsyncClient) -> int | None:
    global _bot_user_id
    if _bot_user_id is None:
        me = await _get(client, "/users/@me")
        if me and me.get("id"):
            _bot_user_id = int(me["id"])
    return _bot_user_id


# ── pure permission math (testable) ─────────────────────────────────────────

def guild_permissions(guild: dict, member: dict) -> int:
    """A member's guild-level (base) permissions: @everyone OR each role, with the
    ADMINISTRATOR shortcut. ``guild`` carries owner_id + roles."""
    if int(member["user"]["id"]) == int(guild["owner_id"]):
        return _ALL
    roles_by_id = {int(r["id"]): int(r["permissions"]) for r in guild.get("roles", [])}
    perms = roles_by_id.get(int(guild["id"]), 0)            # @everyone
    for rid in member.get("roles", []):
        perms |= roles_by_id.get(int(rid), 0)
    return _ALL if perms & PERM_ADMINISTRATOR else perms


def effective_channel_permissions(guild: dict, member: dict, channel: dict) -> int:
    """The member's effective permissions in ``channel`` - base perms then the
    @everyone / role / member channel overwrites (standard Discord order)."""
    base = guild_permissions(guild, member)
    if base == _ALL:
        return _ALL

    guild_id = int(guild["id"])
    member_id = int(member["user"]["id"])
    member_role_ids = [int(r) for r in member.get("roles", [])]
    overwrites = {int(o["id"]): o for o in channel.get("permission_overwrites", [])}

    perms = base
    ow = overwrites.get(guild_id)                          # @everyone overwrite
    if ow:
        perms = (perms & ~int(ow["deny"])) | int(ow["allow"])
    allow = deny = 0
    for rid in member_role_ids:                            # aggregate role overwrites
        ow = overwrites.get(rid)
        if ow:
            allow |= int(ow["allow"])
            deny |= int(ow["deny"])
    perms = (perms & ~deny) | allow
    ow = overwrites.get(member_id)                         # member-specific overwrite
    if ow:
        perms = (perms & ~int(ow["deny"])) | int(ow["allow"])
    return perms


# ── REST calls ──────────────────────────────────────────────────────────────

async def bot_guilds() -> list[dict]:
    """Partial guild objects (id, name, icon) for guilds the bot is a member of."""
    async with httpx.AsyncClient(timeout=15) as client:
        data = await _get(client, "/users/@me/guilds") or []
    return [{"id": str(g["id"]), "name": g.get("name", ""), "icon": g.get("icon")} for g in data]


async def user_can_manage(guild_id: int, user_discord_id: int) -> bool:
    """True if the user is the guild owner or has Administrator / Manage Server."""
    async with httpx.AsyncClient(timeout=15) as client:
        guild = await _get(client, f"/guilds/{guild_id}")
        member = await _get(client, f"/guilds/{guild_id}/members/{user_discord_id}")
    if not guild or not member:
        return False
    perms = guild_permissions(guild, member)
    return bool(perms & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD))


def text_channels(channels: list[dict]) -> list[dict]:
    """Pure: text/announcement channels from a raw channel list, by position."""
    out = [
        {"id": str(ch["id"]), "name": ch.get("name", ""), "position": ch.get("position", 0)}
        for ch in (channels or []) if ch.get("type") in _TEXT_CHANNEL_TYPES
    ]
    out.sort(key=lambda c: c["position"])
    return out


def assignable_roles(roles: list[dict], guild_id: int) -> list[dict]:
    """Pure: human-assignable roles (excludes @everyone and bot/integration-managed
    roles), highest position first - the picker the owner delegates / pings from."""
    out = [
        {"id": str(r["id"]), "name": r["name"], "color": r.get("color", 0), "position": r.get("position", 0)}
        for r in (roles or [])
        if str(r["id"]) != str(guild_id) and not r.get("managed")
    ]
    out.sort(key=lambda r: r["position"], reverse=True)
    return out


async def guild_text_channels(guild_id: int) -> list[dict]:
    """Text/announcement channels in a guild, ordered by position."""
    async with httpx.AsyncClient(timeout=15) as client:
        channels = await _get(client, f"/guilds/{guild_id}/channels") or []
    return text_channels(channels)


async def guild_member_context(guild_id: int, user_discord_id: int) -> dict | None:
    """``{is_owner, is_admin, role_ids}`` for a user in a guild, or None if they
    aren't a member (or the bot can't see the guild). One fetch of guild+member,
    reused for both the manage check and the role-capability check."""
    async with httpx.AsyncClient(timeout=15) as client:
        guild = await _get(client, f"/guilds/{guild_id}")
        member = await _get(client, f"/guilds/{guild_id}/members/{user_discord_id}")
    if not guild or not member:
        return None
    perms = guild_permissions(guild, member)
    return {
        "is_owner": int(guild["owner_id"]) == int(user_discord_id),
        "is_admin": bool(perms & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD)),
        "role_ids": {int(r) for r in member.get("roles", [])},
    }


async def guild_assignable_roles(guild_id: int) -> list[dict]:
    """Human-assignable roles (excludes @everyone and bot/integration-managed
    roles), highest position first - the picker the owner delegates from."""
    async with httpx.AsyncClient(timeout=15) as client:
        roles = await _get(client, f"/guilds/{guild_id}/roles") or []
    return assignable_roles(roles, guild_id)


async def guild_members(guild_id: int) -> list[dict]:
    """Every guild member (paginated, 1000/page, ascending by id). REQUIRES the
    GUILD_MEMBERS privileged intent enabled for the app - the List Guild Members
    endpoint returns 403 (-> DiscordRestError) otherwise."""
    out: list[dict] = []
    after = "0"
    async with httpx.AsyncClient(timeout=20) as client:
        for _ in range(50):                       # safety cap (~50k members)
            batch = await _get(client, f"/guilds/{guild_id}/members?limit=1000&after={after}")
            if not batch:
                break
            out.extend(batch)
            if len(batch) < 1000:
                break
            after = str(max(int(m["user"]["id"]) for m in batch))
    return out


async def guild_member(guild_id: int, user_id: int) -> dict | None:
    """One guild member object (with their current ``roles``), or None if they
    aren't in the guild. Unlike ``guild_members`` this needs no privileged intent."""
    async with httpx.AsyncClient(timeout=15) as client:
        return await _get(client, f"/guilds/{guild_id}/members/{user_id}")


async def add_member_role(guild_id: int, user_id: int, role_id: int, *, reason: str | None = None) -> None:
    """Give a member a role. Needs the bot's **Manage Roles** permission and its own
    top role above ``role_id`` - Discord answers 403 otherwise (-> DiscordRestError)."""
    async with httpx.AsyncClient(timeout=15) as client:
        await _send(client, "PUT", f"/guilds/{guild_id}/members/{user_id}/roles/{role_id}", reason=reason)


async def remove_member_role(guild_id: int, user_id: int, role_id: int, *, reason: str | None = None) -> None:
    """Take a role off a member (same permission requirements as ``add_member_role``)."""
    async with httpx.AsyncClient(timeout=15) as client:
        await _send(client, "DELETE", f"/guilds/{guild_id}/members/{user_id}/roles/{role_id}", reason=reason)


async def guild_snapshot(guild_id: int) -> dict | None:
    """One fetch of guild + the bot's own member + all channels + all roles, for
    the dashboard's per-guild view (preflight for many channels, reconcile, and
    the channel/role pickers - without N round-trips). None if the bot can't see
    the guild."""
    async with httpx.AsyncClient(timeout=15) as client:
        bot_id = await _get_bot_user_id(client)
        guild = await _get(client, f"/guilds/{guild_id}")
        me = await _get(client, f"/guilds/{guild_id}/members/{bot_id}") if bot_id else None
        channels = await _get(client, f"/guilds/{guild_id}/channels")
        roles = await _get(client, f"/guilds/{guild_id}/roles")
    if not guild or not me or channels is None:
        return None
    return {"guild": guild, "me": me, "channels": channels, "roles": roles or []}


def preflight_for(guild: dict, me: dict, channels: list[dict], channel_id) -> dict:
    """Pure: which announce permissions the bot is MISSING in ``channel_id``,
    computed from an already-fetched snapshot. ``{ok, missing:[names], error?}``."""
    channel = next((ch for ch in (channels or []) if str(ch.get("id")) == str(channel_id)), None)
    if channel is None:
        return {"ok": False, "missing": [], "error": "That channel no longer exists."}
    perms = effective_channel_permissions(guild, me, channel)
    missing = [name for name, bit in ANNOUNCE_PERMS if not (perms & bit)]
    return {"ok": not missing, "missing": missing}


async def channel_preflight(guild_id: int, channel_id: int) -> dict:
    """Which announce permissions the bot is MISSING in a channel (standalone
    fetch). Returns ``{ok, missing:[names], error?}``."""
    snap = await guild_snapshot(guild_id)
    if snap is None:
        return {"ok": False, "missing": [], "error": "The bot isn't in this server (or Discord is unreachable)."}
    return preflight_for(snap["guild"], snap["me"], snap["channels"], channel_id)
