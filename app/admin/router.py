from datetime import timedelta

from beanie import PydanticObjectId
from beanie.operators import Set
from fastapi import APIRouter, BackgroundTasks, Depends, Query

from app.admin import ingest_log, runtime_config
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
    LeaderboardBoardAdminList,
    LeaderboardBoardAdminView,
    LeaderboardBoardResetUpdate,
    MarketCategoryAdminView,
    MarketCategoryCreateRequest,
    MarketCategoryListAdmin,
    MarketCategoryReorderRequest,
    MarketCategoryReorderResponse,
    MarketCategoryUpdateRequest,
    RuntimeConfigItem,
    RuntimeConfigList,
    RuntimeConfigUpdate,
    SiteClaimAdminList,
    SiteClaimAdminView,
    SiteClaimedNameRequest,
    StrayAssignRequest,
    TopUser,
)
from app.auth.models import User
from app.core.config import settings
from app.core.dependencies import get_current_superuser
from app.core.errors import APIError, ErrorCode, raise_from_value_error
from app.core.pagination import Page, paginate_newest_first
from app.core.scopes import decode
from app.core.utils import iso, to_oid, utcnow
from app.pageviews.schemas import PageviewSummary
from app.pageviews.service import aggregate_pageviews
from app.site_auth.models import SiteUser
from app.site_auth.schemas import SiteUsernameRequestBody
from app.site_auth.usernames import _load_site_user_or_404
from app.supporters import service as supporters_service
from app.supporters.schemas import (
    SupporterAddRequest,
    SupporterAdminList,
    SupporterAdminView,
    SupporterBulkReplaceRequest,
    SupporterBulkReplaceResponse,
)
from app.tokens.models import ApiToken
from app.trove.market import service as market_service
from app.trove.market.models import MarketInterestItem, MarketItemCategory
from app.trove.mods_hub.schemas import TakedownRequest
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


@router.get("/pageviews", response_model=PageviewSummary)
async def site_pageviews(
    days: int = Query(default=30, ge=1, le=365),
    top: int = Query(default=100, ge=1, le=1000),
) -> PageviewSummary:
    """Showcase-site page-view + unique-visitor rollup for the Site Analytics tab.

    Per-page views and unique visitors over the last ``days`` days, one row per
    real page URL (each mod / player page individually), capped to the ``top`` pages
    by views. Unique visitors are counted once per UTC day (cookieless salted-hash
    dedupe); static assets and the JSON proxies aren't counted.
    """
    return await aggregate_pageviews(days, top)


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
        raise_from_value_error(e)  # dup → 409, empty name → 400
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
    list (de-duped, trimmed, sorted server-side). Refuses an empty list - to
    delete every item, use the per-item DELETE endpoint."""
    try:
        summary = await market_service.admin_replace_interest_items(
            req.items, added_by=admin.id,
        )
    except ValueError as e:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
    return InterestItemBulkReplaceResponse(**summary)


# --- Market item categories (admin) -----------------------------------------
# Sidebar groupings for the /market page. Membership = item NAMES stored on
# the category doc, decoupled from MarketInterestItem so allow-list deletes /
# bulk replaces never lose assignments (re-adding an item puts it straight
# back in its category). Rendered publicly via /site/market/items.


def _category_view(d: MarketItemCategory) -> MarketCategoryAdminView:
    return MarketCategoryAdminView(
        id=str(d.id),
        name=d.name,
        order=d.order,
        items=sorted(set(d.items), key=str.lower),
        created_at=d.created_at,
    )


@router.get("/market/categories", response_model=MarketCategoryListAdmin)
async def list_market_categories_admin() -> MarketCategoryListAdmin:
    """Every category in display order, with its member-item names."""
    docs = await market_service.list_categories()
    return MarketCategoryListAdmin(
        categories=[_category_view(d) for d in docs],
        count=len(docs),
    )


@router.post("/market/categories", response_model=MarketCategoryAdminView,
             status_code=201)
async def create_market_category_admin(
    req: MarketCategoryCreateRequest,
    admin: User = Depends(get_current_superuser),
) -> MarketCategoryAdminView:
    """Create an empty category at the bottom of the display order."""
    try:
        doc = await market_service.admin_create_category(
            req.name, created_by=admin.id,
        )
    except ValueError as e:
        raise_from_value_error(e)  # dup → 409, empty name → 400
    return _category_view(doc)


@router.patch("/market/categories/{category_id}",
              response_model=MarketCategoryAdminView)
async def update_market_category_admin(
    category_id: str,
    req: MarketCategoryUpdateRequest,
) -> MarketCategoryAdminView:
    """Rename and/or replace the member-item list (omitted fields untouched).
    Item names are free-form on purpose - they may reference items not
    currently on the allow-list (kept so they survive allow-list churn)."""
    try:
        doc = await market_service.admin_update_category(
            to_oid(category_id), name=req.name, items=req.items,
        )
    except ValueError as e:
        raise_from_value_error(e)
    if doc is None:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message="No such category")
    return _category_view(doc)


@router.delete("/market/categories/{category_id}", status_code=204)
async def delete_market_category_admin(category_id: str) -> None:
    """Delete one category - its items just become uncategorized."""
    removed = await market_service.admin_delete_category(to_oid(category_id))
    if not removed:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message="No such category")


@router.put("/market/categories/order",
            response_model=MarketCategoryReorderResponse)
async def reorder_market_categories_admin(
    req: MarketCategoryReorderRequest,
) -> MarketCategoryReorderResponse:
    """Persist a new display order (ids first-to-last = top-to-bottom).
    Unlisted ids keep their relative order after the listed ones."""
    touched = await market_service.admin_reorder_categories(
        [to_oid(i) for i in req.ids],
    )
    return MarketCategoryReorderResponse(reordered=touched)


# --- Leaderboards: per-board reset cadence override -----------------------
# The hardcoded ``_DAILY_RESET_UUIDS`` / ``_WEEKLY_RESET_UUIDS`` sets in
# ``app/trove/leaderboards/models.py`` are the fallback when an admin
# hasn't pinned a board. These two endpoints expose the per-doc override
# so the master can flip cadence from the portal without a code push -
# important because cheater detection on lifetime boards needs to skip
# score-outlier + rank-gap and rely on velocity alone.


@router.get(
    "/leaderboards/boards", response_model=LeaderboardBoardAdminList,
)
async def list_leaderboards_admin_boards() -> LeaderboardBoardAdminList:
    """Every captured leaderboard, oldest → newest by category + name,
    with its current effective reset cadence and any admin override."""
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards.models import (
        RESET_KIND_VALUES,
        is_lifetime_kind,
        reset_kind,
    )
    docs = await pg_store.admin_list_boards()
    items: list[LeaderboardBoardAdminView] = []
    for d in docs:
        ov = d["reset_kind_override"]
        eff = ov if (isinstance(ov, str) and ov in RESET_KIND_VALUES) else reset_kind(d["uuid"])
        items.append(LeaderboardBoardAdminView(
            uuid=d["uuid"],
            name=d["name"],
            name_id=d["name_id"],
            category=d["category"],
            category_id=d["category_id"],
            effective_reset_kind=eff,
            reset_kind_override=ov,
            has_periodic_reset=not is_lifetime_kind(eff),
        ))
    # Sort by category then name so the table reads naturally and
    # similar boards group together. Stable secondary sort on uuid so
    # boards sharing a name don't shuffle between calls.
    items.sort(key=lambda b: (b.category, b.name, b.uuid))
    return LeaderboardBoardAdminList(items=items, count=len(items))


@router.patch(
    "/leaderboards/boards/{uuid}", response_model=LeaderboardBoardAdminView,
)
async def set_leaderboard_board_reset_kind(
    uuid: int,
    payload: LeaderboardBoardResetUpdate,
) -> LeaderboardBoardAdminView:
    """Set or clear the reset cadence override for a single board.

    Pass ``"daily"`` / ``"weekly"`` / ``"none"`` to pin; pass ``null`` (or
    omit the field) to clear and fall back to the hardcoded mapping.
    Invalidates the cheaters cache so the next visitor sees a result
    that reflects the new gating; the warmer's wake-event is also fired
    so the new value lands in the cached payload within seconds, not
    after the next TTL boundary."""
    from app.trove.leaderboards import detection as lb_detection
    from app.trove.leaderboards import pg_store
    from app.trove.leaderboards.models import (
        RESET_KIND_VALUES,
        is_lifetime_kind,
        reset_kind,
    )

    new_value = payload.reset_kind_override
    if new_value is not None and new_value not in RESET_KIND_VALUES:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=(
                f"reset_kind_override must be one of "
                f"{', '.join(RESET_KIND_VALUES)} or null; got {new_value!r}"
            ),
        )

    d = await pg_store.set_reset_kind_override(uuid, new_value)
    if d is None:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"No leaderboard with uuid={uuid}",
        )

    # Detection's per-anchor cache keys don't include the per-board reset_kind,
    # so a stale payload would survive the flip. Nuke the cache and wake the
    # warmer to backfill so the next visitor sees accurate flags.
    lb_detection.invalidate_cache()
    lb_detection.trigger_warmer()

    ov = d["reset_kind_override"]
    eff = ov if (isinstance(ov, str) and ov in RESET_KIND_VALUES) else reset_kind(uuid)
    return LeaderboardBoardAdminView(
        uuid=d["uuid"],
        name=d["name"],
        name_id=d["name_id"],
        category=d["category"],
        category_id=d["category_id"],
        effective_reset_kind=eff,
        reset_kind_override=ov,
        has_periodic_reset=not is_lifetime_kind(eff),
    )


# --- Leaderboards: rebuild the per-player board aggregate ------------------
# player_board_agg is maintained incrementally at ingest, but an existing
# dataset (pre-feature) needs a one-time full seed, and out-of-order backfills
# can undercount appearances. This runs the full recompute in the background
# (a big all-history scan) and reports status for the poll below. Master-only
# via the router-level dep.
_agg_rebuild_status: dict = {
    "running": False, "rows": None, "finished_at": None, "error": None,
}


async def _run_agg_rebuild() -> None:
    import logging
    import time as _time

    from app.trove.leaderboards import pg_store
    _agg_rebuild_status.update(running=True, error=None)
    try:
        rows = await pg_store.rebuild_player_board_agg()
        _agg_rebuild_status.update(rows=rows)
    except Exception as exc:  # noqa: BLE001 - surface via status, never 500 a bg task
        _agg_rebuild_status.update(error=str(exc))
        logging.getLogger(__name__).exception("player_board_agg rebuild failed")
    finally:
        _agg_rebuild_status.update(running=False, finished_at=int(_time.time()))


@router.post("/leaderboards/rebuild-player-agg")
async def rebuild_player_board_agg(background_tasks: BackgroundTasks) -> dict:
    """Full recompute of the per-player board aggregate (the /player profile's
    fast read). Seeds the table on an existing dataset and trues up appearance
    counts after backfills. Runs in the background (a full all-history scan);
    poll ``/admin/leaderboards/rebuild-player-agg/status``."""
    if not settings.postgres_enabled:
        raise APIError(status_code=400, code=ErrorCode.bad_request,
                       message="Postgres backend is disabled")
    if _agg_rebuild_status["running"]:
        return {"started": False, "message": "A rebuild is already running."}
    background_tasks.add_task(_run_agg_rebuild)
    return {"started": True,
            "message": "Player aggregate rebuild started - poll "
                       "/admin/leaderboards/rebuild-player-agg/status."}


@router.get("/leaderboards/rebuild-player-agg/status")
async def rebuild_player_board_agg_status() -> dict:
    """Current state of the last player-aggregate rebuild (running / row count)."""
    return _agg_rebuild_status


# --- Leaderboards: cold-tier aged partitions --------------------------------
# Move entry partitions past the physical retention window (leaderboards_pg_tier_
# after_days) off the fast NVMe onto the slower `cold` tablespace. The warmer
# also drips this automatically (a few partitions/day); this is the manual "drain
# the whole backlog now" trigger (the first run can relocate tens of GB). Safe to
# re-run + a no-op when the cold tablespace isn't provisioned. Master-only.
_cold_tier_status: dict = {
    "running": False, "moved": None, "moved_bytes": None,
    "finished_at": None, "error": None,
}


async def _run_cold_tier() -> None:
    import logging
    import time as _time

    from app.admin import runtime_config
    from app.trove.leaderboards import pg_store
    _cold_tier_status.update(running=True, error=None)
    try:
        after_days = int(await runtime_config.get_setting("leaderboards_pg_tier_after_days"))
        res = await pg_store.tier_cold_partitions(after_days, int(_time.time()), limit=None)
        _cold_tier_status.update(moved=res.get("moved") or [],
                                 moved_bytes=res.get("moved_bytes") or 0)
    except Exception as exc:  # noqa: BLE001 - surface via status, never 500 a bg task
        _cold_tier_status.update(error=str(exc))
        logging.getLogger(__name__).exception("cold-tier move failed")
    finally:
        _cold_tier_status.update(running=False, finished_at=int(_time.time()))


@router.post("/leaderboards/tier-cold")
async def tier_cold_partitions(background_tasks: BackgroundTasks) -> dict:
    """Move every entry partition aged past leaderboards_pg_tier_after_days onto
    the cold tablespace (table + indexes), freeing NVMe. Runs in the background
    (the first run can relocate tens of GB); poll .../tier-cold/status."""
    if not settings.postgres_enabled:
        raise APIError(status_code=400, code=ErrorCode.bad_request,
                       message="Postgres backend is disabled")
    if _cold_tier_status["running"]:
        return {"started": False, "message": "A cold-tier move is already running."}
    background_tasks.add_task(_run_cold_tier)
    return {"started": True,
            "message": "Cold-tier move started - poll /admin/leaderboards/tier-cold/status."}


@router.get("/leaderboards/tier-cold/status")
async def tier_cold_partitions_status() -> dict:
    """Cold-tier layout (hot/cold partition counts + bytes, eligible backlog) plus
    the state of the last manual move."""
    out: dict = {"last_run": _cold_tier_status}
    if settings.postgres_enabled:
        import time as _time

        from app.admin import runtime_config
        from app.trove.leaderboards import pg_store
        try:
            after_days = int(await runtime_config.get_setting("leaderboards_pg_tier_after_days"))
            out["layout"] = await pg_store.tier_status(after_days, int(_time.time()))
        except Exception:  # noqa: BLE001 - report, don't 500
            import logging
            logging.getLogger(__name__).exception("tier_status failed")
            out["layout"] = {"error": "tier status unavailable"}
    return out


# --- Codexes: force a parser rebuild ---------------------------------------
# The steady-state indexer only re-touches changed game files, so a parser-code
# change doesn't reach existing rows until a game update. This forces a full
# re-parse of a branch with the current parser (UPSERT in place - no empty
# window). Master-only via the router-level dep.

_CODEX_BRANCHES = ("live-us", "pts")


@router.post("/codexes/rebuild")
async def rebuild_codexes(
    background_tasks: BackgroundTasks,
    branch: str = Query(default="live-us", description="live-us | pts"),
) -> dict:
    """Force a full codex re-parse for a branch with the CURRENT parser. Runs in the
    background (a full build is minutes); poll ``/admin/codexes/status``."""
    if branch not in _CODEX_BRANCHES:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=f"Unknown branch '{branch}' (known: {', '.join(_CODEX_BRANCHES)})",
        )
    if not settings.postgres_enabled:
        raise APIError(status_code=400, code=ErrorCode.bad_request,
                       message="Postgres backend is disabled")
    from app.trove.codexes import indexer
    from app.trove.updates.cas import ContentStore
    if indexer.get_rebuild_status(branch).get("running"):
        return {"started": False, "branch": branch, "message": "A rebuild is already running."}
    store = ContentStore(settings.trove_update_store_dir)
    background_tasks.add_task(indexer.rebuild, branch, store)
    return {"started": True, "branch": branch,
            "message": "Codex rebuild started - poll /admin/codexes/status."}


@router.get("/codexes/status")
async def codex_status(
    branch: str = Query(default="live-us", description="live-us | pts"),
) -> dict:
    """Current entry count + last manual-rebuild status for a branch."""
    if branch not in _CODEX_BRANCHES:
        raise APIError(
            status_code=400, code=ErrorCode.not_found,
            message=f"Unknown branch '{branch}'",
        )
    from app.trove.codexes import indexer, pg_store
    count = await pg_store.branch_count(branch) if settings.postgres_enabled else 0
    version = await pg_store.get_parser_version(branch) if settings.postgres_enabled else 0
    return {
        "branch": branch, "entry_count": count,
        "parser_version": version, "current_parser_version": indexer.CODEX_PARSER_VERSION,
        "rebuild": indexer.get_rebuild_status(branch),
    }


# --- Updates: backfill the "last modified" index ---------------------------
# The archive's per-file `last_ordinal` (which version last touched a file) is set
# by ingest going forward; files that predate the field read 0 until this recompute
# runs once per branch. Pure recompute from the change-log - safe to re-run.

_UPDATE_BRANCHES = ("live-us", "pts")


@router.post("/updates/backfill-modified")
async def backfill_updates_modified(
    background_tasks: BackgroundTasks,
    branch: str = Query(default="live-us", description="live-us | pts"),
) -> dict:
    """Recompute every file's last-modified version for a branch from the change-log,
    so the /updates "Last modified" sort is accurate for pre-existing files. Runs in
    the background (minutes on a large tree); poll ``/admin/updates/backfill-modified/status``."""
    if branch not in _UPDATE_BRANCHES:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request,
            message=f"Unknown branch '{branch}' (known: {', '.join(_UPDATE_BRANCHES)})",
        )
    from app.trove.updates import maintenance
    if maintenance.get_backfill_status(branch).get("running"):
        return {"started": False, "branch": branch, "message": "A backfill is already running."}
    background_tasks.add_task(maintenance.backfill_last_ordinal, branch)
    return {"started": True, "branch": branch,
            "message": "Backfill started - poll /admin/updates/backfill-modified/status."}


@router.get("/updates/backfill-modified/status")
async def backfill_updates_modified_status(
    branch: str = Query(default="live-us", description="live-us | pts"),
) -> dict:
    """Progress of the last-modified backfill for a branch."""
    if branch not in _UPDATE_BRANCHES:
        raise APIError(status_code=400, code=ErrorCode.not_found, message=f"Unknown branch '{branch}'")
    from app.trove.updates import maintenance
    return {"branch": branch, "backfill": maintenance.get_backfill_status(branch)}


# --- Runtime configuration -------------------------------------------------
# Master-only knobs that take effect immediately (5-second cache invalidation
# on the read side). Surfaces ALL registered settings, even unchanged ones,
# so the admin UI can render the full landscape from one call.


@router.get("/config", response_model=RuntimeConfigList)
async def list_runtime_config(
    admin: User = Depends(get_current_superuser),
) -> RuntimeConfigList:
    """Every known runtime tunable + its currently effective value.

    Items that haven't been overridden show ``is_default: true`` and
    ``updated_at: null``. Items the master has changed carry the override
    value, when it was set, and which superuser set it.
    """
    items = await runtime_config.list_all()
    return RuntimeConfigList(
        items=[RuntimeConfigItem(**item) for item in items],
        count=len(items),
    )


@router.put("/config/{key}", response_model=RuntimeConfigItem)
async def update_runtime_config(
    key: str,
    req: RuntimeConfigUpdate,
    admin: User = Depends(get_current_superuser),
) -> RuntimeConfigItem:
    """Set or replace the override for one tunable.

    The value is type-checked + range-checked against the registry
    declaration before persisting; a wrong type or out-of-range value
    returns 400 with the specific reason. Cache for this key is
    invalidated so the next read picks up the change within milliseconds.
    """
    try:
        await runtime_config.set_setting(key, req.value, updated_by=admin.id)
    except runtime_config.UnknownSettingError:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"Unknown setting key '{key}'.",
        )
    except runtime_config.InvalidSettingError as e:
        raise APIError(
            status_code=400, code=ErrorCode.bad_request, message=str(e),
        )
    # Echo back the post-update view so the UI can refresh inline without
    # a follow-up list call.
    items = await runtime_config.list_all()
    for item in items:
        if item["key"] == key:
            return RuntimeConfigItem(**item)
    raise APIError(
        status_code=500, code=ErrorCode.internal_error,
        message="Setting saved but not found on readback - registry mismatch.",
    )


@router.delete("/config/{key}", response_model=RuntimeConfigItem)
async def reset_runtime_config(
    key: str,
    admin: User = Depends(get_current_superuser),
) -> RuntimeConfigItem:
    """Drop the override → next read returns the code default.

    Idempotent: deleting a key that has no override succeeds and returns
    the default-state view. The key must still be registered (404 if not).
    """
    try:
        await runtime_config.reset_setting(key)
    except runtime_config.UnknownSettingError:
        raise APIError(
            status_code=404, code=ErrorCode.not_found,
            message=f"Unknown setting key '{key}'.",
        )
    items = await runtime_config.list_all()
    for item in items:
        if item["key"] == key:
            return RuntimeConfigItem(**item)
    raise APIError(
        status_code=500, code=ErrorCode.internal_error,
        message="Setting reset but not found on readback - registry mismatch.",
    )


# --- Ingest log (master-only) -----------------------------------------------
# Surfaces the small audit trail written by the four master-only ingest
# endpoints (/v1/leaderboards/insert, /v1/market/insert,
# /v1/rotations/chaos-chest/insert, /v1/rotations/challenge/insert). The
# portal's Ingest tab uses this to render "Recent submissions" so the
# operator can see when the last bot push landed and what shape it had.

@router.get("/ingest/log")
async def list_ingest_log(
    limit: int = Query(default=20, ge=1, le=200),
    endpoint: str | None = Query(
        default=None,
        description="Optional filter: only entries for this route path.",
    ),
) -> list[dict]:
    """Recent rows from the ingest log, newest first. Master-only via
    the router-level dep."""
    rows = await ingest_log.recent(limit=limit, endpoint=endpoint)
    return [
        {
            "endpoint": r.endpoint,
            "timestamp": r.timestamp.isoformat(),
            "user_email": r.user_email,
            "success": r.success,
            "summary": r.summary,
            "error": r.error,
            "auth_via": r.auth_via,
            "token_name": r.token_name,
        }
        for r in rows
    ]


# --- Discord: push slash commands -----------------------------------------
# Discord only learns about our slash commands when we PUT them to its API -
# editing app/discord/commands.py does nothing on its own. These let a
# superuser preview the local command set and push it from the panel (no CLI).
# Global pushes take up to ~1h to propagate; pass ?guild_id= for an instant
# per-server push while testing.

@router.get("/discord/commands")
async def discord_commands_preview() -> dict:
    """The slash commands defined locally - i.e. what 'Push to Discord' sends."""
    from app.discord.commands import COMMAND_DEFS
    return {
        "count": len(COMMAND_DEFS),
        "commands": [
            {"name": c["name"], "description": c.get("description", "")}
            for c in COMMAND_DEFS
        ],
    }


@router.post("/discord/register-commands")
async def discord_register_commands(
    guild_id: str | None = Query(
        default=None,
        description="Optional guild id for an instant per-server push (else global).",
    ),
) -> dict:
    """Bulk-overwrite the slash commands on Discord. Master-only via the
    router-level dep; uses the configured bot token."""
    from app.discord.registration import DiscordRegistrationError, register_commands
    try:
        cmds = await register_commands(guild_id)
    except DiscordRegistrationError as exc:
        raise APIError(400, ErrorCode.bad_request, str(exc))
    return {
        "scope": "guild" if guild_id else "global",
        "guild_id": guild_id,
        "count": len(cmds),
        "commands": [
            {"name": c.get("name"), "description": c.get("description", "")}
            for c in cmds
        ],
    }


@router.get("/bot/stats")
async def bot_stats() -> dict:
    """Bot usage for the Dev Portal: servers + users it can see (gateway-reported),
    and per-command slash usage counts. Master-only via the router-level dep."""
    from app.bot import stats
    return await stats.get_stats()


@router.post("/discord/clear-guild-commands")
async def discord_clear_guild_commands(
    guild_id: str = Query(..., description="Guild whose guild-scoped commands to clear."),
) -> dict:
    """Remove a guild's guild-scoped slash commands (bulk-overwrite with an empty
    set). Fixes commands that show twice in one server - the leftover of an instant
    per-guild test push layered on top of the global commands. The global command
    set is untouched. Master-only via the router-level dep."""
    from app.discord.registration import DiscordRegistrationError, clear_guild_commands
    try:
        await clear_guild_commands(guild_id)
    except DiscordRegistrationError as exc:
        raise APIError(400, ErrorCode.bad_request, str(exc))
    return {"cleared": True, "guild_id": guild_id}


# --- Supporters: the public credits list -----------------------------------
# Mirrors the interest-items pattern. Master-only (router-level dep). The list
# renders on /support and is exposed tokenless at /v1/misc/supporters.

def _supporter_view(doc) -> SupporterAdminView:
    return SupporterAdminView(
        name=doc.name,
        added_by=str(doc.added_by) if doc.added_by else None,
        created_at=doc.created_at,
    )


@router.get("/supporters", response_model=SupporterAdminList)
async def list_supporters_admin() -> SupporterAdminList:
    """Every supporter with admin metadata (who added it, when)."""
    docs = await supporters_service.admin_list()
    return SupporterAdminList(items=[_supporter_view(d) for d in docs], count=len(docs))


@router.post("/supporters", response_model=SupporterAdminView, status_code=201)
async def add_supporter_admin(
    req: SupporterAddRequest,
    admin: User = Depends(get_current_superuser),
) -> SupporterAdminView:
    try:
        doc = await supporters_service.add(req.name, added_by=admin.id)
    except ValueError as e:
        raise_from_value_error(e)
    return _supporter_view(doc)


@router.delete("/supporters/{name}", status_code=204)
async def remove_supporter_admin(name: str) -> None:
    removed = await supporters_service.remove(name)
    if not removed:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message=f"No supporter named '{name}'")


def _site_claim_view(u: SiteUser) -> SiteClaimAdminView:
    return SiteClaimAdminView(
        user_id=str(u.id), username=u.username, display_name=u.display_name,
        discord_id=u.discord_id, claimed_trove_name=u.claimed_trove_name,
        claimed_trove_display=u.claimed_trove_display, claimed_at=u.claimed_at,
        claim_verified=u.claim_verified, claim_verified_at=u.claim_verified_at,
    )


async def _get_claimant(user_id: str) -> SiteUser:
    """Load a SiteUser by id, 404 on a bad id or no pending claim."""
    oid = to_oid(user_id)
    if oid is None:
        raise APIError(status_code=404, code=ErrorCode.not_found, message="No such user.")
    u = await SiteUser.get(oid)
    if u is None or not u.claimed_trove_name:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message="No Trove-name claim for that user.")
    return u


@router.get("/site-claims", response_model=SiteClaimAdminList)
async def list_site_claims(pending_only: bool = Query(default=True)) -> SiteClaimAdminList:
    """Trove-name claims for master review. ``pending_only`` (default) shows just the
    unverified ones awaiting approval; set it false to see every claimed name."""
    query: dict = {"claimed_trove_name": {"$ne": None}}
    if pending_only:
        query["claim_verified"] = False
    docs = await SiteUser.find(query).sort("+claimed_at").to_list()
    return SiteClaimAdminList(items=[_site_claim_view(u) for u in docs], count=len(docs))


@router.post("/site-claims/{user_id}/approve", response_model=SiteClaimAdminView)
async def approve_site_claim(user_id: str) -> SiteClaimAdminView:
    """Manually verify a user's claimed Trove name (master ownership approval)."""
    u = await _get_claimant(user_id)
    u.claim_verified = True
    u.claim_verified_at = utcnow()
    u.updated_at = utcnow()
    await u.save()
    return _site_claim_view(u)


@router.post("/site-claims/{user_id}/reject", response_model=SiteClaimAdminView)
async def reject_site_claim(user_id: str) -> SiteClaimAdminView:
    """Reject a claim and release the name so someone else can claim it."""
    u = await _get_claimant(user_id)
    u.clear_claim()
    u.updated_at = utcnow()
    await u.save()
    return _site_claim_view(u)


@router.put("/supporters", response_model=SupporterBulkReplaceResponse)
async def replace_supporters_admin(
    req: SupporterBulkReplaceRequest,
    admin: User = Depends(get_current_superuser),
) -> SupporterBulkReplaceResponse:
    """Bulk replace: drop everything, then insert the de-duped, trimmed list.
    Refuses an empty list - use the per-name DELETE to remove individually."""
    try:
        summary = await supporters_service.replace(req.names, added_by=admin.id)
    except ValueError as e:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message=str(e))
    return SupporterBulkReplaceResponse(**summary)


# --- Mods Hub moderation (master-only takedown of shared mods) --------------
# Users file reports via /v1/mods/hub/projects/{slug}/report; masters triage
# them here and take a project down (drops it from all public listings + detail
# reads) or restore it. Surfaced as buttons on the dev-portal master panel.

@router.get("/mods/reports")
async def list_mod_reports(resolved: bool = Query(default=False)) -> dict:
    """Open (or resolved) notice-and-action reports against public content (mods,
    modpacks, profiles), newest first."""
    from app.trove import moderation
    return {"items": await moderation.list_reports(resolved=resolved)}


@router.post("/mods/reports/{report_id}/dismiss")
async def dismiss_mod_report(report_id: str) -> dict:
    """Resolve a single report without removing the content (bogus/non-actionable)."""
    from app.trove import moderation
    await moderation.dismiss_report(report_id)
    return {"ok": True}


@router.get("/mods/projects")
async def list_all_mod_projects(
    q: str | None = Query(default=None),
    owner: str | None = Query(default=None),
    visibility: str | None = Query(default=None),
    sort: str = Query(default="updated",
                      description="updated | created | popularity | downloads | stars | size"),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Every modder's project (drafts + taken-down included) for master oversight."""
    from app.trove.mods_hub import service as mods_hub_service
    items, total = await mods_hub_service.master_list_projects(
        q=q, owner=owner, visibility=visibility, sort=sort, limit=limit, offset=offset,
    )
    return {"items": items, "count": len(items), "total": total}


@router.delete("/mods/projects/{project_id}", status_code=204)
async def delete_mod_project(project_id: str) -> None:
    """Force-delete any modder's project (master). Removes branches, commits,
    releases and reports; content-addressed blobs are left for GC. Addressed by
    project id (slugs are only unique per owner)."""
    from app.trove.mods_hub import service as mods_hub_service
    await mods_hub_service.master_delete_project(project_id)


@router.post("/mods/projects/{project_id}/takedown")
async def take_down_mod(project_id: str, req: TakedownRequest) -> dict:
    """Remove a mod project from public view (owner still sees it, flagged)."""
    from app.trove.mods_hub import service as mods_hub_service
    project = await mods_hub_service.take_down(project_id, req.reason)
    return {"slug": project.slug, "handle": project.owner_handle,
            "taken_down": project.taken_down, "takedown_reason": project.takedown_reason}


@router.post("/mods/projects/{project_id}/restore")
async def restore_mod(project_id: str) -> dict:
    """Reverse a takedown - the project becomes publicly visible again."""
    from app.trove.mods_hub import service as mods_hub_service
    project = await mods_hub_service.restore(project_id)
    return {"slug": project.slug, "handle": project.owner_handle,
            "taken_down": project.taken_down}


@router.post("/mods/profiles/{profile_id}/takedown")
async def take_down_profile(profile_id: str, req: TakedownRequest) -> dict:
    """Remove a creator profile from public view (owner still sees it, flagged)."""
    from app.trove.mods_hub import service as mods_hub_service
    p = await mods_hub_service.take_down_profile(profile_id, req.reason)
    return {"handle": p.handle, "taken_down": p.taken_down, "takedown_reason": p.takedown_reason}


@router.post("/mods/profiles/{profile_id}/restore")
async def restore_profile(profile_id: str) -> dict:
    """Reverse a profile takedown."""
    from app.trove.mods_hub import service as mods_hub_service
    p = await mods_hub_service.restore_profile(profile_id)
    return {"handle": p.handle, "taken_down": p.taken_down}


# --- Stray (imported) mods: catalog import + approval queue + claims --------

@router.get("/mods/stray/import")
async def stray_import_state() -> dict:
    """Progress/state of the stray-mod bulk import + resync job."""
    from app.trove.mods_hub import strayimport
    return await strayimport.get_state()


@router.post("/mods/stray/import")
async def stray_import_start(
    resync: bool = Query(default=False, description="Refresh existing + queue new mods as pending, vs a fresh bulk import."),
    force: bool = Query(default=False, description="Start even if a run looks in-progress (clears a stale flag)."),
) -> dict:
    """Kick off the stray-mod import (``resync=false`` = bulk import, visible;
    ``resync=true`` = refresh + queue new mods for approval). Runs in the background."""
    from app.trove.mods_hub import strayimport
    return await strayimport.start(resync, force=force)


@router.get("/mods/stray")
async def list_stray_mods(
    status: str | None = Query(default="pending", description="pending | approved | rejected"),
    q: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Imported stray mods for review (default: the pending approval queue)."""
    from app.trove.mods_hub import service as mods_hub_service
    items, total = await mods_hub_service.master_list_stray(
        status=status, q=q, limit=limit, offset=offset)
    return {"items": items, "count": len(items), "total": total}


@router.post("/mods/stray/{project_id}/approve")
async def approve_stray_mod(project_id: str) -> dict:
    """Approve a pending stray mod -> visible in the public catalog."""
    from app.trove.mods_hub import service as mods_hub_service
    return await mods_hub_service.approve_stray(project_id)


@router.post("/mods/stray/{project_id}/reject")
async def reject_stray_mod(project_id: str) -> dict:
    """Reject a stray mod -> hidden, skipped on future resyncs."""
    from app.trove.mods_hub import service as mods_hub_service
    return await mods_hub_service.reject_stray(project_id)


@router.post("/mods/stray/assign")
async def assign_stray_mods(
    req: StrayAssignRequest, admin: User = Depends(get_current_superuser),
) -> dict:
    """Hand one or more stray mods directly to a site user by their database id (no
    claim request) - supports bulk/mass assignment of a selection."""
    from app.trove.mods_hub import service as mods_hub_service
    return await mods_hub_service.admin_assign_stray(req.project_ids, req.user_id, admin.id)


@router.get("/mods/claims")
async def list_mod_claims(
    status: str | None = Query(default="pending", description="pending | approved | rejected"),
) -> dict:
    """User requests to claim stray mods (default: pending)."""
    from app.trove.mods_hub import service as mods_hub_service
    return {"items": await mods_hub_service.list_claims(status=status)}


@router.post("/mods/claims/{claim_id}/approve")
async def approve_mod_claim(claim_id: str, admin: User = Depends(get_current_superuser)) -> dict:
    """Approve a claim: hand the stray mod over to the claimant (becomes their mod)."""
    from app.trove.mods_hub import service as mods_hub_service
    return await mods_hub_service.approve_claim(claim_id, admin.id)


@router.post("/mods/claims/{claim_id}/reject")
async def reject_mod_claim(claim_id: str, admin: User = Depends(get_current_superuser)) -> dict:
    from app.trove.mods_hub import service as mods_hub_service
    return await mods_hub_service.reject_claim(claim_id, admin.id)


# --- Modpack moderation (mirror of /mods/projects) --------------------------

@router.get("/modpacks")
async def list_all_modpacks(
    q: str | None = Query(default=None),
    owner: str | None = Query(default=None),
    visibility: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Every user's modpack (drafts + taken-down included) for master oversight."""
    from app.trove.modpacks import service as modpacks_service
    items, total = await modpacks_service.master_list_modpacks(
        q=q, owner=owner, visibility=visibility, limit=limit, offset=offset)
    return {"items": items, "count": len(items), "total": total}


@router.delete("/modpacks/{pack_id}", status_code=204)
async def delete_modpack_admin(pack_id: str) -> None:
    """Force-delete any modpack (master). Addressed by id (slugs are per-owner)."""
    from app.trove.modpacks import service as modpacks_service
    await modpacks_service.master_delete(pack_id)


@router.post("/modpacks/{pack_id}/takedown")
async def take_down_modpack(pack_id: str, req: TakedownRequest) -> dict:
    """Hide a modpack from public view (owner still sees it, flagged)."""
    from app.trove.modpacks import service as modpacks_service
    pack = await modpacks_service.take_down(pack_id, req.reason)
    return {"slug": pack.slug, "handle": pack.owner_handle,
            "taken_down": pack.taken_down, "takedown_reason": pack.takedown_reason}


@router.post("/modpacks/{pack_id}/restore")
async def restore_modpack(pack_id: str) -> dict:
    from app.trove.modpacks import service as modpacks_service
    pack = await modpacks_service.restore(pack_id)
    return {"slug": pack.slug, "handle": pack.owner_handle, "taken_down": pack.taken_down}


# --- Trove username change requests -----------------------------------------

@router.get("/username-requests")
async def list_username_requests(
    status: str | None = Query(default="pending", description="pending | approved | rejected"),
) -> dict:
    """User requests to change their frozen Trove username (default: pending)."""
    from app.site_auth import usernames
    return {"items": await usernames.list_requests(status=status)}


@router.post("/username-requests/{request_id}/approve")
async def approve_username_request(
    request_id: str, admin: User = Depends(get_current_superuser),
) -> dict:
    """Approve: rename the account's username + re-home their mod/modpack handles."""
    from app.site_auth import usernames
    return await usernames.approve_request(request_id, admin.id)


@router.post("/username-requests/{request_id}/reject")
async def reject_username_request(
    request_id: str, req: TakedownRequest, admin: User = Depends(get_current_superuser),
) -> dict:
    """Deny a username request, with a reason shown to the user."""
    from app.site_auth import usernames
    return await usernames.reject_request(request_id, admin.id, req.reason)


# --- Site (dashboard) users -------------------------------------------------
# Discord-signup accounts that own the public website (mods/modpacks/profiles),
# distinct from the dev-portal API ``User`` accounts handled by /admin/users.

def _site_user_dto(u: SiteUser, mod_count: int = 0, modpack_count: int = 0) -> dict:
    return {
        "id": str(u.id),
        "username": u.username,
        "discord_handle": u.discord_handle or "",
        "display_name": u.display_name,
        "is_active": u.is_active,
        "is_verified": u.is_verified,
        "discord_id": str(u.discord_id) if u.discord_id else None,
        "claimed_trove_name": u.claimed_trove_display or u.claimed_trove_name,
        "claim_verified": u.claim_verified,
        "mod_count": mod_count,
        "modpack_count": modpack_count,
        "created_at": iso(u.created_at),
        "last_login_at": iso(u.last_login_at),
    }


async def _get_site_user(user_id: str) -> SiteUser:
    return await _load_site_user_or_404(user_id, "No such site user.")


@router.get("/site-users")
async def list_site_users(
    q: str | None = Query(default=None, description="search username / discord handle / display name"),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Dashboard (Discord-signup) accounts, with their owned mod/modpack counts."""
    import re as _re

    from app.trove.modpacks.models import ModpackProject
    from app.trove.mods_hub.models import ModProject

    query: dict = {}
    if q and q.strip():
        rx = {"$regex": _re.escape(q.strip()), "$options": "i"}
        query = {"$or": [{"username": rx}, {"discord_handle": rx},
                         {"display_name": rx}]}
    total = await SiteUser.find(query).count()
    docs = await SiteUser.find(query).sort("-created_at").skip(offset).limit(limit).to_list()
    ids = [u.id for u in docs]
    mods: dict = {}
    packs: dict = {}
    if ids:
        mrows = await ModProject.aggregate([
            {"$match": {"owner_id": {"$in": ids}}},
            {"$group": {"_id": "$owner_id", "n": {"$sum": 1}}}]).to_list()
        mods = {str(r["_id"]): r["n"] for r in mrows}
        prows = await ModpackProject.aggregate([
            {"$match": {"owner_id": {"$in": ids}}},
            {"$group": {"_id": "$owner_id", "n": {"$sum": 1}}}]).to_list()
        packs = {str(r["_id"]): r["n"] for r in prows}
    items = [_site_user_dto(u, mods.get(str(u.id), 0), packs.get(str(u.id), 0)) for u in docs]
    return {"items": items, "count": len(items), "total": total}


@router.get("/site-users/{user_id}")
async def get_site_user(user_id: str) -> dict:
    u = await _get_site_user(user_id)
    return _site_user_dto(u)


@router.post("/site-users/{user_id}/deactivate")
async def deactivate_site_user(user_id: str) -> dict:
    """Disable a dashboard account and end all of its sessions immediately."""
    from app.site_auth.sessions import revoke_all_sessions
    u = await _get_site_user(user_id)
    u.is_active = False
    u.updated_at = utcnow()
    await u.save()
    await revoke_all_sessions(u)  # bumps token_version → every access token dies
    return {"id": str(u.id), "is_active": u.is_active}


@router.post("/site-users/{user_id}/activate")
async def activate_site_user(user_id: str) -> dict:
    """Re-enable a previously deactivated dashboard account."""
    u = await _get_site_user(user_id)
    u.is_active = True
    u.updated_at = utcnow()
    await u.save()
    return {"id": str(u.id), "is_active": u.is_active}


@router.post("/site-users/{user_id}/logout")
async def logout_site_user(user_id: str) -> dict:
    """Force-log-out: end every active session without disabling the account."""
    from app.site_auth.sessions import revoke_all_sessions
    u = await _get_site_user(user_id)
    await revoke_all_sessions(u)
    return {"id": str(u.id), "ok": True}


@router.post("/site-users/{user_id}/username")
async def set_site_username(
    user_id: str, body: SiteUsernameRequestBody, admin: User = Depends(get_current_superuser),
) -> dict:
    """Master override of a user's frozen Trove username (re-homes their handles)."""
    from app.site_auth import usernames
    return await usernames.admin_set_username(user_id, body.username, admin.id)


@router.post("/site-users/{user_id}/claimed-name")
async def set_site_claimed_name(user_id: str, req: SiteClaimedNameRequest) -> dict:
    """Master override of a user's claimed Trove (leaderboard) name. Setting a name
    marks it admin-verified; an empty name clears the claim."""
    u = await _get_site_user(user_id)
    name = (req.name or "").strip()
    if not name:
        u.clear_claim()
    else:
        low = name.lower()
        clash = await SiteUser.find_one(SiteUser.claimed_trove_name == low)
        if clash is not None and clash.id != u.id:
            raise APIError(status_code=409, code=ErrorCode.conflict,
                           message=f"'{name}' is already claimed by another user.")
        u.claimed_trove_name = low
        u.claimed_trove_display = name
        u.claimed_at = utcnow()
        u.claim_verified = True
        u.claim_verified_at = utcnow()
        u.claim_baseline = {}
    u.updated_at = utcnow()
    await u.save()
    return _site_user_dto(u)


@router.post("/site-users/{user_id}/refresh-discord")
async def refresh_site_discord(user_id: str) -> dict:
    """Re-fetch the user's live Discord handle (+ avatar) from Discord via the bot."""
    from app.bot import discord_rest
    u = await _get_site_user(user_id)
    if not u.discord_id:
        raise APIError(status_code=400, code=ErrorCode.bad_request,
                       message="This account has no linked Discord id.")
    try:
        data = await discord_rest.fetch_user(u.discord_id)
    except discord_rest.DiscordRestError as exc:
        raise APIError(status_code=502, code=ErrorCode.service_unavailable, message=str(exc))
    if not data:
        raise APIError(status_code=404, code=ErrorCode.not_found,
                       message="Discord has no record of that user any more.")
    handle = (data.get("username") or "").strip()
    if handle:
        u.discord_handle = handle
    if "avatar" in data:
        u.discord_avatar = data.get("avatar")
    u.updated_at = utcnow()
    await u.save()
    return _site_user_dto(u)


# --- Embeddable viewer: live mod previews -----------------------------------

@router.get("/embed/uploads")
async def embed_upload_stats() -> dict:
    """Mod previews a partner has open RIGHT NOW - count + bytes held in Redis.

    There is nothing to clean up here: uploads are never written to disk and expire
    on their own (``embed.upload_ttl_minutes``). This is a load reading, not a store."""
    from app.embed import uploads
    return await uploads.stats()
