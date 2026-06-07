from datetime import timedelta

from beanie import PydanticObjectId
from beanie.operators import Set
from fastapi import APIRouter, Depends, Query

from app.admin.schemas import (
    ActivityOverview,
    AdminTokenView,
    AdminUsageEvent,
    AdminUserSummary,
    InterestItemAddRequest,
    InterestItemAdminView,
    InterestItemBulkReplaceRequest,
    InterestItemBulkReplaceResponse,
    InterestItemListAdmin,
    TopUser,
)
from app.auth.models import User
from app.core.dependencies import get_current_superuser
from app.core.errors import APIError, ErrorCode
from app.core.pagination import Page, paginate_newest_first
from app.core.scopes import decode
from app.core.utils import utcnow
from app.tokens.models import ApiToken
from app.trove.market import service as market_service
from app.trove.market.models import MarketInterestItem
from app.usage.models import UsageEvent
from app.usage.schemas import ActivitySummary
from app.usage.service import ERROR_COND, RATE_LIMITED_COND, aggregate_activity

router = APIRouter(
    prefix="/admin",
    tags=["admin"],
    dependencies=[Depends(get_current_superuser)],
)


def _token_view(token: ApiToken) -> AdminTokenView:
    return AdminTokenView(
        id=str(token.id),
        user_id=str(token.user_id),
        name=token.name,
        prefix=token.prefix,
        scopes=token.scopes,
        scope_names=decode(token.scopes),
        allowed_ip_count=len(token.allowed_ip_hashes),
        revoked=token.revoked,
        revoked_at=token.revoked_at,
        revoke_reason=token.revoke_reason,
        created_at=token.created_at,
        last_used_at=token.last_used_at,
        rotated_at=token.rotated_at,
        expires_at=token.expires_at,
        request_count=token.request_count,
    )


async def _require_user(user_id: PydanticObjectId) -> User:
    user = await User.get(user_id)
    if user is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="User not found")
    return user


# --- Users -----------------------------------------------------------------

@router.get("/users", response_model=list[AdminUserSummary])
async def list_users(
    days: int = Query(default=30, ge=1, le=365),
) -> list[AdminUserSummary]:
    # Roll up token stats per user (lifetime).
    rows = await ApiToken.aggregate(
        [
            {
                "$group": {
                    "_id": "$user_id",
                    "token_count": {"$sum": 1},
                    "total_requests": {"$sum": "$request_count"},
                    "last_used_at": {"$max": "$last_used_at"},
                }
            }
        ]
    ).to_list()
    stats = {str(r["_id"]): r for r in rows}

    # Rate-limit triggers (429s) per user, within the metrics window.
    since = utcnow() - timedelta(days=days)
    rl_rows = await UsageEvent.aggregate(
        [
            {"$match": {"created_at": {"$gte": since}, "status_code": 429}},
            {"$group": {"_id": "$user_id", "rate_limited": {"$sum": 1}}},
        ]
    ).to_list()
    rl = {str(r["_id"]): r["rate_limited"] for r in rl_rows}

    users = await User.find_all().to_list()
    summaries = []
    for u in users:
        s = stats.get(str(u.id))
        summaries.append(
            AdminUserSummary(
                id=str(u.id),
                email=u.email,
                display_name=u.display_name,
                is_active=u.is_active,
                is_verified=u.is_verified,
                is_superuser=u.is_superuser,
                created_at=u.created_at,
                last_login_at=u.last_login_at,
                token_count=s["token_count"] if s else 0,
                total_requests=s["total_requests"] if s else 0,
                last_used_at=s["last_used_at"] if s else None,
                rate_limited=rl.get(str(u.id), 0),
            )
        )
    return summaries


@router.get("/users/{user_id}/tokens", response_model=list[AdminTokenView])
async def list_user_tokens(user_id: PydanticObjectId) -> list[AdminTokenView]:
    await _require_user(user_id)
    tokens = await ApiToken.find(ApiToken.user_id == user_id).to_list()
    return [_token_view(t) for t in tokens]


@router.delete("/users/{user_id}/tokens")
async def revoke_user_tokens(user_id: PydanticObjectId) -> dict[str, int]:
    """Revoke every active token belonging to a user."""
    await _require_user(user_id)
    active = ApiToken.find(ApiToken.user_id == user_id, ApiToken.revoked == False)  # noqa: E712
    count = await active.count()
    if count:
        await active.update(Set({  # pyright: ignore[reportGeneralTypeIssues]
            ApiToken.revoked: True,
            ApiToken.revoked_at: utcnow(),
            ApiToken.revoke_reason: "Revoked by administrator",
        }))
    return {"revoked": count}


@router.get("/users/{user_id}/activity", response_model=ActivitySummary)
async def user_activity(
    user_id: PydanticObjectId,
    days: int = Query(default=7, ge=1, le=365),
) -> ActivitySummary:
    await _require_user(user_id)
    return await aggregate_activity({"user_id": user_id}, days)


@router.get("/users/{user_id}", response_model=AdminUserSummary)
async def get_user(user_id: PydanticObjectId) -> AdminUserSummary:
    user = await _require_user(user_id)
    tokens = await ApiToken.find(ApiToken.user_id == user_id).to_list()
    last_used = [t.last_used_at for t in tokens if t.last_used_at is not None]
    return AdminUserSummary(
        id=str(user.id),
        email=user.email,
        display_name=user.display_name,
        is_active=user.is_active,
        is_verified=user.is_verified,
        is_superuser=user.is_superuser,
        created_at=user.created_at,
        last_login_at=user.last_login_at,
        token_count=len(tokens),
        total_requests=sum(t.request_count for t in tokens),
        last_used_at=max(last_used) if last_used else None,
    )


# --- Tokens ----------------------------------------------------------------

@router.post("/tokens/{token_id}/revoke", response_model=AdminTokenView)
async def revoke_token(token_id: PydanticObjectId) -> AdminTokenView:
    token = await ApiToken.get(token_id)
    if token is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Token not found")
    if not token.revoked:
        token.revoked = True
        token.revoked_at = utcnow()
        token.revoke_reason = "Revoked by administrator"
        await token.save()
    return _token_view(token)


# --- Raw event feed (cursor-paginated) -------------------------------------

@router.get("/events", response_model=Page[AdminUsageEvent])
async def list_events(
    user_id: PydanticObjectId | None = Query(default=None),
    status_code: int | None = Query(default=None, ge=100, le=599),
    cursor: str | None = Query(default=None, description="next_cursor from a prior page"),
    limit: int = Query(default=50, ge=1, le=200),
) -> Page[AdminUsageEvent]:
    """Newest-first feed of raw usage events, keyset-paginated by event id.

    Optionally filter by user and/or HTTP status (e.g. status_code=429 to audit
    rate-limit triggers).
    """
    base: dict = {}
    if user_id is not None:
        base["user_id"] = user_id
    if status_code is not None:
        base["status_code"] = status_code

    docs, next_cursor, has_more = await paginate_newest_first(
        UsageEvent, base, cursor, limit
    )
    return Page[AdminUsageEvent](
        items=[
            AdminUsageEvent(
                id=str(e.id),
                user_id=str(e.user_id),
                token_id=str(e.token_id),
                method=e.method,
                route=e.route,
                path=e.path,
                status_code=e.status_code,
                duration_ms=e.duration_ms,
                created_at=e.created_at,
            )
            for e in docs
        ],
        next_cursor=next_cursor,
        has_more=has_more,
    )


# --- Global activity -------------------------------------------------------

@router.get("/activity", response_model=ActivityOverview)
async def activity_overview(
    days: int = Query(default=7, ge=1, le=365),
    top: int = Query(default=10, ge=1, le=100),
) -> ActivityOverview:
    since = utcnow() - timedelta(days=days)
    match = {"created_at": {"$gte": since}}

    totals = await UsageEvent.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": None,
                    "count": {"$sum": 1},
                    "error_count": {"$sum": ERROR_COND},
                    "rate_limited": {"$sum": RATE_LIMITED_COND},
                    "avg_duration_ms": {"$avg": "$duration_ms"},
                }
            },
        ]
    ).to_list()

    top_rows = await UsageEvent.aggregate(
        [
            {"$match": match},
            {
                "$group": {
                    "_id": "$user_id",
                    "count": {"$sum": 1},
                    "error_count": {"$sum": ERROR_COND},
                    "rate_limited": {"$sum": RATE_LIMITED_COND},
                }
            },
            {"$sort": {"count": -1}},
            {"$limit": top},
        ]
    ).to_list()

    # Resolve emails for the top users in one query.
    ids = [r["_id"] for r in top_rows]
    emails = {
        u.id: u.email for u in await User.find({"_id": {"$in": ids}}).to_list()
    } if ids else {}

    t = totals[0] if totals else {"count": 0, "error_count": 0, "rate_limited": 0, "avg_duration_ms": 0}
    return ActivityOverview(
        window_days=days,
        total_requests=t["count"],
        error_count=t["error_count"],
        rate_limited=t.get("rate_limited", 0),
        avg_duration_ms=round(t["avg_duration_ms"] or 0, 2),
        top_users=[
            TopUser(
                user_id=str(r["_id"]),
                email=emails.get(r["_id"]),
                count=r["count"],
                error_count=r["error_count"],
                rate_limited=r.get("rate_limited", 0),
            )
            for r in top_rows
        ],
    )


# --- Market interest-items (admin) -----------------------------------------
# The bot scrapes the in-game marketplace; only items on THIS list are
# persisted (everything else is dropped at ingest). Editable from the master
# panel so the scrape footprint can change without a redeploy. The list is
# served tokenless via /v1/misc/interest-items.


def _item_view(d: MarketInterestItem) -> InterestItemAdminView:
    return InterestItemAdminView(
        name=d.name,
        added_by=str(d.added_by) if d.added_by is not None else None,
        added_at=d.added_at,
    )


@router.get("/market/interest-items", response_model=InterestItemListAdmin)
async def list_interest_items_admin() -> InterestItemListAdmin:
    """Every interest item with admin metadata (who added it, when)."""
    docs = await market_service.admin_list_interest_items()
    return InterestItemListAdmin(
        items=[_item_view(d) for d in docs],
        count=len(docs),
    )


@router.post("/market/interest-items", response_model=InterestItemAdminView,
             status_code=201)
async def add_interest_item_admin(
    req: InterestItemAddRequest,
    admin: User = Depends(get_current_superuser),
) -> InterestItemAdminView:
    """Add one item to the bot's scan allow-list."""
    try:
        doc = await market_service.admin_add_interest_item(
            req.name, added_by=admin.id,
        )
    except ValueError as e:
        # Empty name → 400; dup → 409 (let the message distinguish).
        msg = str(e)
        if "already exists" in msg:
            raise APIError(status_code=409, code=ErrorCode.conflict, message=msg)
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=msg)
    return _item_view(doc)


@router.delete("/market/interest-items/{name}", status_code=204)
async def remove_interest_item_admin(name: str) -> None:
    """Drop one item from the allow-list."""
    removed = await market_service.admin_remove_interest_item(name)
    if not removed:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message=f"No interest item named '{name}'")


@router.put("/market/interest-items", response_model=InterestItemBulkReplaceResponse)
async def replace_interest_items_admin(
    req: InterestItemBulkReplaceRequest,
    admin: User = Depends(get_current_superuser),
) -> InterestItemBulkReplaceResponse:
    """Atomic-ish bulk replace: drop everything stored, then insert the new
    list (de-duped, trimmed, sorted server-side). Refuses an empty list — to
    delete every item, use the per-item DELETE endpoint."""
    try:
        summary = await market_service.admin_replace_interest_items(
            req.items, added_by=admin.id,
        )
    except ValueError as e:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
    return InterestItemBulkReplaceResponse(**summary)
