from datetime import datetime

from pydantic import BaseModel, EmailStr, Field

from app.core.config import settings


class SignupRequest(BaseModel):
    email: EmailStr
    password: str = Field(min_length=settings.password_min_length, max_length=256)
    display_name: str | None = Field(default=None, max_length=80)
    # Captcha response token from the widget (hCaptcha or Turnstile) on signup.
    captcha_token: str | None = Field(default=None, description="Captcha response token")


class LoginRequest(BaseModel):
    email: EmailStr
    password: str
    captcha_token: str | None = Field(default=None, description="Captcha response token")


class ForgotPasswordRequest(BaseModel):
    email: EmailStr
    captcha_token: str | None = Field(default=None, description="Captcha response token")


class ResendVerificationRequest(BaseModel):
    email: EmailStr


class ResetPasswordRequest(BaseModel):
    token: str
    new_password: str = Field(min_length=settings.password_min_length, max_length=256)


class MessageResponse(BaseModel):
    message: str


class ChangePasswordRequest(BaseModel):
    current_password: str
    new_password: str = Field(min_length=settings.password_min_length, max_length=256)


class ChangeEmailRequest(BaseModel):
    new_email: EmailStr
    password: str  # re-auth to change the email


class UpdateProfileRequest(BaseModel):
    display_name: str | None = Field(default=None, max_length=80)


class ScopeInfo(BaseModel):
    key: str          # "<resource>:<action>"
    bit: int
    resource: str = ""
    description: str


class DeleteAccountRequest(BaseModel):
    confirm_email: EmailStr  # must match the account email


class OAuthExchangeRequest(BaseModel):
    code: str


class PublicConfig(BaseModel):
    app_name: str
    api_url: str
    captcha_provider: str
    captcha_sitekey: str | None = None
    require_verified_for_tokens: bool
    scopes: list[ScopeInfo] = []
    token_creation_daily_limit: int
    revoke_reasons: list[str] = []
    github_oauth_enabled: bool = False


class TokenResponse(BaseModel):
    access_token: str
    refresh_token: str
    token_type: str = "bearer"
    expires_in: int  # access token lifetime, seconds


class RefreshRequest(BaseModel):
    refresh_token: str


class LogoutRequest(BaseModel):
    refresh_token: str | None = None  # the session to end; omitted = current-by-token


class SessionPublic(BaseModel):
    id: str
    ip: str | None = None
    user_agent: str | None = None
    created_at: datetime
    last_used_at: datetime
    expires_at: datetime
    current: bool = False


class UserPublic(BaseModel):
    id: str
    email: EmailStr
    display_name: str | None = None
    is_active: bool
    is_verified: bool
    is_superuser: bool = False
    roles: list[str]
    created_at: datetime
    last_login_at: datetime | None = None
