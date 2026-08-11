"""HTTP surface for the embeddable viewers.

Three groups, all gated by ``feature_embed_viewer_enabled``:

  ``GET  /embed/viewer``      the chrome-free HTML page a partner puts in an iframe.
                              Framing is controlled by ``embed.allowed_origins``
                              (CSP frame-ancestors, applied in app/core/middleware.py).
  ``GET  /site/embed/*``      the data the page fetches. Tokenless + throttled.
  ``POST /v1/embed/tmod``     server-to-server: a partner uploads a .tmod once and gets
                              a preview token. Needs an API token with ``mods:read``
                              (it never touches a browser, so a real credential costs
                              the partner nothing and keeps the endpoint from being an
                              open upload).

**Where the page is served matters.** In production the PAGE comes from the website
container (``app/web/pages.py``) on ``trove.aallyn.net`` - the ONLY origin permitted
to be framed anywhere in the estate - while the data above stays here on the API,
which refuses framing outright. The page reaches across that boundary with CORS,
like every other page on the site. The copy of the page route below serves the
single-process (pre-split) and local-dev case, where both surfaces share an origin.

The read endpoints deliberately mirror the hub's ``/site/mods/releases/{id}/*``
shapes, so the SAME viewer JS drives both surfaces with only a base URL swapped.
"""

from __future__ import annotations

import hashlib
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
from app.trove.render import bp_cache

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
_MED_MAX_AGE = 300
_MED = {"Cache-Control": f"public, max-age={_MED_MAX_AGE}"}
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
    prefab: str | None = Query(default=None, max_length=400,
                               description="A creature by its game prefab path (or unambiguous filename): "
                                           "its rig, clips and every part come from the live game files."),
    dress: str | None = Query(default=None, max_length=700,
                              description="A dressed character: `class:costume:hat:face:weapon:head:hair:weapon_family`. Each slot takes a style stem or a blueprint name."),
) -> service.Source:
    """Shared dependency: resolve the one source param the caller passed."""
    return await service.resolve(release=release, tmod=tmod, game=game, prefab=prefab,
                                 dress=dress)


_SRC = Depends(_source)


# ── the iframe page ────────────────────────────────────────────────────────

@embed_page_router.get("/embed/viewer", response_class=HTMLResponse)
async def embed_viewer(
    request: Request,
    release: str | None = Query(default=None, max_length=64),
    tmod: str | None = Query(default=None, max_length=64),
    game: str | None = Query(default=None, max_length=400),
    prefab: str | None = Query(default=None, max_length=400),
    dress: str | None = Query(default=None, max_length=700),
    path: str | None = Query(default=None, max_length=400),
    sound: str | None = Query(default=None, max_length=200),
    mode: str = Query(default="auto", pattern="^(auto|blueprint|assembled|vfx|audio)$"),
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
        "prefab": prefab or "", "dress": dress or "",
        "path": path or "", "sound": sound or "", "mode": mode, "theme": theme,
        # Same origin here (this process serves both), so no prefix is needed.
        "api_base": "",
        "app_url": settings.app_url.rstrip("/"),
    })


# ── data the page fetches (same-origin, tokenless) ─────────────────────────

@embed_page_router.get("/site/embed/manifest", response_class=JSONResponse)
async def embed_manifest(
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    """What this source can preview: blueprints (+ rig) and .pkfx effects."""
    return JSONResponse(await service.manifest(src), headers=_SHORT)


@embed_page_router.get("/site/embed/blueprint", response_class=Response)
async def embed_blueprint(
    request: Request, path: str = Query(default="", max_length=400),
    fmt: str = Query(default="json", pattern="^(json|bin)$"),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> Response:
    """Served from the decoded-payload cache, gzipped and ETag'd - a partner's
    visitors re-fetching the same model cost us a 304."""
    cached = await service.blueprint(src, path, fmt)
    return bp_cache.respond(request, cached, max_age=_MED_MAX_AGE)


@embed_page_router.get("/site/embed/assembled", response_class=Response)
async def embed_assembled(
    request: Request, fmt: str = Query(default="json", pattern="^(json|bin)$"),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> Response:
    model = await service.assembled(src, fmt)
    if model is None:
        raise APIError(404, ErrorCode.not_found, "No assemblable creature here.")
    return bp_cache.respond(request, model, max_age=_MED_MAX_AGE)


@embed_page_router.get("/site/embed/vfx/manifest", response_class=JSONResponse)
async def embed_vfx_manifest(
    path: str = Query(..., min_length=1, max_length=400),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    return JSONResponse(await service.vfx_manifest(src, path), headers=_MED)


@embed_page_router.get("/site/embed/audio/bank", response_class=JSONResponse)
async def embed_audio_bank(
    path: str = Query(default="", max_length=400),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> JSONResponse:
    """Every sound in one ``.bnk`` - names, codecs, durations. Nothing is decoded
    here, so opening a bank of 1,600 effects costs one small JSON body."""
    return JSONResponse(await service.audio_bank(src, path), headers=_MED)


@embed_page_router.get("/site/embed/audio/sound", response_class=Response)
async def embed_audio_sound(
    request: Request,
    path: str = Query(default="", max_length=400),
    id: int = Query(..., ge=0, le=0xFFFFFFFF),
    raw: bool = Query(default=False),
    src: service.Source = _SRC, _t: None = _LIMIT,
) -> Response:
    """One sound, decoded to Ogg or WAV. ETag'd on the media's own hash, so a
    visitor replaying a sound - or a second page embedding it - costs a 304."""
    data, media, filename, etag = await service.audio_sound(src, path, id, raw)
    if request.headers.get("if-none-match") == etag:
        return Response(status_code=304, headers={"ETag": etag})
    return Response(content=data, media_type=media, headers={
        "ETag": etag,
        "Cache-Control": "public, max-age=86400",
        "Content-Disposition": f'inline; filename="{filename}"',
    })


@embed_page_router.get("/site/embed/allowed-origins", response_class=JSONResponse)
async def embed_allowed_origins() -> JSONResponse:
    """The framing allowlist, for the website container - it serves the framable page
    but has no database to read the setting from (see app/embed/service.py).

    Discloses nothing new: the same list is already in the CSP of the public page.
    Not rate-limited with the others - it's an internal, cached, once-per-30s call
    on the compose network, and throttling it could deny framing estate-wide."""
    return JSONResponse({"origins": await service.allowed_origins()}, headers=_SHORT)


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
    """Hand us a ``.tmod`` and get a short-lived **preview token** for the viewer.

    Call this as your mod page renders, then embed ``/embed/viewer?tmod=<token>``.

    We do not keep the file. It's held in memory only for ``expires_in`` seconds -
    long enough for a visitor to look at it - and then it's gone, with no copy left
    on our side. The expiry does NOT extend while someone is watching, so treat the
    token as per-page-render rather than something to cache for days. The token is
    the file's SHA-256, so posting the same mod again just refreshes the same entry.
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

    # Hash here rather than in the store: the token IS the content hash, and the
    # upload path is the only place that has a reason to compute it.
    got = await uploads.store(data, hashlib.sha256(data).hexdigest())
    return {
        **got,
        # The WEBSITE host: it's the only origin permitted to be framed (the API
        # host refuses framing), so this is the URL a partner can actually embed.
        "viewer_url": f"{settings.app_url.rstrip('/')}/embed/viewer?tmod={got['token']}",
    }


@embed_api_router.get("/tmod/{token}")
async def upload_status(token: str, ctx: AccessContext = _PUB) -> dict:
    """Whether a preview token is still live - so a partner can re-post before a
    visitor hits an expired embed rather than after."""
    data = await uploads.load(token)
    if data is None:
        return {"token": token, "live": False}
    return {"token": token, "live": True, "size": len(data)}
