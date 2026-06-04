from datetime import datetime

from pydantic import BaseModel, EmailStr


class AdminUserSummary(BaseModel):
    id: str
    email: EmailStr
    display_name: str | None = None
    is_active: bool
    is_verified: bool
    is_superuser: bool
    created_at: datetime
    last_login_at: datetime | None = None
    # Aggregated across the user's API tokens.
    token_count: int
    total_requests: int
    last_used_at: datetime | None = None
    rate_limited: int = 0  # 429s the user has triggered (in the metrics window)


class AdminTokenView(BaseModel):
    id: str
    user_id: str
    name: str
    prefix: str
    scopes: int
    scope_names: list[str]
    allowed_ips: list[str]
    revoked: bool
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    created_at: datetime
    last_used_at: datetime | None = None
    last_used_ip: str | None = None
    rotated_at: datetime | None = None
    expires_at: datetime | None = None
    request_count: int


class AdminUsageEvent(BaseModel):
    id: str
    user_id: str
    token_id: str
    method: str
    route: str
    path: str
    status_code: int
    duration_ms: float
    created_at: datetime


class TopUser(BaseModel):
    user_id: str
    email: EmailStr | None = None
    count: int
    error_count: int
    rate_limited: int = 0


class ActivityOverview(BaseModel):
    window_days: int
    total_requests: int
    error_count: int
    rate_limited: int
    avg_duration_ms: float
    top_users: list[TopUser]
