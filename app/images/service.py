"""CRUD, image upload, variable binding, and cached rendering for image designs."""

from __future__ import annotations

import time

from beanie import PydanticObjectId

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.discord import embed_contexts
from app.images import render as render_mod
from app.images.models import (
    MAX_DESIGNS_PER_USER,
    MAX_DIM,
    MAX_LAYERS,
    MIN_DIM,
    ImageDesign,
)
from app.site_auth.models import SiteUser
from app.trove.mods_hub import store

SITE = "https://trove.aallyn.net"

# Small in-process render cache (mirrors og_image): render is cheap but Discord may
# fetch the same URL several times, so we collapse a burst into one render.
_CACHE: dict[str, tuple[float, bytes]] = {}
_CACHE_TTL = 60.0
_CACHE_MAX = 256


def _cache_get(key: str) -> bytes | None:
    hit = _CACHE.get(key)
    if hit and (time.time() - hit[0]) < _CACHE_TTL:
        return hit[1]
    return None


def _cache_put(key: str, png: bytes) -> None:
    if len(_CACHE) >= _CACHE_MAX:
        _CACHE.pop(min(_CACHE, key=lambda k: _CACHE[k][0]), None)
    _CACHE[key] = (time.time(), png)


# ── variable binding ─────────────────────────────────────────────────────────

def bindings() -> list[dict]:
    """The event/announcement kinds a design can bind to, with their variables +
    sample values (drives the studio's binding picker + palette + preview)."""
    return [
        {
            "key": k,
            "label": embed_contexts.KIND_LABELS.get(k, k),
            "variables": embed_contexts.variables(k),
            "sample": embed_contexts.sample_context(k),
            "has_image": embed_contexts.has_image(k),
        }
        for k in embed_contexts.KINDS
    ]


async def _resolve_context(bind_type: str | None) -> dict:
    if not bind_type or not embed_contexts.is_bindable(bind_type):
        return {}
    try:
        return await embed_contexts.context(bind_type)
    except Exception:
        return embed_contexts.sample_context(bind_type)


def _sample_context(bind_type: str | None) -> dict:
    if bind_type and embed_contexts.is_bindable(bind_type):
        return embed_contexts.sample_context(bind_type)
    return {}


# ── referenced blobs ─────────────────────────────────────────────────────────

async def _load_blobs(design: ImageDesign) -> dict[str, bytes]:
    shas = set()
    if design.background.type == "image" and design.background.image_sha:
        shas.add(design.background.image_sha)
    for layer in design.layers or []:
        if layer.type == "image" and layer.image_sha:
            shas.add(layer.image_sha)
    out: dict[str, bytes] = {}
    for sha in shas:
        data = await store.get_blob(sha)
        if data is not None:
            out[sha] = data
    return out


# ── render ───────────────────────────────────────────────────────────────────

async def render_png(design: ImageDesign, *, kind: str | None = None, sample: bool = False) -> bytes:
    """Render a design to PNG. ``kind`` overrides the design's ``bind_type`` for live
    variables; ``sample`` uses representative values (for the editor preview)."""
    bind = kind or design.bind_type
    if sample:
        ctx = _sample_context(bind)
        cache_key = ""                         # previews aren't cached (they change live)
    else:
        ctx = await _resolve_context(bind)
        sig = ctx.get("ends_at") or ctx.get("starts_at") or ""
        cache_key = f"{design.id}:{design.updated_at.timestamp()}:{bind}:{sig}:{int(time.time() // 60)}"
        cached = _cache_get(cache_key)
        if cached is not None:
            return cached
    blobs = await _load_blobs(design)
    png = render_mod.render(design, ctx, lambda sha: blobs.get(sha))
    if cache_key:
        _cache_put(cache_key, png)
    return png


# ── CRUD ─────────────────────────────────────────────────────────────────────

def _clamp(design: ImageDesign) -> None:
    design.width = max(MIN_DIM, min(MAX_DIM, int(design.width)))
    design.height = max(MIN_DIM, min(MAX_DIM, int(design.height)))
    design.layers = (design.layers or [])[:MAX_LAYERS]
    if design.bind_type and not embed_contexts.is_bindable(design.bind_type):
        design.bind_type = None


def _dto(d: ImageDesign) -> dict:
    out = d.model_dump(mode="json")
    out["id"] = str(d.id)
    out.pop("owner_id", None)
    out["render_url"] = f"{SITE}/site/images/{d.id}.png"
    return out


async def _owned(actor: SiteUser, design_id: str) -> ImageDesign:
    try:
        d = await ImageDesign.get(PydanticObjectId(design_id))
    except Exception:
        d = None
    if d is None or d.owner_id != actor.id:
        raise APIError(404, ErrorCode.not_found, "Image design not found.")
    return d


async def list_designs(actor: SiteUser) -> list[dict]:
    docs = await ImageDesign.find(ImageDesign.owner_id == actor.id).sort("-created_at").to_list()
    return [_dto(d) for d in docs]


async def get_design(actor: SiteUser, design_id: str) -> dict:
    return _dto(await _owned(actor, design_id))


async def create_design(actor: SiteUser, body) -> dict:
    count = await ImageDesign.find(ImageDesign.owner_id == actor.id).count()
    if count >= MAX_DESIGNS_PER_USER:
        raise APIError(400, ErrorCode.bad_request,
                       f"You can have at most {MAX_DESIGNS_PER_USER} image designs.")
    d = ImageDesign(owner_id=actor.id, **body.model_dump())
    _clamp(d)
    await d.insert()
    return _dto(d)


async def update_design(actor: SiteUser, design_id: str, body) -> dict:
    d = await _owned(actor, design_id)
    for k, v in body.model_dump().items():
        setattr(d, k, v)
    _clamp(d)
    d.updated_at = utcnow()
    await d.save()
    return _dto(d)


async def delete_design(actor: SiteUser, design_id: str) -> None:
    await (await _owned(actor, design_id)).delete()


async def render_public(design_id: str, kind: str | None = None) -> bytes | None:
    """Render a design by id with live data (no auth - the URL is what Discord fetches).
    Returns None if the design doesn't exist."""
    try:
        d = await ImageDesign.get(PydanticObjectId(design_id))
    except Exception:
        d = None
    if d is None:
        return None
    return await render_png(d, kind=kind)


# ── image upload (reuses the site blob store) ────────────────────────────────

async def upload_image(actor: SiteUser, data: bytes, declared_ct: str | None) -> dict:
    if len(data) > settings.mods_image_max_bytes:
        raise APIError(413, ErrorCode.bad_request,
                       f"Image exceeds the {settings.mods_image_max_bytes}-byte limit.")
    sniffed = store.sniff_image(data)
    if sniffed is None:
        raise APIError(400, ErrorCode.bad_request,
                       "Unsupported image - use PNG, JPEG, WebP or GIF.")
    content_type, w, h = sniffed
    sha, _ = await store.put_blob(data)
    return {"sha": sha, "content_type": content_type, "width": w, "height": h}
