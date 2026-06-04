from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import EmailStr, Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow


class User(Document):
    """A registered account. Email is unique and stored lower-cased."""

    email: EmailStr
    hashed_password: str

    display_name: str | None = None
    is_active: bool = True
    is_verified: bool = False
    is_superuser: bool = False
    roles: list[str] = Field(default_factory=list)

    # Bumped to invalidate every outstanding access token at once (logout-all,
    # password/email change). Access tokens embed this and are checked against it.
    token_version: int = 0

    # Linked GitHub account id (for "Sign in with GitHub"). Null for password-only.
    github_id: int | None = None

    # Set when mail to this address hard-bounces (5xx). We stop sending to it
    # until the user fixes their address (clears on a verified email change).
    email_bounced: bool = False

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)
    last_login_at: datetime | None = None

    class Settings:
        name = "users"
        indexes = [
            IndexModel([("email", ASCENDING)], unique=True),
            # Partial (NOT sparse): every row stores github_id present-but-null,
            # so a sparse index would still treat all the nulls as duplicates.
            # This only enforces uniqueness on rows where github_id is a number.
            IndexModel(
                [("github_id", ASCENDING)],
                unique=True,
                partialFilterExpression={"github_id": {"$type": "number"}},
            ),
        ]


class Session(Document):
    """One login session, keyed by a hashed refresh token. Refresh rotates the
    token (updates the hash); revoking or expiring it ends the session."""

    user_id: PydanticObjectId
    refresh_token_hash: str  # sha256 of the current refresh token
    user_agent: str | None = None
    ip: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    last_used_at: datetime = Field(default_factory=utcnow)
    expires_at: datetime
    revoked: bool = False

    class Settings:
        name = "sessions"
        indexes = [
            IndexModel([("refresh_token_hash", ASCENDING)], unique=True),
            IndexModel([("user_id", ASCENDING), ("last_used_at", DESCENDING)]),
        ]
