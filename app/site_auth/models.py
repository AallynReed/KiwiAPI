"""Beanie documents for the site-side accounts.

Mirrors ``app/auth/models.py`` but with:
  - a username field (unique, lowercased) - the dev portal is email-only
  - a ``claimed_trove_name`` slot used by the dashboard to look up the
    user's leaderboard appearances
  - separate collections so a public signup can NEVER accidentally
    grant access to the API-facing surface the dev portal manages
"""
from datetime import datetime
from typing import Literal

from beanie import Document, PydanticObjectId
from pydantic import EmailStr, Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class SiteUser(Document):
    """A registered showcase-site user. Username + email both unique
    and stored lower-cased so login lookups are O(index seek)."""

    # Sign-in is Discord-only; there is no local password. `username` is the
    # website "Trove username" (canonical lowercase) - the handle for
    # mods/modpacks/profiles. FROZEN: seeded from the Discord handle at signup,
    # then only changed via the admin-approved change flow, so renaming on
    # Discord never shifts a user's mod handles/URLs.
    username: str
    # LIVE Discord handle, resynced on every login (display only); may drift
    # on Discord's side without touching `username`.
    discord_handle: str = ""
    email: EmailStr                            # canonical lowercase (from Discord)

    display_name: str | None = None
    is_active: bool = True
    is_verified: bool = False                  # email verified Discord-side

    # The sole identity for the account - every SiteUser is created via Discord.
    discord_id: int | None = None

    # Discord avatar hash, turned into a cdn.discordapp.com URL by
    # _discord_avatar_url (we never host the image). ``None`` = no custom avatar
    # → fall back to Discord's default embed avatar.
    discord_avatar: str | None = None

    # Cached Discord guild list from the `guilds` OAuth scope - powers the
    # Dashboard's "Discord Bot" tab "your servers" view. None until signed in
    # with that scope; synced_at gates the "reconnect Discord" reprompt.
    discord_guilds: list[dict] | None = None
    discord_guilds_synced_at: datetime | None = None

    # Bumped on email change / logout-all so outstanding access tokens lose
    # authority instantly; the JWT carries the version it was minted against.
    token_version: int = 0

    # Leaderboard-identity claim. Stored lowercased for case-insensitive match
    # against captured ``LeaderboardEntry.player_name`` rows (Trove's dump
    # preserves casing verbatim, but display-vs-canonical rotates). ``None`` = no
    # claim yet.
    claimed_trove_name: str | None = None      # lowercased
    claimed_trove_display: str | None = None   # whatever casing the user typed
    claimed_at: datetime | None = None

    # Verification is a MANUAL master approval (the score-progression self-check
    # is retired). ``claim_baseline`` ({board_uuid str: score}; str keys because
    # Mongo disallows numeric document keys) is still captured at claim time, but
    # its only remaining reader is the board-count surfaced to the dashboard.
    claim_verified: bool = False
    claim_baseline: dict[str, float] = Field(default_factory=dict)
    claim_verified_at: datetime | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_login_at: datetime | None = None

    def clear_claim(self) -> None:
        """Release the leaderboard-identity claim, resetting every claim field
        (the caller still owns ``updated_at`` + persisting the change)."""
        self.claimed_trove_name = None
        self.claimed_trove_display = None
        self.claimed_at = None
        self.claim_verified = False
        self.claim_verified_at = None
        self.claim_baseline = {}

    class Settings:
        name = "site_users"
        indexes = [
            IndexModel([("username", ASCENDING)], unique=True),
            IndexModel([("email", ASCENDING)], unique=True),
            IndexModel(
                [("discord_id", ASCENDING)],
                unique=True,
                partialFilterExpression={"discord_id": {"$type": "number"}},
            ),
            # Partial: only enforce uniqueness on rows that actually claimed
            # a Trove name. Multiple ``None``s coexist fine; two users
            # claiming the same name don't.
            IndexModel(
                [("claimed_trove_name", ASCENDING)],
                unique=True,
                partialFilterExpression={"claimed_trove_name": {"$type": "string"}},
            ),
        ]


class SiteSession(Document):
    """One login session for a SiteUser. Same refresh-rotation scheme as
    ``app/auth/models.Session`` - hashed refresh token, single-use
    rotation, revoke flag, expiry timestamp."""

    site_user_id: PydanticObjectId
    refresh_token_hash: str                    # sha256 of the current refresh token
    user_agent: str | None = None
    ip: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    last_used_at: datetime = Field(default_factory=utcnow)
    expires_at: datetime
    revoked: bool = False

    class Settings:
        name = "site_sessions"
        indexes = [
            IndexModel([("refresh_token_hash", ASCENDING)], unique=True),
            IndexModel([("site_user_id", ASCENDING), ("last_used_at", DESCENDING)]),
        ]


class UsernameChangeRequest(Document):
    """A user's request to change their frozen **Trove username** to a new value
    (e.g. their in-game name). Reviewed by a master, who approves (renames the
    account's ``username``) or rejects with a ``reason``. One *pending* request per
    user; resolved ones are kept for history."""

    site_user_id: PydanticObjectId
    current_username: str
    requested_username: str                    # validated, canonical lowercase
    status: Literal["pending", "approved", "rejected"] = "pending"
    reason: str = ""                           # denial reason (shown to the user)
    resolved_by: PydanticObjectId | None = None  # master who approved/rejected
    resolved_at: datetime | None = None

    created_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "username_change_requests"
        indexes = [
            IndexModel([("status", ASCENDING), ("created_at", DESCENDING)]),
            IndexModel([("site_user_id", ASCENDING), ("created_at", DESCENDING)]),
        ]
