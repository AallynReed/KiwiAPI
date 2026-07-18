"""Dashboard API for the Discord bot (User Dashboard "Discord Bot" tab).

Authenticated as the Dashboard (site) user - the ``SiteUser`` from "Sign in with
Discord" on trove.aallyn.net, NOT the dev portal. Uses the user's linked
``discord_id`` + the bot token (REST) to find the servers they can configure, pick
announcement channels + a role to ping, warn about missing bot permissions, and
(owners/admins) delegate config capabilities to real Discord roles. Config + the
role mapping live in ``GuildConfig``.

Permission model:
- **Admins** (server owner / Administrator / Manage-Server) can do everything AND
  edit the role->capability mapping.
- **Delegated** users hold a Discord role the owner mapped to a capability:
  - ``manage_announcements`` - toggle announcement types + set channels.
  - ``manage_ping_roles``    - choose which role each announcement pings (a real
    security boundary: pinging can mass-notify members).
- Mappings + every channel/role id are validated against LIVE state on every load
  (``reconcile``): a deleted role is removed from grants + pings, and a deleted
  channel disables its announcement and is flagged loudly for the user.
"""
import asyncio
import logging

from beanie import PydanticObjectId
from beanie.operators import In
from fastapi import APIRouter, Depends
from pydantic import BaseModel

from app import i18n
from app.bot import discord_rest
from app.bot.announcements import ANNOUNCEMENT_TYPES, TYPES_BY_KEY
from app.bot.discord_rest import (
    PERM_ADMINISTRATOR,
    PERM_MANAGE_GUILD,
    DiscordRestError,
)
from app.bot.models import (
    CLUB_RANK_LABELS,
    CLUB_RANKS,
    MAX_CLUBS_PER_GUILD,
    AnnouncementSetting,
    Club,
    GuildConfig,
    MarketWatchItem,
    demote_rank,
    promote_rank,
)
from app.bot.reconcile import reconcile, reconcile_club
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.discord import embed_contexts
from app.embed_templates import EmbedTemplate
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser
from app.site_auth.oauth import clear_discord_token, fetch_discord_guilds, get_discord_token

logger = logging.getLogger("kiwi.bot.dashboard")

router = APIRouter(prefix="/v1/site-auth/discord", tags=["discord-bot"])

# Config capabilities the owner can delegate to Discord roles. Extensible: add a
# key here and gate the relevant action with `_require_capability`.
CAPABILITIES: dict[str, str] = {
    "manage_announcements": "Configure announcements (enable types + set channels)",
    "manage_ping_roles": "Choose which roles announcements ping",
    "manage_clubs": "Create and edit the server's Trove clubs",
}

# Permissions requested in the bot invite - the exact bitfield configured for the
# Kiwi app (View/Send/Embed + Read History, Add Reactions, Attach Files, Mention
# Everyone, External Emojis, Create Invite, Manage Roles/Webhooks/Nicknames,
# Events, Polls). The announce-channel preflight below still only needs
# View/Send/Embed to actually post.
_INVITE_PERMS = 5084151364439105


def _invite_url() -> str | None:
    if not settings.discord_client_id:
        return None
    return (
        f"https://discord.com/oauth2/authorize?client_id={settings.discord_client_id}"
        f"&scope=bot&permissions={_INVITE_PERMS}"
    )


def _reconnect_url() -> str | None:
    """Where to send a user to (re)authorize Discord with the `guilds` scope - the
    site "Sign in with Discord", which always requests `guilds`."""
    if not settings.discord_oauth_enabled:
        return None
    return f"{settings.api_url}/v1/site-auth/oauth/discord/start"


def _perms_int(value) -> int:
    try:
        return int(value)
    except (TypeError, ValueError):
        return 0


async def _discord(awaitable):
    """Await a ``discord_rest`` call, mapping a transport failure to a 502."""
    try:
        return await awaitable
    except DiscordRestError as exc:
        raise APIError(502, ErrorCode.internal_error, f"Couldn't reach Discord: {exc}")


async def _load_or_new_config(guild_id: int) -> tuple[GuildConfig | None, GuildConfig]:
    """Load the guild's config (or build a fresh one) and fold in legacy fields.

    Returns ``(existing, cfg)``: ``existing`` is the loaded doc or ``None`` (the
    caller inspects it for the permission check and to pick save vs insert);
    ``cfg`` is the working doc, always migrated."""
    existing = await GuildConfig.find_one(GuildConfig.guild_id == guild_id)
    cfg = existing or GuildConfig(guild_id=guild_id)
    cfg.migrate_legacy()
    return existing, cfg


async def _save_config(existing: GuildConfig | None, cfg: GuildConfig, user: SiteUser) -> None:
    """Stamp who/when and persist (``save`` when it already existed, else ``insert``)."""
    cfg.updated_by = user.discord_id
    cfg.updated_at = utcnow()
    if existing:
        await cfg.save()
    else:
        await cfg.insert()


def _valid_id(value: str | None, valid: set[int]) -> int | None:
    """A single snowflake string, kept only if it names a currently-live id."""
    if value and str(value).isdigit() and int(value) in valid:
        return int(value)
    return None


def _valid_ids(values, valid: set[int]) -> list[int]:
    """Snowflake strings filtered to currently-live ids (order preserved, no dedupe)."""
    return [int(r) for r in values if str(r).isdigit() and int(r) in valid]


def _live_ids(snap: dict, guild_id: int) -> tuple[set[int], set[int]]:
    """Live ``(text-channel ids, assignable-role ids)`` from an already-fetched snapshot."""
    channel_ids = {int(c["id"]) for c in discord_rest.text_channels(snap["channels"])}
    role_ids = {int(r["id"]) for r in discord_rest.assignable_roles(snap["roles"], guild_id)}
    return channel_ids, role_ids


class AnnouncementUpdate(BaseModel):
    enabled: bool = False
    channel_id: str | None = None            # snowflake string, or null to clear
    ping_role_ids: list[str] = []            # snowflake strings; roles to @-mention


class AnnouncementsUpdate(BaseModel):
    announcements: dict[str, AnnouncementUpdate]   # registry key -> settings


class AnnouncementTemplateUpdate(BaseModel):
    template: EmbedTemplate | None = None          # null clears the custom embed


class LiveBoardUpdate(BaseModel):
    enabled: bool = False
    channel_id: str | None = None            # snowflake string, or null to clear


class MarketWatchItemUpdate(BaseModel):
    name: str
    max_price_each: float | None = None      # per-unit flux ceiling; null = any listing


class MarketWatchUpdate(BaseModel):
    enabled: bool = False
    channel_id: str | None = None            # snowflake string, or null to clear
    ping_role_ids: list[str] = []            # snowflake strings; roles to @-mention
    items: list[MarketWatchItemUpdate] = []


class ClubUpdate(BaseModel):
    name: str
    public: bool = False
    description: str | None = None
    banner_url: str | None = None
    avatar_url: str | None = None
    discord_url: str | None = None
    website_url: str | None = None
    role_links: dict[str, str] = {}          # in-game rank -> discord role id string


class PermissionsUpdate(BaseModel):
    permissions: dict[str, list[str]]        # capability -> [role id strings]


class LanguageUpdate(BaseModel):
    language: str                            # a code from app.i18n.SUPPORTED


async def _member_ctx(guild_id: int, user: SiteUser) -> dict:
    """The user's {is_owner, is_admin, role_ids} in the guild, or a 403/502."""
    if not user.discord_id:
        raise APIError(403, ErrorCode.forbidden,
                       "Link your Discord account (sign in with Discord) to manage the bot.")
    ctx = await _discord(discord_rest.guild_member_context(guild_id, user.discord_id))
    if ctx is None:
        raise APIError(403, ErrorCode.forbidden,
                       "You're not a member of this server (or the bot isn't in it).")
    return ctx


def _has_capability(ctx: dict, config_perms: dict, capability: str) -> bool:
    """Admins have every capability; otherwise the user must hold a LIVE role the
    owner mapped to it (deleted roles aren't in role_ids, so they don't count)."""
    if ctx["is_admin"]:
        return True
    allowed = {int(r) for r in config_perms.get(capability, [])}
    return bool(ctx["role_ids"] & allowed)


def _require_admin(ctx: dict) -> None:
    if not ctx["is_admin"]:
        raise APIError(403, ErrorCode.forbidden,
                       "Only the server owner or Manage-Server admins can change who can configure the bot.")


async def _snapshot(guild_id: int) -> dict:
    """One batched fetch of the guild's live channels/roles/permissions, or a 502/403."""
    snap = await _discord(discord_rest.guild_snapshot(guild_id))
    if snap is None:
        raise APIError(403, ErrorCode.forbidden,
                       "The bot isn't in this server (or Discord is unreachable).")
    return snap


def _detail_payload(guild_id: int, ctx: dict, cfg: GuildConfig | None, snap: dict) -> dict:
    """Assemble the per-guild detail response from an already-fetched snapshot
    (no network). Lists every announcement type with its current config +
    per-channel preflight, the channel/role pickers, and (admins) the
    capability->role mapping editor."""
    guild, me = snap["guild"], snap["me"]
    raw_channels, raw_roles = snap["channels"], snap["roles"]
    config_perms = cfg.config_perms if cfg else {}
    settings_map = cfg.announcements if cfg else {}

    can_manage = _has_capability(ctx, config_perms, "manage_announcements")
    can_ping = _has_capability(ctx, config_perms, "manage_ping_roles")
    can_clubs = _has_capability(ctx, config_perms, "manage_clubs")

    announcements = []
    for atype in ANNOUNCEMENT_TYPES:
        s = settings_map.get(atype.key)
        channel_id = s.channel_id if s else None
        preflight = (discord_rest.preflight_for(guild, me, raw_channels, channel_id)
                     if s and s.enabled and channel_id else None)
        announcements.append({
            "key": atype.key,
            "label": atype.label,
            "description": atype.description,
            "category": atype.category,
            "enabled": bool(s and s.enabled),
            "channel_id": str(channel_id) if channel_id else None,
            "ping_role_ids": [str(r) for r in s.ping_role_ids] if s else [],
            "channel_missing": bool(s and s.channel_missing),
            "customizable": embed_contexts.has_customization(atype.key),
            "template": s.template.model_dump() if (s and s.template) else None,
            "preflight": preflight,
        })

    board = cfg.live_board if cfg else None
    board_channel = board.channel_id if board else None
    board_preflight = (discord_rest.preflight_for(guild, me, raw_channels, board_channel)
                       if board and board.enabled and board_channel else None)
    mw = cfg.market_watch if cfg else None

    result = {
        "guild_id": str(guild_id),
        "is_admin": ctx["is_admin"],
        "can_manage_announcements": can_manage,
        "can_manage_ping_roles": can_ping,
        "can_manage_clubs": can_clubs,
        "language": (cfg.language if cfg and cfg.language in i18n.SUPPORTED else "en"),
        "languages": [{"code": c, "label": label} for c, label in i18n.LANGS],
        "channels": discord_rest.text_channels(raw_channels),
        "roles": discord_rest.assignable_roles(raw_roles, guild_id),
        "announcements": announcements,
        "announcing": any(a["enabled"] for a in announcements),
        "live_board": {
            "enabled": bool(board and board.enabled),
            "channel_id": str(board_channel) if board_channel else None,
            "channel_missing": bool(board and board.channel_missing),
            "preflight": board_preflight,
        },
        "market_watch": {
            "enabled": bool(mw and mw.enabled),
            "channel_id": str(mw.channel_id) if mw and mw.channel_id else None,
            "ping_role_ids": [str(r) for r in (mw.ping_role_ids if mw else [])],
            "channel_missing": bool(mw and mw.channel_missing),
            "items": [{"name": it.name, "max_price_each": it.max_price_each}
                      for it in (mw.items if mw else [])],
        },
    }
    if ctx["is_admin"]:
        result["capabilities"] = [{"key": k, "label": v} for k, v in CAPABILITIES.items()]
        result["permissions"] = {k: [str(r) for r in config_perms.get(k, [])] for k in CAPABILITIES}
    return result


@router.get("/guilds")
async def list_my_guilds(user: SiteUser = Depends(get_current_site_user)) -> dict:
    """The user's servers, fetched LIVE from Discord on demand (the guild list is
    never stored at rest): configurable ones (bot present + they're admin/delegated)
    plus owner/admin servers the bot isn't in yet (to invite). Reprompts a Discord
    reconnect if we don't have a valid cached token (expired/revoked/never synced)."""
    invite = _invite_url()
    reconnect = _reconnect_url()
    token = await get_discord_token(user.id) if user.discord_id else None
    user_guilds = await fetch_discord_guilds(token) if token else None
    if user_guilds is None:
        if token:                      # token present but rejected -> drop the stale key
            await clear_discord_token(user.id)
        return {
            "linked": bool(user.discord_id),
            "guilds_synced": False,
            "invite_url": invite,
            "reconnect_url": reconnect,
            "guilds": [],
        }

    try:
        bot_ids = {int(g["id"]) for g in await discord_rest.bot_guilds()}
    except DiscordRestError as exc:
        return {"linked": True, "guilds_synced": True, "invite_url": invite,
                "reconnect_url": reconnect, "guilds": [], "error": str(exc)}
    present = [g for g in user_guilds if int(g["id"]) in bot_ids]
    present_ids = [int(g["id"]) for g in present]
    ctxs = await asyncio.gather(
        *[discord_rest.guild_member_context(gid, user.discord_id) for gid in present_ids],
        return_exceptions=True,
    )
    ctx_by_id = {gid: (c if isinstance(c, dict) else None)
                 for gid, c in zip(present_ids, ctxs, strict=True)}
    configs: dict[int, GuildConfig] = {}
    if present_ids:
        configs = {c.guild_id: c for c in await GuildConfig.find(In(GuildConfig.guild_id, present_ids)).to_list()}

    guilds = []
    for g in user_guilds:
        gid = int(g["id"])
        oauth_admin = bool(g.get("owner")) or bool(
            _perms_int(g.get("permissions")) & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD)
        )
        if gid in bot_ids:
            ctx = ctx_by_id.get(gid)
            if ctx is None:                      # member fetch failed / not really a member
                continue
            cfg = configs.get(gid)
            if cfg and cfg.migrate_legacy():
                await cfg.save()
            config_perms = cfg.config_perms if cfg else {}
            accessible = ctx["is_admin"] or any(
                ctx["role_ids"] & {int(r) for r in rids} for rids in config_perms.values()
            )
            if not accessible:
                continue
            announcing = bool(cfg and any(
                s.enabled and not s.channel_missing for s in cfg.announcements.values()))
            guilds.append({
                "id": str(gid), "name": g["name"], "icon": g.get("icon"),
                "bot_present": True, "is_admin": ctx["is_admin"],
                "announcing": announcing,
            })
        elif oauth_admin:
            # Owner/admin server the bot isn't in yet -> offer to invite it (the
            # guild is pre-selected in Discord's invite dialog).
            guilds.append({
                "id": str(gid), "name": g["name"], "icon": g.get("icon"),
                "bot_present": False, "is_admin": True,
                "invite_url": (invite + f"&guild_id={gid}") if invite else None,
            })
    guilds.sort(key=lambda g: (not g["bot_present"], g["name"].lower()))  # configurable first
    return {"linked": True, "guilds_synced": True, "invite_url": invite,
            "reconnect_url": reconnect, "guilds": guilds}


@router.get("/guilds/{guild_id}")
async def guild_detail(guild_id: int, user: SiteUser = Depends(get_current_site_user)) -> dict:
    """A server's announcement config + channel/role pickers + permission
    preflight. Reconciles against live Discord state on every load (drops deleted
    roles, flags deleted channels). Admins additionally get the role->capability
    mapping editor."""
    ctx = await _member_ctx(guild_id, user)
    cfg = await GuildConfig.find_one(GuildConfig.guild_id == guild_id)
    config_perms = cfg.config_perms if cfg else {}
    if not (_has_capability(ctx, config_perms, "manage_announcements")
            or _has_capability(ctx, config_perms, "manage_ping_roles")):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to configure the bot in this server.")

    snap = await _snapshot(guild_id)
    if cfg:
        dirty = cfg.migrate_legacy()
        dirty = reconcile(cfg,
                          {int(c["id"]) for c in snap["channels"]},
                          {int(r["id"]) for r in snap["roles"]}) or dirty
        if dirty:
            cfg.updated_at = utcnow()
            await cfg.save()
    return _detail_payload(guild_id, ctx, cfg, snap)


@router.put("/guilds/{guild_id}/announcements")
async def set_announcements(
    guild_id: int, body: AnnouncementsUpdate, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Update announcement types (enable/disable + channel + ping role). Changing a
    type's enabled state or channel needs ``manage_announcements``; changing which
    role it pings needs ``manage_ping_roles``. Returns the freshly-assembled detail."""
    ctx = await _member_ctx(guild_id, user)
    existing, cfg = await _load_or_new_config(guild_id)
    config_perms = existing.config_perms if existing else {}
    can_manage = _has_capability(ctx, config_perms, "manage_announcements")
    can_ping = _has_capability(ctx, config_perms, "manage_ping_roles")
    if not (can_manage or can_ping):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to configure the bot in this server.")

    snap = await _snapshot(guild_id)
    live_channel_ids, live_role_ids = _live_ids(snap, guild_id)

    for key, upd in body.announcements.items():
        atype = TYPES_BY_KEY.get(key)
        if atype is None:
            continue                              # ignore unknown / retired types
        cur = cfg.announcements.get(key) or AnnouncementSetting()
        new_channel = _valid_id(upd.channel_id, live_channel_ids)
        new_pings = sorted(set(_valid_ids(upd.ping_role_ids, live_role_ids)))

        # Permission split: who-gets-pinged is gated separately from channels/toggles.
        if (upd.enabled != cur.enabled or new_channel != cur.channel_id) and not can_manage:
            raise APIError(403, ErrorCode.forbidden,
                           "You can't change announcement channels or toggles in this server.")
        if new_pings != sorted(cur.ping_role_ids) and not can_ping:
            raise APIError(403, ErrorCode.forbidden,
                           "You can't change which roles get pinged in this server.")

        if upd.enabled and not new_channel:
            raise APIError(400, ErrorCode.bad_request,
                           f"Pick a channel before enabling {atype.label}.")

        if can_manage:
            was_enabled, old_channel = cur.enabled, cur.channel_id
            cur.enabled = upd.enabled
            cur.channel_id = new_channel
            cur.channel_missing = False           # channel was re-validated against live state
            # Seed the anchor on (re)enable / channel switch so the FIRST post is
            # the next event, not the in-progress one.
            if cur.enabled and new_channel and (not was_enabled or old_channel != new_channel):
                try:
                    cur.last_anchor = await atype.current_anchor()
                except Exception:
                    cur.last_anchor = None
        if can_ping:
            cur.ping_role_ids = new_pings
        cfg.announcements[key] = cur

    await _save_config(existing, cfg, user)
    return _detail_payload(guild_id, ctx, cfg, snap)


@router.get("/announcement-meta")
async def announcement_meta(user: SiteUser = Depends(get_current_site_user)) -> dict:
    """Per-type embed-editor metadata (variables, default template, sample, image
    support) for every customizable announcement type. Fetched once by the dashboard."""
    out = []
    for atype in ANNOUNCEMENT_TYPES:
        if not embed_contexts.has_customization(atype.key):
            continue
        out.append({
            "key": atype.key,
            "variables": embed_contexts.variables(atype.key),
            "default_template": embed_contexts.default_template(atype.key).model_dump(),
            "sample": embed_contexts.sample_context(atype.key),
            "has_image": embed_contexts.has_image(atype.key),
        })
    return {"meta": out}


@router.put("/guilds/{guild_id}/announcements/{key}/template")
async def set_announcement_template(
    guild_id: int, key: str, body: AnnouncementTemplateUpdate,
    user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Set (or clear, with ``template: null``) the custom embed for one announcement
    type. Needs ``manage_announcements``. The default embed is used whenever there's no
    template or it's disabled."""
    atype = TYPES_BY_KEY.get(key)
    if atype is None or not embed_contexts.has_customization(key):
        raise APIError(404, ErrorCode.not_found, "Unknown announcement type.")
    ctx = await _member_ctx(guild_id, user)
    existing, cfg = await _load_or_new_config(guild_id)
    if not _has_capability(ctx, existing.config_perms if existing else {}, "manage_announcements"):
        raise APIError(403, ErrorCode.forbidden,
                       "You can't configure announcements in this server.")
    cur = cfg.announcements.get(key) or AnnouncementSetting()
    cur.template = body.template
    cfg.announcements[key] = cur
    await _save_config(existing, cfg, user)
    snap = await _snapshot(guild_id)
    return _detail_payload(guild_id, ctx, cfg, snap)


@router.put("/guilds/{guild_id}/language")
async def set_language(
    guild_id: int, body: LanguageUpdate, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Set the language the bot speaks in this server (announcements, the live
    board, and slash replies invoked here). Needs ``manage_announcements`` (or
    admin). Returns the freshly-assembled detail."""
    ctx = await _member_ctx(guild_id, user)
    existing, cfg = await _load_or_new_config(guild_id)
    if not _has_capability(ctx, existing.config_perms if existing else {}, "manage_announcements"):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to configure the bot in this server.")
    if body.language not in i18n.SUPPORTED:
        raise APIError(400, ErrorCode.bad_request, "Unsupported language.")

    cfg.language = body.language
    await _save_config(existing, cfg, user)
    snap = await _snapshot(guild_id)
    return _detail_payload(guild_id, ctx, cfg, snap)


@router.put("/guilds/{guild_id}/live-board")
async def set_live_board(
    guild_id: int, body: LiveBoardUpdate, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Enable/disable the self-updating "Trove Now" board + set its channel.
    Requires ``manage_announcements``. Switching channel re-posts a fresh board."""
    ctx = await _member_ctx(guild_id, user)
    existing, cfg = await _load_or_new_config(guild_id)
    if not _has_capability(ctx, existing.config_perms if existing else {}, "manage_announcements"):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to configure the bot in this server.")

    snap = await _snapshot(guild_id)
    live_channel_ids, _ = _live_ids(snap, guild_id)
    channel_id = _valid_id(body.channel_id, live_channel_ids)
    if body.enabled and not channel_id:
        raise APIError(400, ErrorCode.bad_request, "Pick a channel before enabling the live board.")

    board = cfg.live_board
    if channel_id != board.channel_id:
        board.message_id = None              # post a fresh board in the new channel
    board.enabled = body.enabled
    board.channel_id = channel_id
    board.channel_missing = False
    await _save_config(existing, cfg, user)
    return _detail_payload(guild_id, ctx, cfg, snap)


@router.put("/guilds/{guild_id}/market-watch")
async def set_market_watch(
    guild_id: int, body: MarketWatchUpdate, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Enable/disable the marketplace watch + set its channel, ping roles, and the
    watchlist (each item with an optional per-unit flux ceiling). Requires
    ``manage_announcements``; the ping roles also need ``manage_ping_roles``."""
    ctx = await _member_ctx(guild_id, user)
    existing, cfg = await _load_or_new_config(guild_id)
    config_perms = existing.config_perms if existing else {}
    if not _has_capability(ctx, config_perms, "manage_announcements"):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to configure the bot in this server.")

    snap = await _snapshot(guild_id)
    live_channel_ids, live_role_ids = _live_ids(snap, guild_id)
    channel_id = _valid_id(body.channel_id, live_channel_ids)
    if body.enabled and not channel_id:
        raise APIError(400, ErrorCode.bad_request, "Pick a channel before enabling market watch.")

    mw = cfg.market_watch
    # Ping roles only change when the caller can manage them (else keep existing).
    if _has_capability(ctx, config_perms, "manage_ping_roles"):
        ping_role_ids = _valid_ids(body.ping_role_ids, live_role_ids)
    else:
        ping_role_ids = mw.ping_role_ids
    # Normalize + de-dupe the watchlist (cap at 50). Preserve last_alert_sig for kept
    # items so editing the list doesn't re-alert deals we've already posted.
    prior = {it.name.lower(): it for it in mw.items}
    items: list[MarketWatchItem] = []
    seen: set[str] = set()
    for raw in body.items[:50]:
        nm = (raw.name or "").strip()
        if not nm or nm.lower() in seen:
            continue
        seen.add(nm.lower())
        prev = prior.get(nm.lower())
        cap = (float(raw.max_price_each)
               if raw.max_price_each is not None and raw.max_price_each > 0 else None)
        items.append(MarketWatchItem(
            name=nm, max_price_each=cap,
            last_alert_sig=prev.last_alert_sig if prev else None,
        ))
    mw.enabled = body.enabled
    mw.channel_id = channel_id
    mw.ping_role_ids = ping_role_ids
    mw.channel_missing = False
    mw.items = items
    await _save_config(existing, cfg, user)
    return _detail_payload(guild_id, ctx, cfg, snap)


# Clubs: Discord-side proxies of in-game Trove clubs.

def _club_view(club: Club) -> dict:
    return {
        "id": str(club.id),
        "name": club.name,
        "public": club.public,
        "description": club.description,
        "banner_url": club.banner_url,
        "avatar_url": club.avatar_url,
        "discord_url": club.discord_url,
        "website_url": club.website_url,
        "role_links": {rank: str(rid) for rank, rid in club.role_links.items()},
    }


def _club_url(value: str | None, label: str) -> str | None:
    """Validate an optional http(s) URL field, or 400."""
    if value is None:
        return None
    v = str(value).strip()
    if not v:
        return None
    if not v.startswith(("http://", "https://")):
        raise APIError(400, ErrorCode.bad_request, f"{label} must start with http:// or https://.")
    if len(v) > 500:
        raise APIError(400, ErrorCode.bad_request, f"{label} is too long.")
    return v


async def _require_clubs(guild_id: int, user: SiteUser) -> dict:
    """Member ctx + a manage_clubs capability check (admins always pass)."""
    ctx = await _member_ctx(guild_id, user)
    cfg = await GuildConfig.find_one(GuildConfig.guild_id == guild_id)
    if not _has_capability(ctx, cfg.config_perms if cfg else {}, "manage_clubs"):
        raise APIError(403, ErrorCode.forbidden,
                       "You don't have permission to manage clubs in this server.")
    return ctx


async def _get_club(guild_id: int, club_id: str) -> Club:
    try:
        club = await Club.get(PydanticObjectId(club_id))
    except Exception:
        club = None
    if club is None or club.guild_id != guild_id:
        raise APIError(404, ErrorCode.not_found, "Club not found.")
    return club


@router.get("/guilds/{guild_id}/clubs")
async def list_clubs(guild_id: int, user: SiteUser = Depends(get_current_site_user)) -> dict:
    """The guild's clubs + the role picker. Re-checks live roles and drops any
    rank->role link whose role was deleted (no events; checked on load)."""
    await _require_clubs(guild_id, user)
    snap = await _snapshot(guild_id)
    live_role_ids = {int(r["id"]) for r in snap["roles"]}
    clubs = await Club.find(Club.guild_id == guild_id).sort("+created_at").to_list()
    for club in clubs:
        if reconcile_club(club, live_role_ids):
            club.updated_at = utcnow()
            await club.save()
    return {
        "guild_id": str(guild_id),
        "max_clubs": MAX_CLUBS_PER_GUILD,
        "ranks": [{"key": r, "label": CLUB_RANK_LABELS[r]} for r in CLUB_RANKS],
        "roles": discord_rest.assignable_roles(snap["roles"], guild_id),
        "clubs": [_club_view(c) for c in clubs],
    }


@router.post("/guilds/{guild_id}/clubs")
async def create_club(guild_id: int, user: SiteUser = Depends(get_current_site_user)) -> dict:
    """Create a new (blank) club, up to ``MAX_CLUBS_PER_GUILD`` per guild."""
    await _require_clubs(guild_id, user)
    if await Club.find(Club.guild_id == guild_id).count() >= MAX_CLUBS_PER_GUILD:
        raise APIError(400, ErrorCode.bad_request,
                       f"A server can have at most {MAX_CLUBS_PER_GUILD} clubs.")
    club = await Club(guild_id=guild_id, name="New Club", updated_by=user.discord_id).insert()
    return _club_view(club)


@router.put("/guilds/{guild_id}/clubs/{club_id}")
async def update_club(
    guild_id: int, club_id: str, body: ClubUpdate,
    user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Update a club's metadata + rank->role links. Links are validated against the
    live guild roles; a non-existent role is ignored."""
    await _require_clubs(guild_id, user)
    club = await _get_club(guild_id, club_id)

    name = (body.name or "").strip()
    if not name:
        raise APIError(400, ErrorCode.bad_request, "Club name is required.")

    snap = await _snapshot(guild_id)
    _, live_role_ids = _live_ids(snap, guild_id)
    role_links: dict[str, int] = {}
    for rank, rid in (body.role_links or {}).items():
        if rank in CLUB_RANKS and str(rid).isdigit() and int(rid) in live_role_ids:
            role_links[rank] = int(rid)

    club.name = name[:100]
    club.public = bool(body.public)
    club.description = ((body.description or "").strip()[:1000]) or None
    club.banner_url = _club_url(body.banner_url, "Banner URL")
    club.avatar_url = _club_url(body.avatar_url, "Profile picture URL")
    club.discord_url = _club_url(body.discord_url, "Discord link")
    club.website_url = _club_url(body.website_url, "Website link")
    club.role_links = role_links
    club.updated_by = user.discord_id
    club.updated_at = utcnow()
    await club.save()
    return _club_view(club)


@router.delete("/guilds/{guild_id}/clubs/{club_id}")
async def delete_club(
    guild_id: int, club_id: str, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    await _require_clubs(guild_id, user)
    club = await _get_club(guild_id, club_id)
    await club.delete()
    return {"deleted": True, "id": club_id}


def _member_view(m: dict) -> dict:
    user = m.get("user") or {}
    name = m.get("nick") or user.get("global_name") or user.get("username") or "Unknown"
    avatar = (f"https://cdn.discordapp.com/avatars/{user['id']}/{user['avatar']}.png?size=64"
              if user.get("avatar") and user.get("id") else None)
    return {"id": str(user.get("id")), "name": name, "avatar": avatar}


def _rank_entry(rank: str, linked: set[str]) -> dict:
    """A roster rank descriptor carrying the promote/demote targets the UI needs to
    enable its arrows + word its confirmation modal (``None`` where there's no
    move). Targets follow the linked-rank-skip rules in ``models.promote_rank``."""
    up = promote_rank(rank, linked)
    down = demote_rank(rank, linked)
    return {
        "key": rank, "label": CLUB_RANK_LABELS[rank],
        "promote_to": up, "promote_label": CLUB_RANK_LABELS[up] if up else None,
        "demote_to": down, "demote_label": CLUB_RANK_LABELS[down] if down else None,
    }


def _current_rank(held: set[int], role_links: dict[str, int]) -> str | None:
    """A member's effective club rank: the highest linked rank whose role they hold."""
    for rank in CLUB_RANKS:                        # hierarchy order -> highest wins
        rid = role_links.get(rank)
        if rid and rid in held:
            return rank
    return None


@router.get("/guilds/{guild_id}/clubs/{club_id}/roster")
async def club_roster(
    guild_id: int, club_id: str, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """The club roster: members of each linked rank role, grouped by rank (each
    member listed once, at their highest rank). Needs the Server Members intent -
    returns ``available: false`` with a hint when it isn't enabled. Each rank
    carries its promote/demote targets so the dashboard can drive role management."""
    await _require_clubs(guild_id, user)
    club = await _get_club(guild_id, club_id)
    linked = set(club.role_links)
    ranks = [_rank_entry(r, linked) for r in CLUB_RANKS if r in club.role_links]
    if not ranks:
        return {"available": True, "ranks": [], "roster": {}}

    try:
        members = await discord_rest.guild_members(guild_id)
    except DiscordRestError as exc:
        hint = ("Enable the Server Members Intent for the bot (Discord Developer Portal "
                "→ Bot → Privileged Gateway Intents) to load the roster."
                if "403" in str(exc) else f"Couldn't load members: {exc}")
        return {"available": False, "ranks": ranks, "roster": {}, "error": hint}

    roster: dict[str, list] = {rk["key"]: [] for rk in ranks}
    for m in members:
        if (m.get("user") or {}).get("bot"):
            continue
        held = {int(r) for r in m.get("roles", [])}
        rank = _current_rank(held, club.role_links)
        if rank:
            roster[rank].append(_member_view(m))
    return {"available": True, "ranks": ranks, "roster": roster}


@router.post("/guilds/{guild_id}/clubs/{club_id}/roster/{member_id}/{action}")
async def club_roster_action(
    guild_id: int, club_id: str, member_id: int, action: str,
    user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Promote or demote a roster member one rank step (the bot edits their Discord
    roles). The target follows the linked-rank-skip rules; president is never a
    promotion target. Makes the target rank the member's ONLY linked rank role
    (adds it, then strips the others) so they end up cleanly at the new rank."""
    if action not in ("promote", "demote"):
        raise APIError(404, ErrorCode.not_found, "Unknown roster action.")
    await _require_clubs(guild_id, user)
    club = await _get_club(guild_id, club_id)
    linked = set(club.role_links)
    if not linked:
        raise APIError(400, ErrorCode.bad_request, "This club has no rank roles linked yet.")

    member = await discord_rest.guild_member(guild_id, member_id)
    if member is None:
        raise APIError(404, ErrorCode.not_found, "That member is no longer in this server.")
    if (member.get("user") or {}).get("bot"):
        raise APIError(400, ErrorCode.bad_request, "Bots can't hold a club rank.")

    held = {int(r) for r in member.get("roles", [])}
    current = _current_rank(held, club.role_links)
    if current is None:
        raise APIError(400, ErrorCode.bad_request,
                       "This member doesn't hold any of the club's rank roles yet.")

    target = (promote_rank(current, linked) if action == "promote"
              else demote_rank(current, linked))
    if target is None:
        raise APIError(409, ErrorCode.conflict,
                       "There's no higher rank to promote to." if action == "promote"
                       else "There's no lower rank to demote to.")

    target_role = club.role_links[target]
    reason = f"Kiwi club {action}: {current} -> {target} (dashboard)"
    try:
        if target_role not in held:               # add target first - never leave them rankless
            await discord_rest.add_member_role(guild_id, member_id, target_role, reason=reason)
        for rank in CLUB_RANKS:                    # then strip every other linked rank role
            rid = club.role_links.get(rank)
            if rid and rid != target_role and rid in held:
                await discord_rest.remove_member_role(guild_id, member_id, rid, reason=reason)
    except DiscordRestError as exc:
        hint = ("The bot needs the Manage Roles permission, and its own role must sit "
                "above the club's rank roles in the server's role list."
                if "403" in str(exc) else f"Discord wouldn't apply the change: {exc}")
        raise APIError(502, ErrorCode.service_unavailable, hint) from exc

    return {
        "ok": True, "action": action, "member": _member_view(member)["name"],
        "from": current, "from_label": CLUB_RANK_LABELS[current],
        "to": target, "to_label": CLUB_RANK_LABELS[target],
    }


@router.put("/guilds/{guild_id}/permissions")
async def set_permissions(
    guild_id: int, body: PermissionsUpdate, user: SiteUser = Depends(get_current_site_user),
) -> dict:
    """Map config capabilities -> Discord role ids (owner/admin only). Submitted ids
    are filtered to known capabilities + currently-existing roles before storing."""
    ctx = await _member_ctx(guild_id, user)
    _require_admin(ctx)
    live_roles = {int(r["id"]) for r in await _discord(discord_rest.guild_assignable_roles(guild_id))}

    cleaned: dict[str, list[int]] = {}
    for cap, role_ids in body.permissions.items():
        if cap not in CAPABILITIES:
            continue
        valid = sorted(set(_valid_ids(role_ids, live_roles)))
        if valid:
            cleaned[cap] = valid

    existing = await GuildConfig.find_one(GuildConfig.guild_id == guild_id)
    cfg = existing or GuildConfig(guild_id=guild_id)
    cfg.config_perms = cleaned
    await _save_config(existing, cfg, user)

    return {"saved": True, "permissions": {k: [str(r) for r in v] for k, v in cleaned.items()}}
