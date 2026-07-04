"""Per-guild configuration for the Kiwi gateway bot.

Written by the dev-portal dashboard (the server owner / delegated roles) and read
by the bot process. One doc per Discord guild the bot is active in.
"""
from datetime import datetime

from beanie import Document
from pydantic import BaseModel, Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow
from app.embed_templates import EmbedTemplate


class AnnouncementSetting(BaseModel):
    """One announcement type's per-guild config, stored under
    ``GuildConfig.announcements[<registry key>]`` (see app/bot/announcements.py)."""

    enabled: bool = False
    channel_id: int | None = None
    ping_role_ids: list[int] = Field(default_factory=list)   # roles to @-mention (multi)
    ping_role_id: int | None = None              # legacy single; folded into ping_role_ids
    # Optional custom embed. When ``enabled``, the announcer renders this (against the
    # type's live variable context) instead of the default embed/image banner. A custom
    # embed uses native Discord ``<t:>`` timestamps, so it isn't image-refreshed.
    template: EmbedTemplate | None = None
    # Edge-trigger guard: the anchor we last posted for. A string so it covers
    # both window timestamps ("1718200000") and state signatures ("status:down").
    # Each new anchor is posted once; enabling mid-window seeds this to the
    # current anchor so the FIRST post is the next change, not the in-progress one.
    last_anchor: str | None = None
    # Set when the configured channel has been deleted from the guild. Surfaced
    # loudly in the dashboard and (while true) suppresses sends; cleared when the
    # user repoints to a live channel. See app/bot/reconcile.py.
    channel_missing: bool = False


class LiveBoard(BaseModel):
    """The self-updating "Trove Now" board: one message the bot keeps current by
    editing in place on every event (see app/bot/liveboard.py)."""

    enabled: bool = False
    channel_id: int | None = None
    message_id: int | None = None                # the board message we edit; None until posted
    channel_missing: bool = False                # configured channel was deleted


class MarketWatchItem(BaseModel):
    """One watched marketplace item. Alerts when the cheapest active listing's
    per-unit price is at/under ``max_price_each`` (or whenever it's listed at all,
    when that's None). ``last_alert_sig`` edge-triggers: the cheapest price we last
    alerted for, so the same deal isn't re-posted on every hourly refresh."""

    name: str
    max_price_each: float | None = None
    last_alert_sig: str | None = None


class MarketWatch(BaseModel):
    """Per-guild marketplace watch: one channel + ping roles, plus a watchlist of
    items. Checked when new market data lands (the hourly ingest 'market' event);
    see app/bot/market_watch.py."""

    enabled: bool = False
    channel_id: int | None = None
    ping_role_ids: list[int] = Field(default_factory=list)
    channel_missing: bool = False                # configured channel was deleted
    items: list[MarketWatchItem] = Field(default_factory=list)


class GuildConfig(Document):
    guild_id: int                                       # Discord guild id - unique

    # --- Announcements (per-type config, keyed by the registry token) ---
    # Replaces the legacy single-challenge fields below; ``migrate_legacy`` folds
    # those into this map once, then they stay empty.
    announcements: dict[str, AnnouncementSetting] = Field(default_factory=dict)

    # --- Live "Trove Now" board (one self-updating message) ---
    live_board: LiveBoard = Field(default_factory=LiveBoard)

    # --- Marketplace watch (one channel + a watchlist of items) ---
    market_watch: MarketWatch = Field(default_factory=MarketWatch)

    # --- Bot output language ---
    # BCP-ish code from app.i18n.SUPPORTED (en/fr/de/pt-PT/ru/ja/zh-CN). The bot
    # speaks this language in THIS server: announcements, the live board, and slash
    # replies invoked here. Default English; validated against SUPPORTED on save.
    language: str = "en"

    # --- Role-based config permissions ---
    # capability -> [discord role ids] allowed to set it. The guild owner / admins
    # always have full control regardless. Roles are validated against the live
    # guild role list (reconcile + check time), so a deleted role loses its grant.
    config_perms: dict[str, list[int]] = Field(default_factory=dict)

    # --- Legacy (pre-registry) hourly-challenge fields ---
    # Kept so existing docs still load; read once by ``migrate_legacy`` and then
    # cleared. Do not read these directly - use ``announcements["hourly_challenge"]``.
    hourly_challenge_enabled: bool = False
    announce_channel_id: int | None = None
    last_announced_challenge_anchor: int | None = None

    # --- Audit ---
    updated_by: int | None = None                       # discord user id of last editor
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "guild_configs"
        indexes = [IndexModel([("guild_id", ASCENDING)], unique=True)]

    def migrate_legacy(self) -> bool:
        """Fold legacy fields into the current shape once (idempotent; safe to call
        on every load). Returns True if anything changed (the caller persists).

        Covers: the pre-registry single-challenge fields -> ``announcements``, and
        the single ``ping_role_id`` -> the ``ping_role_ids`` list."""
        changed = False
        if (self.hourly_challenge_enabled or self.announce_channel_id is not None
                or self.last_announced_challenge_anchor is not None):
            if "hourly_challenge" not in self.announcements:
                self.announcements["hourly_challenge"] = AnnouncementSetting(
                    enabled=self.hourly_challenge_enabled,
                    channel_id=self.announce_channel_id,
                    last_anchor=(str(self.last_announced_challenge_anchor)
                                 if self.last_announced_challenge_anchor is not None else None),
                )
            self.hourly_challenge_enabled = False
            self.announce_channel_id = None
            self.last_announced_challenge_anchor = None
            changed = True
        # Split the single hourly-challenge type into per-category types, carrying
        # the channel / ping roles / last anchor across so an upgraded guild keeps
        # announcing (now once per category). Idempotent: the pop no-ops afterwards.
        legacy_ch = self.announcements.pop("hourly_challenge", None)
        if legacy_ch is not None:
            for cat in ("collection", "rampage", "racing", "target", "dungeon"):
                key = f"challenge_{cat}"
                if key not in self.announcements:
                    self.announcements[key] = AnnouncementSetting(
                        enabled=legacy_ch.enabled,
                        channel_id=legacy_ch.channel_id,
                        ping_role_ids=list(legacy_ch.ping_role_ids),
                        last_anchor=legacy_ch.last_anchor,
                        channel_missing=legacy_ch.channel_missing,
                    )
            changed = True
        # Single ping role -> the multi list.
        for s in self.announcements.values():
            if s.ping_role_id is not None:
                if s.ping_role_id not in s.ping_role_ids:
                    s.ping_role_ids.append(s.ping_role_id)
                s.ping_role_id = None
                changed = True
        return changed


class TrackedAnnouncement(Document):
    """One posted announcement message the bot still needs to manage.

    Recorded when an ``auto_manage`` announcement type posts (see
    app/bot/announcer.py). The janitor sweep deletes the Discord message once it's
    irrelevant (``expires_at`` passed) and then drops this row, so the collection
    only ever holds live, pending cleanups - and it survives restarts (the sweep
    just re-reads it). ``refresh`` marks image announcements that need re-editing
    each minute to keep their baked-in "ends in X" accurate (wired with images)."""

    guild_id: int
    channel_id: int
    message_id: int
    kind: str                                    # announcement registry key
    anchor: str                                  # the occurrence this message is for
    expires_at: int | None = None                # unix; delete at/after this (None = supersede-only)
    refresh: bool = False                        # re-edit to refresh an image countdown
    refresh_v: str | None = None                 # last image ?v token edited in (cadence-scaled)
    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "tracked_announcements"
        indexes = [
            IndexModel([("guild_id", ASCENDING), ("kind", ASCENDING)]),   # supersede lookup
            IndexModel([("expires_at", ASCENDING)]),                      # janitor sweep
        ]


# In-game Trove club ranks, in hierarchy order (highest first). The key is the
# permanent token (stored in Club.role_links); the value is the dashboard label.
CLUB_RANKS: tuple[str, ...] = ("president", "vice_president", "officer", "captain", "member")
CLUB_RANK_LABELS: dict[str, str] = {
    "president": "President", "vice_president": "Vice-President",
    "officer": "Officer", "captain": "Captain", "member": "Member",
}
MAX_CLUBS_PER_GUILD = 3


def promote_rank(current: str, linked: set[str]) -> str | None:
    """The rank a member at ``current`` is promoted INTO: the nearest LINKED rank
    above them, walking up one step at a time and skipping ranks with no linked
    role. **President is never a promotion target** (it can only be set in-game),
    so the ceiling is vice-president. ``None`` if there's no higher linked rank.

    e.g. a member promotes to captain; if captain isn't linked but officer is, it
    skips straight to officer. ``linked`` is the set of ranks in ``role_links``."""
    if current not in CLUB_RANKS:
        return None
    cur = CLUB_RANKS.index(current)
    for idx in range(cur - 1, 0, -1):          # toward higher rank; stop above president (idx 0)
        if CLUB_RANKS[idx] in linked:
            return CLUB_RANKS[idx]
    return None


def demote_rank(current: str, linked: set[str]) -> str | None:
    """The rank a member at ``current`` is demoted INTO: the nearest LINKED rank
    below them (skipping unlinked ranks); floor is member. ``None`` if there's no
    lower linked rank. A president CAN be demoted (the no-president rule only
    blocks promoting *to* president)."""
    if current not in CLUB_RANKS:
        return None
    cur = CLUB_RANKS.index(current)
    for idx in range(cur + 1, len(CLUB_RANKS)):    # toward lower rank; member is last
        if CLUB_RANKS[idx] in linked:
            return CLUB_RANKS[idx]
    return None


class Club(Document):
    """A Discord-side proxy of an in-game Trove club. A guild can hold up to
    ``MAX_CLUBS_PER_GUILD``. ``role_links`` maps an in-game rank -> the Discord role
    that represents it (optional, not enforced); roles are validated against the live
    guild on load (``reconcile.reconcile_club``), so a deleted role's link is dropped.
    ``public`` (default off) gates a future public clubs directory."""

    guild_id: int
    name: str                                    # required
    public: bool = False
    description: str | None = None
    banner_url: str | None = None
    avatar_url: str | None = None                # profile picture
    discord_url: str | None = None
    website_url: str | None = None
    role_links: dict[str, int] = Field(default_factory=dict)   # rank -> discord role id

    updated_by: int | None = None                # discord user id of last editor
    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "clubs"
        indexes = [
            IndexModel([("guild_id", ASCENDING)]),
            IndexModel([("public", ASCENDING)]),          # future public directory
        ]
