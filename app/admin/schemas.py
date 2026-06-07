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
    # IPs are hashed in the DB — even an admin can't read them back. The count
    # is all we expose. (0 = no IP restriction.)
    allowed_ip_count: int
    revoked: bool
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    created_at: datetime
    last_used_at: datetime | None = None
    # last_used_ip was removed — see app/tokens/models.py.
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


# --- Market interest-items (admin) -----------------------------------------


class InterestItemAdminView(BaseModel):
    """One interest-item row with admin metadata.

    ``added_by`` is null for items inserted by the boot-time seeder; otherwise
    it's the User id of the admin who added it via the admin endpoint."""

    name: str
    added_by: str | None = None
    added_at: datetime


class InterestItemListAdmin(BaseModel):
    items: list[InterestItemAdminView]
    count: int


class InterestItemAddRequest(BaseModel):
    name: str


class InterestItemBulkReplaceRequest(BaseModel):
    items: list[str]


class InterestItemBulkReplaceResponse(BaseModel):
    removed: int   # rows deleted before insert
    added: int     # rows inserted (after de-dup + trim)
