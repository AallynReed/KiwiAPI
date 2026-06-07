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

from fastapi import APIRouter, HTTPException, Request, UploadFile
from fastapi.responses import HTMLResponse, JSONResponse, StreamingResponse
from fastapi.templating import Jinja2Templates

from app.core.config import settings

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
