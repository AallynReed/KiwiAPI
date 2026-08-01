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
from pydantic import Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class SiteUser(Document):
    """A registered showcase-site user. Identity is the Discord account (no email
    stored); `username` is unique and lower-cased so lookups are O(index seek)."""

    # Sign-in is Discord-only; there is no local password. `username` is the
    # website "Trove username" (canonical lowercase) - the handle for
    # mods/modpacks/profiles. FROZEN: seeded from the Discord handle at signup,
    # then only changed via the admin-approved change flow, so renaming on
    # Discord never shifts a user's mod handles/URLs.
    username: str
    # LIVE Discord handle, resynced on every login (display only); may drift
    # on Discord's side without touching `username`.
    discord_handle: str = ""
    # Sign-in is Discord-only and we don't request the `email` OAuth scope - the
    # Discord id is the sole identity (data minimization). `notify_email` below is a
    # SEPARATE, purely OPT-IN address the user can add themselves ONLY to receive
    # notifications (giveaway wins, account/content actions). Default None = no email
    # stored at all. Never collected at signup; used for nothing but notifications.
    notify_email: str | None = None

    display_name: str | None = None
    is_active: bool = True
    is_verified: bool = False                  # Discord identity verified (always true for Discord logins)
    # Set when the user self-deletes: the row is kept as an anonymized tombstone
    # (all PII stripped) so their mods/modpacks stay live under a non-identifying
    # owner, but it can never be logged into or re-linked. See app/site_auth/account.py.
    is_deleted: bool = False

    # The sole identity for the account - every SiteUser is created via Discord.
    discord_id: int | None = None

    # Discord avatar hash, turned into a cdn.discordapp.com URL by
    # _discord_avatar_url (we never host the image). ``None`` = no custom avatar
    # → fall back to Discord's default embed avatar.
    discord_avatar: str | None = None

    # NOTE: the user's Discord guild (server) list is deliberately NOT stored here.
    # It's fetched LIVE from Discord only when the user actually opens the Dashboard
    # "Discord Bot" tab, using a short-lived Discord access token cached in Redis at
    # login (see app/site_auth/oauth.py). GDPR data-minimization: we don't keep the
    # user's server membership (a social graph) at rest. Any list collected by the
    # old flow is purged at startup (app/core/database.py).

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

    # --- Creator token (Mods Hub API access) ------------------------------
    # ONE per account, minted lazily the first time the user opens the "API
    # access" panel. A developer pastes it into their dev-portal account to
    # CONNECT to this creator; the connection (`ModCreatorLink`) is what actually
    # carries the per-project permissions, so the token is a connect code, not a
    # per-call credential. Only the sha256 is stored - the plaintext is shown once
    # at mint\rotate. Rotating mints a new token AND revokes every connection made
    # with the old one, which is the "cut everyone off" button.
    creator_token_hash: str | None = None
    creator_token_prefix: str | None = None    # display slice, e.g. "kiwi_creator_ab12cd34"
    creator_token_at: datetime | None = None   # last mint\rotate

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
            # The creator-token lookup on connect. PARTIAL (not sparse) for the
            # same reason as discord_id above: rows store the field present-but-
            # null, so a sparse unique index would collide every unminted account.
            IndexModel(
                [("creator_token_hash", ASCENDING)],
                unique=True,
                partialFilterExpression={"creator_token_hash": {"$type": "string"}},
            ),
        ]


class SiteSession(Document):
    """One login session for a SiteUser. Same refresh-rotation scheme as
    ``app/auth/models.Session`` - hashed refresh token, single-use
    rotation, revoke flag, expiry timestamp."""

    site_user_id: PydanticObjectId
    refresh_token_hash: str                    # sha256 of the current refresh token
    # Coarse "Browser on OS" label for the "active sessions" list. We deliberately
    # store NEITHER the raw User-Agent NOR the IP address (data minimization).
    device: str | None = None

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
