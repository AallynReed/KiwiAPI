"""HTTP surface for the embeddable viewers.

Three groups, all gated by ``feature_embed_viewer_enabled``:

  ``GET  /embed/viewer``      the chrome-free HTML page a partner puts in an iframe.
                              Framing is controlled by ``embed.allowed_origins``
                              (CSP frame-ancestors, applied in app/core/middleware.py).
  ``GET  /site/embed/*``      the data the page fetches - same-origin from inside the
                              iframe, so no CORS and no token. Tokenless + throttled.
  ``POST /v1/embed/tmod``     server-to-server: a partner uploads a .tmod once and gets
                              a preview token. Needs an API token with ``mods:read``
                              (it never touches a browser, so a real credential costs
                              the partner nothing and keeps the endpoint from being an
                              open upload).

The read endpoints deliberately mirror the hub's ``/site/mods/releases/{id}/*``
shapes, so the SAME viewer JS drives both surfaces with only a base URL swapped.
"""

from __future__ import annotations

from pathlib import Path

from fastapi import APIRouter, Depends, File, Query, Request, Response, UploadFile
from fastapi.responses import HTMLResponse, JSONResponse
from fastapi.templating import Jinja2Templates

from app.admin import runtime_config
from app.core.config import settings
from app.core.dependencies import (
    AccessContext,
    TokenContext,
    public_scope,
    require_scope,
)
from app.core.errors import COMMON_ERROR_RESPONSES, APIError, ErrorCode
from app.core.ratelimit import check_rate_limit
from app.core.utils import client_ip
from app.embed import service, uploads
from app.trove import tmod as tmod_mod

# The iframe page + the JSON/asset endpoints it fetches. Hidden from the public
# reference (they're browser-internal); the documented partner API is the upload
# router below.
embed_page_router = APIRouter(tags=["embed"], include_in_schema=False)

embed_api_router = APIRouter(
    prefix="/v1/embed", tags=["embed"], responses=COMMON_ERROR_RESPONSES,
)

_PUB = Depends(public_scope("mods:read"))

_TEMPLATES = Jinja2Templates(directory=str(Path(settings.site_root) / "templates"))

_SHORT = {"Cache-Control": "public, max-age=60"}
_MED = {"Cache-Control": "public, max-age=300"}
_LONG = {"Cache-Control": "public, max-age=3600"}


async def _throttle(request: Request) -> None:
    """Per-IP budget for the embed's data endpoints.

    These are tokenless *and* callable from any page on the internet, and each one
    decodes a blueprint or parses a .tmod - so they get their own bucket instead of
    riding the shared anonymous API budget, where embed traffic would crowd out
    everything else (and vice versa). Tuned by ``embed_rate_limit_*``.
    """
    max_, window = await runtime_config.get_rate_limit("embed_rate_limit")
    await check_rate_limit(f"embed:{client_ip(request) or 'unknown'}", max_, window)


_LIMIT = Depends(_throttle)


async def _source(
    release: str | None = Query(default=None, max_length=64,
                                description="A published Mods Hub release id."),
    tmod: str | None = Query(default=None, max_length=64,
                             description="An upload token from POST /v1/embed/tmod."),
    game: str | None = Query(default=None, max_length=400,
                             description="A .blueprint/.pkfx path (or filename) in the live game files."),
) -> service.Source:
    """Shared dependency: resolve the one source param the caller passed."""
    return await service.resolve(release=release, tmod=tmod, game=game)


_SRC = Depends(_source)


# ── the iframe page ────────────────────────────────────────────────────────

@embed_page_router.get("/embed/viewer", response_class=HTMLResponse)
async def embed_viewer(
    request: Request,
    release: str | None = Query(default=None, max_length=64),
    tmod: str | None = Query(default=None, max_length=64),
    game: str | None = Query(default=None, max_length=400),
    path: str | None = Query(default=None, max_length=400),
    mode: str = Query(default="auto", pattern="^(auto|blueprint|assembled|vfx)$"),
    theme: str = Query(default="dark", pattern="^(dark|light)$"),
) -> HTMLResponse:
    """The embeddable viewer page.

    Renders a shell only - the source params are handed to the client, which fetches
    the manifest and mounts the right viewer. Deliberately does NOT resolve the source
    server-side: a bad id should paint an in-frame error message the visitor can read,
    not a bare 404 inside somebody else's page.
    """
    return _TEMPLATES.TemplateResponse(request, "embed_viewer.html", {
        "release": release or "", "tmod": tmod or "", "game": game or "",
        "path": path or "", "mode": mode, "theme": theme,
        "app_url": settings.app_url.rstrip("/"),
    })


# ── data the page fetches (same-origin, tokenless) ─────────────────────────

@embed_page_router.get("/site/embed/manifest", response_class=JSONResponse)
async def embed_manifest(
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    """What this source can preview: blueprints (+ rig) and .pkfx effects."""
    return JSONResponse(await service.manifest(src), headers=_SHORT)


@embed_page_router.get("/site/embed/blueprint", response_class=JSONResponse)
async def embed_blueprint(
    path: str = Query(default="", max_length=400),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    return JSONResponse(await service.blueprint(src, path), headers=_MED)


@embed_page_router.get("/site/embed/assembled", response_class=JSONResponse)
async def embed_assembled(
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    model = await service.assembled(src)
    if model is None:
        raise APIError(404, ErrorCode.not_found, "No assemblable creature here.")
    return JSONResponse(model, headers=_MED)


@embed_page_router.get("/site/embed/vfx/manifest", response_class=JSONResponse)
async def embed_vfx_manifest(
    path: str = Query(..., min_length=1, max_length=400),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    return JSONResponse(await service.vfx_manifest(src, path), headers=_MED)


@embed_page_router.get("/site/embed/vfx/asset", response_class=Response)
async def embed_vfx_asset(
    path: str = Query(..., min_length=1, max_length=400),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> Response:
    data, media = await service.vfx_asset(src, path)
    return Response(content=data, media_type=media, headers=_LONG)


# ── partner API: upload a .tmod to preview ─────────────────────────────────

@embed_api_router.post("/tmod")
async def upload_tmod(
    file: UploadFile = File(..., description="The .tmod to make previewable."),
    ctx: TokenContext = Depends(require_scope("mods:read")),
) -> dict:
    """Upload a ``.tmod`` and get a **preview token** for the embeddable viewer.

    Call this from your backend when a mod is published or updated, then embed
    ``/embed/viewer?tmod=<token>``. The token is the file's SHA-256, so re-posting
    an unchanged mod returns the same token and stores nothing new.

    The upload stays previewable for as long as it keeps being viewed (every view
    slides the expiry); a mod nobody looks at is purged. Re-post to renew a token
    that has expired.
    """
    data = await file.read()
    if not data:
        raise APIError(400, ErrorCode.bad_request, "Empty upload.")
    if len(data) > settings.embed_upload_max_bytes:
        raise APIError(413, ErrorCode.bad_request,
                       f"The .tmod exceeds the {settings.embed_upload_max_bytes}-byte limit.")
    try:
        tmod_mod.read_tmod(data)
    except tmod_mod.TmodError as e:
        raise APIError(400, ErrorCode.bad_request, f"Not a readable .tmod: {e}") from None

    name = (file.filename or "").rsplit("/", 1)[-1].rsplit("\\", 1)[-1][:120]
    got = await uploads.store(data, name)
    return {
        **got,
        "viewer_url": f"{settings.app_url.rstrip('/')}/embed/viewer?tmod={got['token']}",
    }


@embed_api_router.get("/tmod/{token}")
async def upload_status(token: str, ctx: AccessContext = _PUB) -> dict:
    """Whether a preview token is still live - so a partner can re-upload before a
    visitor hits an expired embed rather than after."""
    got = await uploads.load(token)
    if got is None:
        return {"token": token, "live": False}
    data, name = got
    return {"token": token, "live": True, "name": name, "size": len(data)}
