"""Request + response models for the site-side auth endpoints."""
from datetime import datetime

from pydantic import BaseModel, Field


class SiteUpdateProfileRequest(BaseModel):
    display_name: str | None = Field(default=None, max_length=80)
    # Opt-in notification email. Send "" to clear it, an address to set it, or omit
    # to leave it unchanged. Validated in the handler.
    notify_email: str | None = Field(default=None, max_length=254)


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
    # Optional: browsers now send the refresh token as an HttpOnly cookie and
    # POST an empty body. Non-browser clients (desktop app) still pass it here.
    refresh_token: str | None = None


class SiteLogoutRequest(BaseModel):
    refresh_token: str | None = None


class SiteUsernameRequestBody(BaseModel):
    """Request to change the frozen Trove username (admin-approved)."""
    username: str = Field(min_length=3, max_length=24)


class SiteUserPublic(BaseModel):
    id: str
    username: str                              # frozen "Trove username" (mod handle)
    discord_handle: str = ""                   # live Discord handle (display only)
    notify_email: str | None = None            # opt-in notifications address (or None)
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
    """Claim status for the dashboard. ``detail`` carries the message shown
    to the user (now always the "pending manual review" line - verification
    is a master approval, not self-service)."""
    verified: bool
    detail: str
    user: SiteUserPublic
