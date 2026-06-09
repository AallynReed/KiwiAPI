from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field


class CreateTokenRequest(BaseModel):
    name: str = Field(min_length=1, max_length=80)
    # Scope bitmask (OR of scope bits). 0 = all scopes, present and future.
    scopes: int = Field(default=0, ge=0)
    # Optional defence-in-depth for the TOKEN OWNER: if any IPs are supplied,
    # requests from other IPs are rejected. Stored HASHED - neither admins
    # nor a DB breach can read them back; the API only ever knows whether a
    # candidate IP matches. CIDRs are NOT supported (hashes can't range-
    # match). Empty (the default) means no IP restriction.
    allowed_ips: list[str] = Field(default_factory=list)
    # 30 (default), 60, 90 days, or null for no expiry.
    expires_in_days: Literal[30, 60, 90] | None = 30


# Default revoke reasons offered in the UI; users may also supply a custom one.
REVOKE_REASONS: list[str] = [
    "No longer using it",
    "Rotating to a new token",
    "Possibly compromised",
    "Created by mistake",
]


class RevokeTokenRequest(BaseModel):
    reason: str = Field(min_length=1, max_length=200)


class EditTokenRequest(BaseModel):
    # Only the name and allowed IPs are editable - never the secret or scopes.
    # ``allowed_ips`` replaces the whole pinned list (we can't add/remove a
    # specific hash since the user only sees a count). Pass ``[]`` to drop
    # all pinning; pass ``None`` to leave the list alone.
    name: str | None = Field(default=None, min_length=1, max_length=80)
    allowed_ips: list[str] | None = None


class TokenPublic(BaseModel):
    id: str
    name: str
    prefix: str
    scopes: int  # the raw bitmask (0 = all)
    scope_names: list[str]  # decoded names of the set bits (empty when all)
    # The pinned IPs are stored hashed - the owner can SEE how many they
    # pinned but can't read them back (same property as password storage).
    # 0 means no IP restriction on this token.
    allowed_ip_count: int
    revoked: bool
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    created_at: datetime
    last_used_at: datetime | None = None
    # NOTE: last_used_ip was removed - keeping it plaintext leaked PII; hashing
    # it would render the field useless to the owner.
    rotated_at: datetime | None = None
    expires_at: datetime | None = None
    request_count: int


class TokenCreatedResponse(TokenPublic):
    # The full secret - returned exactly once, at creation (or rotation) time.
    token: str
