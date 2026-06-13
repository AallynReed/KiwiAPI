"""Request + response models for the site-side auth endpoints."""
from datetime import datetime

from pydantic import BaseModel, EmailStr, Field


class SiteUpdateProfileRequest(BaseModel):
    display_name: str | None = Field(default=None, max_length=80)


class SiteClaimTroveNameRequest(BaseModel):
    """Claim a Trove player name. v1 is self-attest - anybody can claim
    any name. UI shows an 'unverified' badge. Future: prove ownership
    via captured-in-club-bio or similar."""
    trove_name: str = Field(min_length=1, max_length=80)


class SiteTokenResponse(BaseModel):
    access_token: str
    refresh_token: str
    token_type: str = "bearer"
    expires_in: int


class SiteRefreshRequest(BaseModel):
    refresh_token: str


class SiteLogoutRequest(BaseModel):
    refresh_token: str | None = None


class SiteUserPublic(BaseModel):
    id: str
    username: str
    email: EmailStr
    display_name: str | None = None
    # Ready-to-use Discord CDN avatar URL (built server-side from the stored
    # hash). Always populated for Discord accounts - falls back to Discord's
    # default avatar - so the UI can render a picture without hosting one.
    avatar_url: str | None = None
    is_active: bool
    is_verified: bool
    claimed_trove_name: str | None = None
    claimed_trove_display: str | None = None
    claimed_at: datetime | None = None
    claim_verified: bool = False
    claim_verified_at: datetime | None = None
    # How many baseline boards are tracked at claim time - surfaced so
    # the dashboard can render "We're watching N boards" without making
    # a second call to inspect ``claim_baseline``.
    claim_baseline_board_count: int = 0
    created_at: datetime
    last_login_at: datetime | None = None


class SiteVerifyTroveClaimResponse(BaseModel):
    """Response from the on-demand verification endpoint. Lets the
    dashboard show a precise success/failure message ("Verified! Score
    went up on Enemies Defeated") rather than a generic toast."""
    verified: bool
    detail: str
    user: SiteUserPublic
