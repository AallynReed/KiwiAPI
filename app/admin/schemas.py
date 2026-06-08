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


# --- Leaderboard per-board admin (reset-kind override) ---------------------
# The admin panel lists every captured board so the master can pin its
# reset cadence (daily / weekly / none). The hardcoded set in
# app/trove/leaderboards/models.py is the fallback when no override has
# been set. Editing here also matters for cheater detection: lifetime
# boards skip score-outlier + rank-gap and only use velocity.

class LeaderboardBoardAdminView(BaseModel):
    uuid: int
    name: str
    name_id: str
    category: str
    category_id: str
    # The cadence currently in effect for this board. One of "daily",
    # "weekly", "default" (hardcoded fallback when uuid isn't in either
    # hardcoded set), or "none" (admin pinned).
    effective_reset_kind: str
    # The admin override if set, else None.
    reset_kind_override: str | None = None
    # ``True`` for daily / weekly. The portal uses this to colour-code
    # rows so the admin can see at a glance which boards will run all
    # three cheater checks vs. velocity-only.
    has_periodic_reset: bool


class LeaderboardBoardAdminList(BaseModel):
    items: list[LeaderboardBoardAdminView]
    count: int


class LeaderboardBoardResetUpdate(BaseModel):
    """Body for ``PATCH /admin/leaderboards/boards/{uuid}``.

    ``reset_kind_override`` is one of ``"daily"`` / ``"weekly"`` /
    ``"none"`` to pin the cadence, or ``None`` to clear the override
    and fall back to the hardcoded mapping.
    """
    reset_kind_override: str | None = None


# --- Runtime configuration -------------------------------------------------
# Sparse Mongo overrides on top of declared registry defaults. The list
# endpoint always returns every REGISTERED key (so the admin UI doesn't
# have to know what's available — it just renders what the server reports).


class RuntimeConfigItem(BaseModel):
    """One known tunable + its currently effective value."""
    key: str                          # e.g. "feedback.discord_webhook"
    category: str                     # "feedback" | "rate_limits" | ...
    type: str                         # "str" | "int" | "bool" | "float"
    description: str
    secret: bool                      # UI masks the value by default
    default: object                   # code default
    value: object                     # effective value (override or default)
    is_default: bool                  # True when no override exists
    min_value: int | float | None = None
    max_value: int | float | None = None
    choices: list[str] | None = None
    updated_at: datetime | None = None
    updated_by_user_id: str | None = None


class RuntimeConfigList(BaseModel):
    items: list[RuntimeConfigItem]
    count: int


class RuntimeConfigUpdate(BaseModel):
    """Body for ``PUT /admin/config/{key}``. Type-checked + range-checked
    server-side against the registry spec for that key."""
    value: object
