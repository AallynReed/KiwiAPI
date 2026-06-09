"""Routes for the BetterTroveTools showcase site (`trove.aallyn.net`).

Page routes: ``/``, ``/documentation``, ``/commands``, ``/leaderboards``,
``/updates``, ``/support``.

Plus a small JSON surface under ``/site/*`` (leaderboards + updates
proxies, screenshots index) that the page-side JS calls same-origin so
visitors don't get throttled by per-token caps.

Templates were ported from a Quart app; the old ``url_for('static', ...)``
calls were rewritten to hardcoded ``/static/...`` paths (the mount lives
at ``/static`` in ``app/main.py``), so the templates render straight
through Jinja2Templates without a custom url-builder.
"""

from pathlib import Path

from fastapi import APIRouter, HTTPException, Query, Request
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.templating import Jinja2Templates

from app.admin import runtime_config
from app.core.config import settings
from app.trove.leaderboards import activity as leaderboards_activity
from app.trove import status as trove_status
from app.trove.leaderboards import detection as leaderboards_detection
from app.trove.leaderboards import service as leaderboards_service
from app.trove.updates import compare as updates_compare
from app.trove.updates import read as updates_read
from app.trove.updates.cas import ContentStore
from app.trove.updates.cdn import BRANCHES as UPDATE_BRANCHES

# Filename extensions accepted as Trove screenshots for the hero slideshow.
# Anything else in the folder (READMEs, .DS_Store, etc.) is silently skipped.
_SCREENSHOT_EXTS = {".webp", ".png", ".jpg", ".jpeg", ".gif"}

_TEMPLATES = Jinja2Templates(directory=str(Path(settings.site_root) / "templates"))

router = APIRouter(tags=["site"], include_in_schema=False)


@router.get("/", response_class=HTMLResponse)
async def home(request: Request) -> HTMLResponse:
    """The BetterTroveTools landing page."""
    return _TEMPLATES.TemplateResponse(request, "index.html", {})


@router.get("/documentation", response_class=HTMLResponse)
async def documentation(request: Request) -> HTMLResponse:
    """The user manual."""
    return _TEMPLATES.TemplateResponse(request, "docs.html", {})


@router.get("/commands", response_class=HTMLResponse)
async def commands(request: Request) -> HTMLResponse:
    """In-game Trove slash-command reference. Page shell + JS only -
    actual command data lives in ``site/static/commands.json`` and is
    fetched + rendered client-side so language switches don't reload."""
    return _TEMPLATES.TemplateResponse(request, "commands.html", {})


@router.get("/support", response_class=HTMLResponse)
async def support(request: Request) -> HTMLResponse:
    """Dedicated 'Support the project' page - landing for the red-heart
    navbar link. The bottom-right floating widget is also rendered on
    every page; this one gives a richer pitch for visitors who want a
    full read on what the donations actually fund."""
    return _TEMPLATES.TemplateResponse(request, "support.html", {})


@router.get("/status", response_class=HTMLResponse)
async def status_page(request: Request) -> HTMLResponse:
    """Dedicated Trove server-status page - live Live/PTS state plus a
    downtime-history timeline. Page shell + JS; data comes from
    ``/site/trove-status`` + ``/site/trove-status/history``."""
    return _TEMPLATES.TemplateResponse(request, "status.html", {})


@router.get("/login", response_class=HTMLResponse)
async def login(request: Request) -> HTMLResponse:
    """Public-facing login form (username OR email + password). Auth
    backend lives at /v1/site-auth/* - see app/site_auth/."""
    return _TEMPLATES.TemplateResponse(request, "login.html", {})


@router.get("/signup", response_class=HTMLResponse)
async def signup(request: Request) -> HTMLResponse:
    """Public-facing signup form (username + email + password). Open
    signup; email verification gates the dashboard's Trove-name claim."""
    return _TEMPLATES.TemplateResponse(request, "signup.html", {})


@router.get("/dashboard", response_class=HTMLResponse)
async def dashboard(request: Request) -> HTMLResponse:
    """Logged-in user dashboard. Client-side checks for a stored token
    and redirects to /login if absent - no server-side gate so the
    page can serve same-origin caches without varying on auth."""
    return _TEMPLATES.TemplateResponse(request, "dashboard.html", {})


@router.get("/forgot-password", response_class=HTMLResponse)
async def forgot_password(request: Request) -> HTMLResponse:
    """Email-based password-reset request form. POSTs to
    /v1/site-auth/forgot-password - enumeration-safe by design (the
    response shape doesn't reveal whether the email is registered)."""
    return _TEMPLATES.TemplateResponse(request, "forgot_password.html", {})


@router.get("/reset-password", response_class=HTMLResponse)
async def reset_password(request: Request) -> HTMLResponse:
    """Landing for the reset link in the email. Reads ``?token=...``
    client-side, prompts for the new password, POSTs to
    /v1/site-auth/reset-password."""
    return _TEMPLATES.TemplateResponse(request, "reset_password.html", {})


@router.get("/market", response_class=HTMLResponse)
async def market(request: Request) -> HTMLResponse:
    """In-game marketplace browser (Beta). Reads from the
    ``market_listings`` collection via the /site/market/* proxies
    below - bypasses the public API's per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "market.html", {})


# --- /site/market/* - same-origin JSON proxies for the page ---------------
# Same shape as /site/leaderboards/* and /site/updates/* - call the
# service layer directly, skip the public API's auth + scope + rate-limit
# pipeline. Same trade-off: data is already public, no reason to throttle
# the same browsers we'd be happy to serve via api.aallyn.net anyway.

@router.get("/site/market/items", response_class=JSONResponse)
async def site_market_items() -> JSONResponse:
    from app.trove.market import service as market_service
    items = await market_service.list_distinct_items()
    return JSONResponse(
        {"items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/market/listings", response_class=JSONResponse)
async def site_market_listings(
    name: str | None = Query(default=None),
    price_min: float | None = Query(default=None, ge=0),
    price_max: float | None = Query(default=None, ge=0),
    last_seen_after: int | None = Query(default=None, ge=0),
    hide_expired: bool = Query(default=True),
    sort: str = Query(default="-last_seen"),
    limit: int = Query(default=100, ge=1, le=500),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    from app.trove.market import service as market_service
    if price_min is not None and price_max is not None and price_min > price_max:
        raise HTTPException(
            status_code=400,
            detail="price_min cannot be greater than price_max",
        )
    items, total = await market_service.list_listings(
        name=name, price_min=price_min, price_max=price_max,
        last_seen_after=last_seen_after, hide_expired=hide_expired,
        sort=sort, limit=limit, offset=offset,
    )
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/market/items/{name}/summary", response_class=JSONResponse)
async def site_market_item_summary(name: str) -> JSONResponse:
    from app.trove.market import service as market_service
    summary = await market_service.item_summary(name)
    if summary is None:
        raise HTTPException(
            status_code=404,
            detail=f"No active market listings for '{name}'",
        )
    return JSONResponse(summary, headers={"Cache-Control": "public, max-age=30"})


@router.get(
    "/site/market/items/{name}/history",
    response_class=JSONResponse,
)
async def site_market_item_history(
    name: str,
    days: int = Query(default=7, ge=1, le=30),
    include_expired: bool = Query(default=False),
    keep_outliers: bool = Query(
        default=False,
        description="Skip the log-space outlier filter and return the raw cloud.",
    ),
) -> JSONResponse:
    """Per-listing price-vs-time series for one item - drives the
    price-evolution chart on the /market page.

    Toggles:
      - ``include_expired`` - include posts that have aged out of the
        in-game live cycle (3h stale / 7d TTL).
      - ``keep_outliers`` - by default we run a log-space modified-Z
        filter (threshold 3.5) to drop extreme one-off listings that
        would otherwise stretch the y-axis through the roof. Pass
        ``true`` to see the raw cloud.
    """
    from app.trove.market import service as market_service
    payload = await market_service.price_history(
        name, days=days,
        include_expired=include_expired,
        keep_outliers=keep_outliers,
    )
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/leaderboards", response_class=HTMLResponse)
async def leaderboards(request: Request) -> HTMLResponse:
    """Trove leaderboards browser - public site read of the same data the
    ``/v1/leaderboards/*`` API exposes. The page hits dedicated JSON
    endpoints under ``/site/leaderboards/*`` (see below) which bypass the
    public API's token/scope/rate-limit pipeline and call the service
    layer directly. The data is public anyway, so the bypass costs us
    nothing and avoids subjecting site browsers to per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "leaderboards.html", {})


# --- /leaderboards JSON endpoints ------------------------------------------
# These mirror the four read-side helpers from app/trove/router.py but skip
# the TokenContext dep + archive-rate-limit. They're intentionally NOT
# include_in_schema (the router already opts out) - the public surface is
# still /v1/leaderboards/*, this is just a site convenience.

@router.get("/site/leaderboards/config", response_class=JSONResponse)
async def site_lb_config() -> JSONResponse:
    """Runtime tunables the leaderboards page needs to render its chrome.

    Currently only the hot-retention window (so the subtitle's "N-day
    live retention" line tracks master-panel changes within the 5s
    runtime_config cache window)."""
    days = await runtime_config.get_setting("leaderboards_hot_retention_days")
    return JSONResponse(
        {"hot_retention_days": int(days)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/timestamps", response_class=JSONResponse)
async def site_lb_timestamps(
    limit: int = Query(default=60, ge=1, le=365),
) -> JSONResponse:
    items = await leaderboards_service.list_timestamps(limit)
    return JSONResponse(
        {"items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/boards", response_class=JSONResponse)
async def site_lb_boards(
    created_at: int = Query(..., description="Anchor in unix seconds"),
) -> JSONResponse:
    rows = await leaderboards_service.list_boards_at(created_at)
    return JSONResponse(
        {"created_at": created_at, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=60"},
    )


# LITERAL-prefix routes must come BEFORE the ``/{uuid}/...`` catch-alls
# below - FastAPI matches in declaration order, and a path-param int
# validator on "activity" / "cheaters" would 422 (not fall through) if
# the catch-all matched first. Same dance as ``/players/{name}/...`` -
# put the named segments above the parameterised ones.
@router.get("/site/leaderboards/activity", response_class=JSONResponse)
async def site_lb_activity() -> JSONResponse:
    """Same payload as `/v1/leaderboards/activity` but served same-origin
    so the page can fetch without CORS."""
    payload = await leaderboards_activity.estimate_active_players()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/leaderboards/activity/history", response_class=JSONResponse)
async def site_lb_activity_history(days: int = 7) -> JSONResponse:
    """Same payload as ``/v1/leaderboards/activity/history`` - same-origin
    proxy so the showcase page can fetch without CORS / token gymnastics.
    Returns a time-series of activity estimates with both raw counts
    and per-hour rates, the latter being what the chart line plots so
    missed-capture gaps don't show as spikes."""
    days = max(1, min(int(days), 30))
    payload = await leaderboards_activity.estimate_active_players_history(days=days)
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    return JSONResponse(payload, headers={"Cache-Control": f"public, max-age={ttl}"})


@router.get("/site/trove-status", response_class=JSONResponse)
async def site_trove_status() -> JSONResponse:
    """Live Trove server status (Live + PTS) - same payload as
    ``/v1/misc/trove-status``, served same-origin so the landing + status
    pages can fetch it without CORS."""
    payload = trove_status.get_status()
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/site/trove-status/history", response_class=JSONResponse)
async def site_trove_status_history(env: str = "live", days: int = 30) -> JSONResponse:
    """Status-timeline history for the /status page (same payload as
    ``/v1/misc/trove-status/history``). ``env`` ∈ live / pts."""
    env = env if env in ("live", "pts") else "live"
    days = max(1, min(int(days), 90))
    payload = await trove_status.get_history(env, days)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/leaderboards/cheaters", response_class=JSONResponse)
async def site_lb_cheaters() -> JSONResponse:
    """Possible-cheaters analysis for the leaderboards page. Same payload
    as the public ``GET /v1/leaderboards/cheaters`` endpoint - served
    here so the page can fetch same-origin (no CORS dance from
    trove.aallyn.net). The detection module itself caches the result
    for ``cheaters_cache_ttl_seconds`` so this is cheap to call."""
    payload = await leaderboards_detection.detect_possible_cheaters()
    ttl = int(await runtime_config.get_setting("cheaters_cache_ttl_seconds"))
    return JSONResponse(
        payload,
        headers={"Cache-Control": f"public, max-age={ttl}"},
    )


@router.get("/site/leaderboards/{uuid}/entries", response_class=JSONResponse)
async def site_lb_entries(
    uuid: int,
    created_at: int = Query(..., description="Anchor in unix seconds"),
    limit: int = Query(default=100, ge=1, le=1000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    items, total = await leaderboards_service.list_entries(
        uuid, created_at, limit=limit, offset=offset,
    )
    return JSONResponse(
        {
            "uuid": uuid, "created_at": created_at,
            "items": items, "count": len(items), "total": total,
        },
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/players/{player_name}/history",
            response_class=JSONResponse)
async def site_lb_player_history(
    player_name: str,
    limit: int = Query(default=50, ge=1, le=500),
    uuid: int | None = Query(default=None),
) -> JSONResponse:
    rows = await leaderboards_service.player_history(
        player_name, limit=limit, uuid=uuid,
    )
    return JSONResponse(
        {"player_name": player_name, "items": rows, "count": len(rows)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/leaderboards/{uuid}/history", response_class=JSONResponse)
async def site_lb_board_history(
    uuid: int,
    days: int = Query(default=7, ge=1, le=30),
    top: int = Query(default=5, ge=1, le=20),
) -> JSONResponse:
    """Score-vs-time trajectories for the current top-``top`` players on
    a board over the last ``days`` days. Drives the per-board chart on
    the leaderboards page."""
    payload = await leaderboards_service.board_history(uuid, days=days, top=top)
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/leaderboards/players/{player_name}/series",
            response_class=JSONResponse)
async def site_lb_player_series(
    player_name: str,
    days: int = Query(default=7, ge=1, le=30),
) -> JSONResponse:
    """Score-vs-time trajectories for ONE player, grouped per board,
    over the last ``days`` days. Drives the per-player chart in the
    history side-panel."""
    payload = await leaderboards_service.player_history_series(
        player_name, days=days,
    )
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=30"})


@router.get("/updates", response_class=HTMLResponse)
async def updates(request: Request) -> HTMLResponse:
    """Trove updates browser - public site read of the same archive that
    ``/v1/updates/*`` exposes. The page hits the JSON helpers below
    (``/site/updates/*``) which bypass the token/scope/rate-limit pipeline
    so site browsers don't get throttled by per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "updates.html", {})


# --- /updates JSON endpoints ----------------------------------------------
# Mirrors the public ``/v1/updates/*`` surface but tokenless + same-origin
# (no CORS, no scope, no archive rate-limit). Implementation is one-line
# calls into the shared read module.

def _site_check_branch(branch: str) -> None:
    if branch not in UPDATE_BRANCHES:
        raise HTTPException(
            status_code=404,
            detail=f"Unknown branch '{branch}' (known: {', '.join(UPDATE_BRANCHES)})",
        )


@router.get("/site/updates/branches", response_class=JSONResponse)
async def site_up_branches() -> JSONResponse:
    items = await updates_read.list_branches()
    # ``list_branches`` returns ``last_probe_at`` as a ``datetime``; the v1
    # mirror runs that through a Pydantic ``BranchInfo`` which handles
    # ISO serialisation for us. ``JSONResponse`` uses plain ``json.dumps``,
    # which would 500 on the datetime - convert manually here.
    serialised = [
        {**b, "last_probe_at": b["last_probe_at"].isoformat() if b.get("last_probe_at") else None}
        for b in items
    ]
    return JSONResponse(
        {"items": serialised, "count": len(serialised)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/versions", response_class=JSONResponse)
async def site_up_versions(
    branch: str,
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    _site_check_branch(branch)
    docs, total = await updates_read.list_versions(branch, limit, offset)
    items = [
        {
            "branch": d.branch, "ordinal": d.ordinal, "version_tag": d.version_tag,
            "captured_at": d.captured_at.isoformat() if d.captured_at else None,
            "completed_at": d.completed_at.isoformat() if d.completed_at else None,
            "files_added": d.files_added, "files_modified": d.files_modified,
            "files_removed": d.files_removed, "bytes_added": d.bytes_added,
        }
        for d in docs
    ]
    return JSONResponse(
        {"items": items, "count": len(items), "total": total},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/changes", response_class=JSONResponse)
async def site_up_changes(
    branch: str,
    ordinal: int | None = Query(default=None),
    version: str | None = Query(default=None),
    type: str | None = Query(default=None),
    limit: int = Query(default=200, ge=1, le=2000),
    offset: int = Query(default=0, ge=0),
) -> JSONResponse:
    _site_check_branch(branch)
    if type is not None and type not in ("added", "modified", "removed"):
        raise HTTPException(status_code=400, detail=f"Invalid type '{type}'")
    ver = await updates_read.resolve_version(branch, ordinal, version)
    if ver is None:
        raise HTTPException(status_code=404, detail="No matching version for that branch")
    docs, total = await updates_read.list_changes(branch, ver.ordinal, type, limit, offset)
    return JSONResponse(
        {
            "branch": branch, "ordinal": ver.ordinal, "version_tag": ver.version_tag,
            "entries": [
                {"path": d.path, "type": d.type,
                 "content_sha256": d.content_sha256, "size": d.size}
                for d in docs
            ],
            "count": len(docs), "total": total,
            "files_added": ver.files_added,
            "files_modified": ver.files_modified,
            "files_removed": ver.files_removed,
        },
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/tree", response_class=JSONResponse)
async def site_up_tree(
    branch: str,
    prefix: str = Query(default=""),
) -> JSONResponse:
    _site_check_branch(branch)
    if prefix and not prefix.endswith("/"):
        prefix += "/"
    entries = await updates_read.list_directory(branch, prefix)
    return JSONResponse(
        {"branch": branch, "prefix": prefix,
         "entries": entries, "count": len(entries)},
        headers={"Cache-Control": "public, max-age=60"},
    )


@router.get("/site/updates/{branch}/file/meta", response_class=JSONResponse)
async def site_up_file_meta(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    _site_check_branch(branch)
    meta = await updates_read.get_file_meta(branch, path)
    if meta is None:
        raise HTTPException(status_code=404, detail=f"No file '{path}'")
    return JSONResponse({"branch": branch, **meta},
                        headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/updates/{branch}/file/history", response_class=JSONResponse)
async def site_up_file_history(
    branch: str, path: str = Query(...),
) -> JSONResponse:
    _site_check_branch(branch)
    rows = await updates_read.file_history(branch, path)
    if not rows:
        raise HTTPException(
            status_code=404,
            detail=f"No history for '{path}' on branch '{branch}'",
        )
    items = [
        {**r, "captured_at": r["captured_at"].isoformat() if r.get("captured_at") else None}
        for r in rows
    ]
    return JSONResponse(
        {"branch": branch, "path": path, "items": items, "count": len(items)},
        headers={"Cache-Control": "public, max-age=30"},
    )


@router.get("/site/updates/{branch}/file/compare", response_class=JSONResponse)
async def site_up_file_compare(
    branch: str,
    path: str = Query(...),
    from_: int = Query(..., alias="from"),
    to: int = Query(...),
) -> JSONResponse:
    """Diff two versions of a file. Same body shape as
    ``/v1/updates/{branch}/file/compare`` - kept here so the page can
    fetch same-origin and avoid the OpenAPI surface for what is, after
    all, a UI-driven request."""
    _site_check_branch(branch)
    v_from = await updates_read.resolve_version(branch, ordinal=from_)
    v_to = await updates_read.resolve_version(branch, ordinal=to)
    if v_from is None or v_to is None:
        raise HTTPException(
            status_code=404,
            detail="One of the requested ordinals doesn't exist on this branch",
        )
    a = await updates_read.resolve_file_at_version(branch, path, from_)
    b = await updates_read.resolve_file_at_version(branch, path, to)
    a_info = {
        "ordinal": v_from.ordinal, "version_tag": v_from.version_tag,
        "captured_at": v_from.captured_at.isoformat() if v_from.captured_at else None,
        "content_sha256": a["content_sha256"] if a else None,
        "size": a["size"] if a else 0,
    }
    b_info = {
        "ordinal": v_to.ordinal, "version_tag": v_to.version_tag,
        "captured_at": v_to.captured_at.isoformat() if v_to.captured_at else None,
        "content_sha256": b["content_sha256"] if b else None,
        "size": b["size"] if b else 0,
    }
    payload = {"branch": branch, "path": path, "from": a_info, "to": b_info}

    if (a_info["content_sha256"] or "") == (b_info["content_sha256"] or "") and a_info["content_sha256"] is not None:
        payload.update({"identical": True, "is_text": True, "hunks": []})
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})
    if a is None and b is None:
        payload.update({
            "identical": True, "is_text": True, "hunks": [],
            "reason": "file did not exist at either side",
        })
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})

    store = ContentStore(settings.trove_update_store_dir)
    a_bytes = store.get(a["content_sha256"]) if a else None
    b_bytes = store.get(b["content_sha256"]) if b else None
    a_dec = updates_compare.decode_blob(a_bytes)
    b_dec = updates_compare.decode_blob(b_bytes)
    if not (a_dec.is_text and b_dec.is_text):
        payload.update({
            "identical": False, "is_text": False,
            "reason": a_dec.reason or b_dec.reason or "binary",
            "hunks": [],
        })
        return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})
    hunks = updates_compare.make_hunks(a_dec.lines, b_dec.lines)
    payload.update({"identical": False, "is_text": True, "hunks": hunks})
    return JSONResponse(payload, headers={"Cache-Control": "public, max-age=60"})


@router.get("/site/screenshots.json", response_class=JSONResponse)
async def hero_screenshots() -> JSONResponse:
    """List of Trove screenshots for the landing-page hero slideshow.

    Reads ``site/static/trove-screens/`` and returns every image (by file
    extension whitelist) sorted alphabetically. Lets the user drop new
    screenshots into the folder and have them appear on the next page
    load without an HTML edit. Filenames are exposed as URLs only - full
    paths never leak.

    Empty list (folder missing, no recognised images) is a clean OK that
    the landing-page JS treats as "no slideshow"; the orbs + grid stay.
    """
    folder = Path(settings.site_root) / "static" / "trove-screens"
    files: list[str] = []
    if folder.is_dir():
        for path in sorted(folder.iterdir()):
            if path.is_file() and path.suffix.lower() in _SCREENSHOT_EXTS:
                files.append(f"/static/trove-screens/{path.name}")
    # 60-second client cache: long enough that a back-button hit doesn't
    # re-list the folder, short enough that adding a new screenshot shows
    # up within a minute without a hard refresh.
    return JSONResponse(
        {"screenshots": files, "count": len(files)},
        headers={"Cache-Control": "public, max-age=60"},
    )


# --- byte-patcher tools (removed 2026-06) -----------------------------------
# /unlock_debug and /unlock_fps used to live here as a pair of file-upload
# forms that byte-patched Trove.exe (one to enable the debug console, the
# other to lift the FPS cap). Both have been removed: Trion shipped anti-
# cheat in mid-2026 and any client-binary tampering is now grounds for a
# ban. Keep this comment as a breadcrumb for the next person who finds a
# stray link in an old changelog and wonders where the route went.
