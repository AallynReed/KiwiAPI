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

    # Login identity.
    username: str                              # canonical lowercase
    email: EmailStr                            # canonical lowercase
    hashed_password: str

    # Profile.
    display_name: str | None = None            # human-presented label
    is_active: bool = True
    is_verified: bool = False                  # email-verified

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

    # Mirror dev-portal field so a hard-bounce can pause delivery without
    # silently spamming a dead inbox. Cleared when the user verifies a
    # new address.
    email_bounced: bool = False

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_login_at: datetime | None = None

    class Settings:
        name = "site_users"
        indexes = [
            IndexModel([("username", ASCENDING)], unique=True),
            IndexModel([("email", ASCENDING)], unique=True),
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
