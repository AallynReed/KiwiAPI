"""Beanie documents for the site-side accounts.

Mirrors ``app/auth/models.py`` but with:
  - a username field (unique, lowercased) - the dev portal is email-only
  - a ``claimed_trove_name`` slot used by the dashboard to look up the
    user's leaderboard appearances
  - separate collections so a public signup can NEVER accidentally
    grant access to the API-facing surface the dev portal manages
"""
from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import EmailStr, Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class SiteUser(Document):
    """A registered showcase-site user. Username + email both unique
    and stored lower-cased so login lookups are O(index seek)."""

    # Login identity. Sign-in is Discord-only; there is no local password.
    username: str                              # canonical lowercase
    email: EmailStr                            # canonical lowercase (from Discord)

    # Profile.
    display_name: str | None = None            # human-presented label
    is_active: bool = True
    is_verified: bool = False                  # Discord-verified email

    # Linked Discord account id (snowflake; for "Sign in with Discord"). The
    # sole identity for the account - every SiteUser is created via Discord.
    discord_id: int | None = None

    # Discord avatar hash. The API turns this into a cdn.discordapp.com URL
    # (see _discord_avatar_url) so we display the user's avatar without ever
    # hosting an image ourselves. Refreshed on each login; ``None`` = the user
    # has no custom avatar and we fall back to Discord's default embed avatar.
    discord_avatar: str | None = None

    # Cached Discord guild list (id/name/icon/owner/permissions) from the
    # `guilds` OAuth scope - powers the Dashboard's "Discord Bot" tab "your
    # servers" view. None until the user signs in with the guilds scope;
    # synced_at gates the "reconnect Discord" reprompt for older grants.
    discord_guilds: list[dict] | None = None
    discord_guilds_synced_at: datetime | None = None

    # Bumped on password / email change / logout-all so outstanding access
    # tokens lose their authority instantly. Same trick the dev portal
    # uses; the JWT carries the version it was minted against.
    token_version: int = 0

    # Leaderboard-identity claim. Stored lowercased for case-insensitive
    # match against captured ``LeaderboardEntry.player_name`` rows
    # (which Trove's dump preserves verbatim, but display-vs-canonical
    # rotates over time). ``None`` = no claim yet.
    claimed_trove_name: str | None = None      # lowercased
    claimed_trove_display: str | None = None   # whatever casing the user typed
    claimed_at: datetime | None = None

    # ── Trove-name verification ─────────────────────────────────────
    # v1 verification is score-progression: when the user claims a
    # name, we snapshot their current score on every (board, score)
    # they appear on at that moment. The user goes plays Trove -
    # raising at least one score on at least one lifetime-accumulating
    # board. On demand (Verify now button) we re-fetch their current
    # scores and compare; if any went up, the claim is verified.
    #
    # ``claim_verified`` is True once the check passes.
    # ``claim_baseline`` is ``{board_uuid (str): score}`` captured at
    # claim time. Stored as str-keyed dict since Mongo doesn't allow
    # numeric keys.
    # ``claim_verified_at`` records when the check passed.
    claim_verified: bool = False
    claim_baseline: dict[str, float] = Field(default_factory=dict)
    claim_verified_at: datetime | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_login_at: datetime | None = None

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
