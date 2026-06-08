"""Routes for the BetterTroveTools showcase site (`trove.aallyn.net`).

Four routes:
- ``GET  /``                — the landing page (renders templates/index.html)
- ``GET  /documentation``   — the user manual
- ``GET  /unlock_debug``    — file-upload form
- ``POST /unlock_debug``    — byte-patches an uploaded Trove.exe to enable the
                              debug console, returns the patched binary
- ``GET  /unlock_fps``      — same shape as above for the FPS uncap

Templates were ported from a Quart app; the old ``url_for('static', ...)``
calls were rewritten to hardcoded ``/static/...`` paths (the mount lives at
``/static`` in ``app/main.py``), so the templates render straight through
Jinja2Templates without a custom url-builder.
"""

from io import BytesIO
from pathlib import Path

from fastapi import APIRouter, HTTPException, Query, Request, UploadFile
from fastapi.responses import HTMLResponse, JSONResponse, StreamingResponse
from fastapi.templating import Jinja2Templates

from app.admin import runtime_config
from app.core.config import settings
from app.trove.leaderboards import service as leaderboards_service

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
    """In-game Trove slash-command reference. Page shell + JS only —
    actual command data lives in ``site/static/commands.json`` and is
    fetched + rendered client-side so language switches don't reload."""
    return _TEMPLATES.TemplateResponse(request, "commands.html", {})


@router.get("/leaderboards", response_class=HTMLResponse)
async def leaderboards(request: Request) -> HTMLResponse:
    """Trove leaderboards browser — public site read of the same data the
    ``/v1/leaderboards/*`` API exposes. The page hits dedicated JSON
    endpoints under ``/site/leaderboards/*`` (see below) which bypass the
    public API's token/scope/rate-limit pipeline and call the service
    layer directly. The data is public anyway, so the bypass costs us
    nothing and avoids subjecting site browsers to per-token caps."""
    return _TEMPLATES.TemplateResponse(request, "leaderboards.html", {})


# --- /leaderboards JSON endpoints ------------------------------------------
# These mirror the four read-side helpers from app/trove/router.py but skip
# the TokenContext dep + archive-rate-limit. They're intentionally NOT
# include_in_schema (the router already opts out) — the public surface is
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


@router.get("/site/screenshots.json", response_class=JSONResponse)
async def hero_screenshots() -> JSONResponse:
    """List of Trove screenshots for the landing-page hero slideshow.

    Reads ``site/static/trove-screens/`` and returns every image (by file
    extension whitelist) sorted alphabetically. Lets the user drop new
    screenshots into the folder and have them appear on the next page
    load without an HTML edit. Filenames are exposed as URLs only — full
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


# --- byte-patcher tools -----------------------------------------------------
# Each tool reads the uploaded Trove.exe, replaces ONE specific byte sequence,
# and streams the patched file back. The body-cap exception for /unlock_*
# (`site_max_request_body_bytes = 110 MB`) lives in app/core/middleware.py.

_DEBUG_FIND = bytes.fromhex("7C 39 68 E0 02 00 00")
_DEBUG_REPL = bytes.fromhex("90 90 68 E0 02 00 00")

_FPS_FIND = bytes.fromhex("11 11 11 11 11 11 81 3F")
_FPS_REPL = bytes.fromhex("00 00 00 00 00 00 00 00")


async def _patch_exe(file: UploadFile | None, find: bytes, replace: bytes) -> StreamingResponse:
    if file is None or not file.filename:
        raise HTTPException(status_code=400, detail="No file uploaded.")
    data = await file.read()
    patched = data.replace(find, replace)
    return StreamingResponse(
        BytesIO(patched),
        media_type="application/octet-stream",
        headers={"Content-Disposition": 'attachment; filename="Trove.exe"'},
    )


@router.get("/unlock_debug", response_class=HTMLResponse)
async def unlock_debug_form(request: Request) -> HTMLResponse:
    return _TEMPLATES.TemplateResponse(request, "unlock_debug.html", {})


@router.post("/unlock_debug")
async def unlock_debug(trove_exe: UploadFile | None = None) -> StreamingResponse:
    """Byte-patch the uploaded Trove.exe to enable the in-client debug console."""
    return await _patch_exe(trove_exe, _DEBUG_FIND, _DEBUG_REPL)


@router.get("/unlock_fps", response_class=HTMLResponse)
async def unlock_fps_form(request: Request) -> HTMLResponse:
    return _TEMPLATES.TemplateResponse(request, "unlock_fps.html", {})


@router.post("/unlock_fps")
async def unlock_fps(trove_exe: UploadFile | None = None) -> StreamingResponse:
    """Byte-patch the uploaded Trove.exe to remove the FPS cap."""
    return await _patch_exe(trove_exe, _FPS_FIND, _FPS_REPL)
