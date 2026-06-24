"""HTTP surface for Modpacks: ``/v1/modpacks/hub/*``.

Two routers share the prefix, mirroring the Mods Hub:
  - ``modpacks_hub_router`` - PUBLIC reads + downloads, tokenless via
    ``public_scope("mods:read")``.
  - ``modpacks_hub_write_router`` - site-login-gated writes
    (``get_current_site_user``); driven by the website studio.

The website's browse + owner-draft reveal goes through the same-origin
``/site/modpacks/*`` proxies (which pass the *site* user as viewer); these ``/v1``
reads always view as anonymous. Banners reuse the Mods Hub image store/serving.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, File, Query, Response, UploadFile

from app.core.dependencies import AccessContext, public_scope
from app.core.errors import COMMON_ERROR_RESPONSES, APIError, ErrorCode
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser
from app.trove.mods_hub import service as mods_service
from app.trove.modpacks import service
from app.trove.modpacks.models import ModpackProject
from app.trove.modpacks.schemas import (
    CreateModpackRequest,
    CreateVariantRequest,
    SetEntriesRequest,
    UpdateModpackRequest,
    UpdateVariantRequest,
)

modpacks_hub_router = APIRouter(
    prefix="/v1/modpacks/hub", tags=["modpacks-hub"], responses=COMMON_ERROR_RESPONSES,
)
modpacks_hub_write_router = APIRouter(
    prefix="/v1/modpacks/hub", tags=["modpacks-hub"], responses=COMMON_ERROR_RESPONSES,
)
# The documented, app-facing catalog API (in the OpenAPI reference). Returns
# absolute image / page / download URLs so external apps can consume it directly.
modpacks_public_router = APIRouter(
    prefix="/v1/modpacks", tags=["modpacks"], responses=COMMON_ERROR_RESPONSES,
)

_PUB = Depends(public_scope("mods:read"))
_USER = Depends(get_current_site_user)


async def _require_owned(handle: str, slug: str, user: SiteUser) -> ModpackProject:
    """Load a modpack the caller owns, or 404 (uniform - never leaks existence of
    someone else's draft via a 403)."""
    pack = await service.get_pack(handle, slug)
    if pack is None or pack.owner_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Modpack not found")
    return pack


# ── public reads ───────────────────────────────────────────────────────────

@modpacks_hub_router.get("/projects")
async def list_modpacks(
    ctx: AccessContext = _PUB,
    q: str | None = Query(default=None, max_length=120),
    tag: str | None = Query(default=None, max_length=40),
    author: str | None = Query(default=None, max_length=80),
    sort: str = Query(default="recent"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Browse public modpacks. ``sort`` ∈ recent | downloads | new | title."""
    items, total = await service.list_public(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return {"items": items, "count": len(items), "total": total}


@modpacks_hub_router.get("/projects/{handle}/{slug}")
async def get_modpack(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    pack = await service.get_for_view(handle, slug, None)
    return await service.pack_detail(pack, None)


@modpacks_hub_router.get("/projects/{handle}/{slug}/download")
async def download_modpack(
    handle: str, slug: str, ctx: AccessContext = _PUB,
    variant: str | None = Query(default=None, max_length=80),
    format: str = Query(default="tpack", pattern="^(tpack|zip)$"),
) -> Response:
    """Download a modpack variant: a ``.tpack`` (default) or a ``.zip``. Public;
    bumps the download count. Unlocked entries resolve to the latest build."""
    pack = await service.get_for_view(handle, slug, None)
    blob, filename, media = await service.build_artifact(pack, variant, format)
    await service.record_download(pack)
    return Response(
        content=blob, media_type=media,
        headers={"Content-Disposition": f'attachment; filename="{filename}"'},
    )


# ── owner listing + writes (site login) ─────────────────────────────────────

@modpacks_hub_write_router.get("/me/projects")
async def list_my_modpacks(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_owned(user)}


@modpacks_hub_write_router.post("/projects", status_code=201)
async def create_modpack(req: CreateModpackRequest, user: SiteUser = _USER) -> dict:
    pack = await service.create_modpack(
        user, title=req.title, summary=req.summary, description=req.description,
        tags=req.tags, visibility=req.visibility,
    )
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.patch("/projects/{handle}/{slug}")
async def update_modpack(
    handle: str, slug: str, req: UpdateModpackRequest, user: SiteUser = _USER,
) -> dict:
    pack = await _require_owned(handle, slug, user)
    pack = await service.update_modpack(pack, user, **req.model_dump(exclude_unset=True))
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.delete("/projects/{handle}/{slug}", status_code=204)
async def delete_modpack(handle: str, slug: str, user: SiteUser = _USER) -> Response:
    pack = await _require_owned(handle, slug, user)
    await service.delete_modpack(pack, user)
    return Response(status_code=204)


@modpacks_hub_write_router.post("/projects/{handle}/{slug}/banner")
async def upload_banner(
    handle: str, slug: str, file: UploadFile = File(...), user: SiteUser = _USER,
) -> dict:
    pack = await _require_owned(handle, slug, user)
    asset = await mods_service.store_image(user, await file.read(), file.content_type)
    pack = await service.set_banner(pack, user, asset.sha)
    return {"banner_sha": pack.banner_sha}


# ── variants ────────────────────────────────────────────────────────────────

@modpacks_hub_write_router.post("/projects/{handle}/{slug}/variants", status_code=201)
async def add_variant(
    handle: str, slug: str, req: CreateVariantRequest, user: SiteUser = _USER,
) -> dict:
    pack = await _require_owned(handle, slug, user)
    pack = await service.add_variant(pack, user, name=req.name, copy_from=req.copy_from)
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.patch("/projects/{handle}/{slug}/variants/{name}")
async def update_variant(
    handle: str, slug: str, name: str, req: UpdateVariantRequest, user: SiteUser = _USER,
) -> dict:
    pack = await _require_owned(handle, slug, user)
    pack = await service.update_variant(pack, user, name, **req.model_dump(exclude_unset=True))
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.delete("/projects/{handle}/{slug}/variants/{name}")
async def delete_variant(handle: str, slug: str, name: str, user: SiteUser = _USER) -> dict:
    pack = await _require_owned(handle, slug, user)
    pack = await service.delete_variant(pack, user, name)
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.put("/projects/{handle}/{slug}/variants/{name}/entries")
async def set_entries(
    handle: str, slug: str, name: str, req: SetEntriesRequest, user: SiteUser = _USER,
) -> dict:
    pack = await _require_owned(handle, slug, user)
    pack = await service.set_entries(pack, user, name, req.entries)
    return await service.pack_detail(pack, user)


@modpacks_hub_write_router.post("/projects/{handle}/{slug}/star")
async def star_modpack(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """Like (favourite) a modpack. Idempotent; returns ``{starred, star_count}``."""
    pack = await service.get_for_view(handle, slug, user)
    return await service.star_pack(user, pack)


@modpacks_hub_write_router.delete("/projects/{handle}/{slug}/star")
async def unstar_modpack(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    pack = await service.get_for_view(handle, slug, user)
    return await service.unstar_pack(user, pack)


# ── public catalog API (documented; /v1/modpacks/*) ─────────────────────────
# App-facing surface with absolute URLs. A modpack is addressed by <handle>/<slug>.
# The list path "" is declared first; /{handle}/{slug} and its /download follow.

@modpacks_public_router.get("")
async def list_modpacks_public(
    ctx: AccessContext = _PUB,
    q: str | None = Query(default=None, max_length=120,
                          description="Full-text search over title / summary / tags."),
    tag: str | None = Query(default=None, max_length=40, description="Filter by an exact tag."),
    author: str | None = Query(default=None, max_length=80, description="Filter by author username."),
    sort: str = Query(default="recent", description="recent | downloads | new | title"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """List public modpacks (cards, no mod lists). Filter by text / tag / author and
    sort. Each card carries absolute ``banner_url`` + ``page_url``."""
    items, total = await service.public_list(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return {"items": items, "count": len(items), "total": total,
            "limit": limit, "offset": offset}


@modpacks_public_router.get("/for-mod/{handle}/{slug}")
async def modpacks_for_mod_public(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    """Public modpacks that include the given mod (by ``<handle>/<slug>``) - the
    backlink from a mod to the packs that bundle it. Empty list if none/unknown."""
    items = await service.public_packs_for_mod(handle, slug)
    return {"items": items, "count": len(items)}


@modpacks_public_router.get("/{handle}/{slug}")
async def get_modpack_public(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    """Full metadata for one modpack: every variant + the mods (and the version each
    resolves to) it bundles, with absolute ``download_url``/``zip_url`` per variant.
    404 if the pack isn't public."""
    return await service.public_detail(handle, slug)


@modpacks_public_router.get("/{handle}/{slug}/download")
async def download_modpack_public(
    handle: str, slug: str, ctx: AccessContext = _PUB,
    variant: str | None = Query(default=None, max_length=80,
                                description="Which variant to download; defaults to the pack's default."),
    format: str = Query(default="tpack", pattern="^(tpack|zip)$",
                        description="tpack (a .tmod-style bundle, default) or zip."),
) -> Response:
    """Download a modpack variant as a ``.tpack`` (default) or ``.zip``. Built on the
    fly; unlocked mods resolve to their latest published build. Bumps the count."""
    pack = await service.get_for_view(handle, slug, None)
    blob, filename, media = await service.build_artifact(pack, variant, format)
    await service.record_download(pack)
    return Response(
        content=blob, media_type=media,
        headers={"Content-Disposition": f'attachment; filename="{filename}"'},
    )
