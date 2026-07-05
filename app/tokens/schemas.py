from datetime import datetime
from typing import Literal

from pydantic import BaseModel, Field


class CreateTokenRequest(BaseModel):
    name: str = Field(min_length=1, max_length=80)
    # Scope bitmask (OR of scope bits). 0 = all scopes, present and future.
    scopes: int = Field(default=0, ge=0)
    # Opt-in IP allowlist (see ApiToken for storage/CIDR rationale). Empty = no
    # restriction; requests from any other IP are rejected once set.
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
    # Only name and allowed IPs are editable (never the secret or scopes).
    # ``allowed_ips`` replaces the whole list: ``[]`` drops all pinning,
    # ``None`` leaves it alone (no add/remove - the user only sees a count).
    name: str | None = Field(default=None, min_length=1, max_length=80)
    allowed_ips: list[str] | None = None


class TokenPublic(BaseModel):
    id: str
    name: str
    prefix: str
    scopes: int  # the raw bitmask (0 = all)
    scope_names: list[str]  # decoded names of the set bits (empty when all)
    # Pinned IPs are hashed, so only the count is exposed; 0 = no restriction.
    allowed_ip_count: int
    revoked: bool
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    created_at: datetime
    last_used_at: datetime | None = None
    rotated_at: datetime | None = None
    expires_at: datetime | None = None
    request_count: int


class TokenCreatedResponse(TokenPublic):
    # The full secret - returned exactly once, at creation (or rotation) time.
    token: str
