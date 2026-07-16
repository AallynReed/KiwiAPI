from datetime import datetime

from pydantic import BaseModel, EmailStr, Field


class StrayAssignRequest(BaseModel):
    """Assign one or more stray mods to a site user by their database id."""
    user_id: str = Field(min_length=1, description="Target SiteUser database id.")
    project_ids: list[str] = Field(min_length=1, description="Stray mod project ids to assign.")


class SiteClaimedNameRequest(BaseModel):
    """Admin override of a site user's claimed Trove name (empty clears it)."""
    name: str = Field(default="", max_length=64)


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
    # IPs are hashed in the DB - even an admin can't read them back. The count
    # is all we expose. (0 = no IP restriction.)
    allowed_ip_count: int
    revoked: bool
    revoked_at: datetime | None = None
    revoke_reason: str | None = None
    created_at: datetime
    last_used_at: datetime | None = None
    # last_used_ip was removed - see app/tokens/models.py.
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
    removed: int
    added: int  # after de-dup + trim


# --- Market item categories (admin) -----------------------------------------
# Sidebar groupings for the /market page. Membership is stored as item NAMES
# on the category (NOT on the interest-item docs), so allow-list deletes and
# the bulk replace never uncategorize anything.


class MarketCategoryAdminView(BaseModel):
    id: str
    name: str
    order: int
    items: list[str]
    created_at: datetime


class MarketCategoryListAdmin(BaseModel):
    categories: list[MarketCategoryAdminView]
    count: int


class MarketCategoryCreateRequest(BaseModel):
    name: str


class MarketCategoryUpdateRequest(BaseModel):
    """Partial update: omitted fields stay untouched."""

    name: str | None = None
    items: list[str] | None = None


class MarketCategoryReorderRequest(BaseModel):
    """Category ids in the desired display order (first = top). Ids not
    listed keep their relative order after the listed ones."""

    ids: list[str]


class MarketCategoryReorderResponse(BaseModel):
    reordered: int  # docs whose order actually changed


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
    # One of "daily", "weekly", "default" (hardcoded fallback when uuid is in
    # neither hardcoded set), or "none" (admin-pinned).
    effective_reset_kind: str
    reset_kind_override: str | None = None
    # True for daily/weekly. Boards without a periodic reset run velocity-only
    # cheater detection (score-outlier + rank-gap skipped); the portal
    # colour-codes rows on this.
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
# have to know what's available - it just renders what the server reports).


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


class SiteClaimAdminView(BaseModel):
    """One Trove-name claim, for the master approval queue."""
    user_id: str
    username: str
    display_name: str | None = None
    discord_id: int | None = None
    claimed_trove_name: str | None = None
    claimed_trove_display: str | None = None
    claimed_at: datetime | None = None
    claim_verified: bool = False
    claim_verified_at: datetime | None = None


class SiteClaimAdminList(BaseModel):
    items: list[SiteClaimAdminView]
    count: int
