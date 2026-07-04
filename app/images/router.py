"""Image studio: Dashboard CRUD + upload + a public PNG render endpoint.

CRUD/upload/preview are site-login-gated (``get_current_site_user``); the render
endpoint (``GET /site/images/{id}.png``) is **public** so Discord can fetch the
image when it's used in an embed. The whole router is gated by the
``feature_image_studio_enabled`` master switch where it's mounted in main.py.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, File, Query, Response, UploadFile

from app.core.errors import APIError, ErrorCode
from app.images import service
from app.images.schemas import DesignBody
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser

router = APIRouter(tags=["image-studio"])

_USER = Depends(get_current_site_user)
_PNG_HEADERS = {"Cache-Control": "public, max-age=60"}


# ── editor metadata + CRUD (site-JWT) ────────────────────────────────────────

@router.get("/v1/images/bindings")
async def list_bindings(user: SiteUser = _USER) -> dict:
    """Event/announcement kinds a design can bind to (+ their variables & samples)."""
    return {"bindings": service.bindings()}


@router.get("/v1/images")
async def list_images(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_designs(user)}


@router.post("/v1/images", status_code=201)
async def create_image(body: DesignBody, user: SiteUser = _USER) -> dict:
    return await service.create_design(user, body)


@router.post("/v1/images/upload", status_code=201)
async def upload_image(file: UploadFile = File(...), user: SiteUser = _USER) -> dict:
    """Upload a PNG/JPEG/WebP/GIF for use as a background or image layer (-> a blob sha)."""
    data = await file.read()
    return await service.upload_image(user, data, file.content_type)


@router.post("/v1/images/preview")
async def preview_image(
    body: DesignBody, kind: str | None = Query(None), user: SiteUser = _USER,
) -> Response:
    """Render an unsaved design with sample data - the studio's live preview."""
    from app.images.service import render_png
    png = await render_png(body, kind=kind, sample=True)
    return Response(content=png, media_type="image/png",
                    headers={"Cache-Control": "no-store"})


@router.get("/v1/images/{design_id}")
async def get_image(design_id: str, user: SiteUser = _USER) -> dict:
    return await service.get_design(user, design_id)


@router.put("/v1/images/{design_id}")
async def update_image(design_id: str, body: DesignBody, user: SiteUser = _USER) -> dict:
    return await service.update_design(user, design_id, body)


@router.delete("/v1/images/{design_id}", status_code=204)
async def delete_image(design_id: str, user: SiteUser = _USER) -> Response:
    await service.delete_design(user, design_id)
    return Response(status_code=204)


# ── public render (no auth - Discord fetches this) ───────────────────────────

@router.get("/site/images/{design_id}.png")
async def render_image(
    design_id: str,
    kind: str | None = Query(None, description="Override the design's bound event type"),
    v: str | None = Query(None, description="Cache-buster (ignored server-side)"),
) -> Response:
    png = await service.render_public(design_id, kind)
    if png is None:
        raise APIError(404, ErrorCode.not_found, "Image not found.")
    return Response(content=png, media_type="image/png", headers=_PNG_HEADERS)
