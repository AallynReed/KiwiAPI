from datetime import datetime

from beanie import Document, PydanticObjectId
from pydantic import Field
from pymongo import ASCENDING, IndexModel

from app.core.utils import utcnow


class ApiToken(Document):
    """A user-issued API key. Only the SHA-256 hash of the token is stored."""

    user_id: PydanticObjectId
    name: str
    prefix: str  # human-recognizable leading slice, e.g. "kiwi_ab12cd34"
    hashed_token: str

    scopes: int = 0  # bitmask of granted scopes; 0 = all scopes (current + future)
    # Per-token salt + HMAC-SHA256 hashes of pinned IPs. Hashed like passwords
    # so neither admins nor a DB breach reveal which IPs are pinned. Empty list
    # means "no IP restriction" - the allowlist is opt-in (see `core/ip_hash.py`).
    # CIDRs are NOT supported - a hash can't range-match.
    ip_salt: str | None = None                             # urlsafe-b64; minted with the token
    allowed_ip_hashes: list[str] = Field(default_factory=list)
    revoked: bool = False
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    expires_at: datetime | None = None
    # Set once an "expiring soon" warning email has gone out, so we don't repeat
    # it on every maintenance sweep. Reset when the token is rotated.
    expiry_warned: bool = False

    created_at: datetime = Field(default_factory=utcnow)
    last_used_at: datetime | None = None
    # No ``last_used_ip``: plaintext leaks PII, and hashing it would deny the
    # owner the "is this me?" compare that is the field's only point.
    rotated_at: datetime | None = None
    request_count: int = 0

    class Settings:
        name = "api_tokens"
        indexes = [
            IndexModel([("hashed_token", ASCENDING)], unique=True),
            IndexModel([("user_id", ASCENDING)]),
            # Inactivity auto-revoke sweep scans active tokens by last activity.
            IndexModel([("revoked", ASCENDING), ("last_used_at", ASCENDING)]),
            # Expiry-warning sweep scans active tokens with an upcoming expiry.
            IndexModel([("revoked", ASCENDING), ("expires_at", ASCENDING)]),
        ]
