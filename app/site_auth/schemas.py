"""Request + response models for the site-side auth endpoints."""
import re
from datetime import datetime

from pydantic import BaseModel, EmailStr, Field, field_validator

from app.core.config import settings

# Username constraints - readable, URL-safe, no impersonation foot-guns.
USERNAME_PATTERN = re.compile(r"^[a-zA-Z0-9_]{3,24}$")


def _validate_username(v: str) -> str:
    """Lowercase the username and enforce the alpha-num + underscore
    shape. Called from request models so a bad signup is rejected with
    a clear 422 rather than leaking into the DB layer."""
    if not isinstance(v, str):
        raise ValueError("username must be a string")
    if not USERNAME_PATTERN.match(v):
        raise ValueError(
            "username must be 3–24 chars, letters/digits/underscore only",
        )
    return v.lower()


class SiteSignupRequest(BaseModel):
    username: str
    email: EmailStr
    password: str = Field(min_length=settings.password_min_length, max_length=256)
    display_name: str | None = Field(default=None, max_length=80)
    captcha_token: str | None = Field(default=None, description="Captcha response token")

    @field_validator("username", mode="before")
    @classmethod
    def _username_shape(cls, v: str) -> str:
        return _validate_username(v)


class SiteLoginRequest(BaseModel):
    """Login by username OR email - the field is named ``identifier``
    so the same form on the page can accept either without dispatching
    on shape on the client. The server picks the right index lookup."""
    identifier: str = Field(min_length=3, max_length=120)
    password: str
    captcha_token: str | None = Field(default=None, description="Captcha response token")


class SiteForgotPasswordRequest(BaseModel):
    email: EmailStr
    captcha_token: str | None = Field(default=None, description="Captcha response token")


class SiteResendVerificationRequest(BaseModel):
    email: EmailStr


class SiteResetPasswordRequest(BaseModel):
    token: str
    new_password: str = Field(min_length=settings.password_min_length, max_length=256)


class SiteChangePasswordRequest(BaseModel):
    current_password: str
    new_password: str = Field(min_length=settings.password_min_length, max_length=256)


class SiteUpdateProfileRequest(BaseModel):
    display_name: str | None = Field(default=None, max_length=80)


class SiteClaimTroveNameRequest(BaseModel):
    """Claim a Trove player name. v1 is self-attest - anybody can claim
    any name. UI shows an 'unverified' badge. Future: prove ownership
    via captured-in-club-bio or similar."""
    trove_name: str = Field(min_length=1, max_length=80)


class SiteMessageResponse(BaseModel):
    message: str


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
