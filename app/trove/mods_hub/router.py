"""HTTP surface for the Mods Hub: ``/v1/mods/hub/*``.

Two routers share the prefix:
  - ``mods_hub_router`` - PUBLIC reads, tokenless via ``public_scope("mods:read")``
    (in the OpenAPI reference). API consumers see only public/unlisted projects.
  - ``mods_hub_write_router`` - site-login-gated writes (``get_current_site_user``);
    mounted ``include_in_schema=False`` since they're driven by the website studio,
    not API-token developers.

The website's browse + owner-draft reveal goes through the same-origin
``/site/mods/*`` proxies in ``app/site/router.py`` (which pass the *site* user as
the viewer); these ``/v1`` reads always view as anonymous.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, File, Form, Query, Response, UploadFile

from app.core.dependencies import AccessContext, public_scope
from app.core.errors import COMMON_ERROR_RESPONSES, APIError, ErrorCode
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser
from app.trove import mod_categories
from app.trove.mods_hub import service
from app.trove.mods_hub.schemas import (
    ClaimRequest,
    CreateBranchRequest,
    CreateProjectRequest,
    CreateReleaseRequest,
    GitTokenRequest,
    HashLookupRequest,
    ReportRequest,
    UpdateProfileRequest,
    UpdateProjectRequest,
    UpdateReleaseRequest,
)

mods_hub_router = APIRouter(
    prefix="/v1/mods/hub", tags=["mods-hub"], responses=COMMON_ERROR_RESPONSES,
)
mods_hub_write_router = APIRouter(
    prefix="/v1/mods/hub", tags=["mods-hub"], responses=COMMON_ERROR_RESPONSES,
)
# The documented, app-facing catalog API (in the OpenAPI reference). Returns
# absolute image/download URLs so external apps can consume it directly.
mods_public_router = APIRouter(
    prefix="/v1/mods", tags=["mods"], responses=COMMON_ERROR_RESPONSES,
)

_PUB = Depends(public_scope("mods:read"))
_USER = Depends(get_current_site_user)

_IMMUTABLE = {"Cache-Control": "public, max-age=31536000, immutable"}


# ── public reads ──────────────────────────────────────────────────────────

@mods_hub_router.get("/projects")
async def list_projects(
    ctx: AccessContext = _PUB,
    q: str | None = Query(default=None, max_length=120),
    tag: str | None = Query(default=None, max_length=40),
    author: str | None = Query(default=None, max_length=80),
    sort: str = Query(default="recent"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """Browse published mods. Filter by ``q`` (text), ``tag`` or ``author``;
    ``sort`` ∈ recent | downloads | new | title."""
    items, total = await service.list_public(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return {"items": items, "count": len(items), "total": total}


@mods_hub_router.get("/tags")
async def list_tags(ctx: AccessContext = _PUB) -> dict:
    """Tag facets for the filter bar: ``{categories:[{tag,count}], custom:[{tag,count}]}``
    - the fixed categories first, then custom tags by descending count."""
    return await service.tag_facets()


@mods_hub_router.get("/profile/{handle}")
async def get_profile(handle: str, ctx: AccessContext = _PUB) -> dict:
    """A modder's public profile (customizations + their public mods)."""
    data = await service.profile_view(handle, None)
    if data is None:
        raise APIError(404, ErrorCode.not_found, "No such modder")
    return data


@mods_hub_router.get("/projects/{handle}/{slug}")
async def get_project(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    project = await service.get_for_view(handle, slug, None)
    return await service.project_detail(project, None)


@mods_hub_router.get("/projects/{handle}/{slug}/branches")
async def get_branches(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    return {"items": await service.list_branches(project)}


@mods_hub_router.get("/projects/{handle}/{slug}/commits")
async def get_commits(
    handle: str, slug: str, ctx: AccessContext = _PUB,
    branch: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
) -> dict:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    items, total = await service.list_commits(project, branch, limit, offset)
    return {"items": items, "count": len(items), "total": total}


@mods_hub_router.get("/projects/{handle}/{slug}/tree")
async def get_tree(
    handle: str, slug: str, ctx: AccessContext = _PUB, ref: str = Query(default=""),
) -> dict:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    return await service.get_tree(project, ref)


@mods_hub_router.get("/projects/{handle}/{slug}/raw/{commit_ref}/{path:path}")
async def get_raw_file(
    handle: str, slug: str, commit_ref: str, path: str, ctx: AccessContext = _PUB,
) -> Response:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    data = await service.get_file_bytes(project, commit_ref, path)
    return Response(content=data, media_type="application/octet-stream")


@mods_hub_router.get("/projects/{handle}/{slug}/compare")
async def compare_commits(
    handle: str, slug: str, ctx: AccessContext = _PUB,
    base: str = Query(...), head: str = Query(...),
) -> dict:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    return await service.compare(project, base, head)


@mods_hub_router.get("/projects/{handle}/{slug}/placement")
async def get_placement(
    handle: str, slug: str, ctx: AccessContext = _PUB, ref: str = Query(default=""),
) -> dict:
    """Check a commit's files against Trove's placement rules: what compiles,
    what's skipped (root / non-Trove folder / ignored type), and what's misplaced
    vs the game's structure (auto-fixable)."""
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    return await service.placement_report(project, ref)


@mods_hub_router.get("/projects/{handle}/{slug}/releases")
async def get_releases(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    project = await service.get_for_view(handle, slug, None)
    return {"items": await service.list_releases(
        project, include_drafts=False, include_hidden=False)}


@mods_hub_router.get("/projects/{handle}/{slug}/forks")
async def get_forks(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    project = await service.get_for_view(handle, slug, None)
    return {"items": await service.list_forks(project)}


@mods_hub_router.get("/releases/{release_id}")
async def get_release(release_id: str, ctx: AccessContext = _PUB) -> dict:
    release, project = await service.release_with_project(release_id, None)
    return {**service._release_dto(release), "project_slug": project.slug,
            "project_handle": project.owner_handle, "project_title": project.title}


@mods_hub_router.get("/releases/{release_id}/download")
async def download_release(release_id: str, ctx: AccessContext = _PUB) -> Response:
    """Download a release's compiled ``.tmod``. Public; bumps the download count."""
    release, project = await service.release_with_project(release_id, None)
    data = await service.record_download(release, project)
    return Response(
        content=data, media_type=service.release_media_type(release),
        headers={"Content-Disposition":
                 f'attachment; filename="{service.release_download_filename(release)}"'},
    )


@mods_hub_router.get("/image/{sha}")
async def get_image(sha: str, ctx: AccessContext = _PUB) -> Response:
    got = await service.get_image(sha)
    if got is None:
        raise APIError(404, ErrorCode.not_found, "Image not found")
    data, content_type = got
    return Response(content=data, media_type=content_type, headers=_IMMUTABLE)


# ── site-login writes ─────────────────────────────────────────────────────

async def _require_owned(handle: str, slug: str, user: SiteUser) -> service.ModProject:
    """Load a project the caller owns, or 404 (uniform - never leaks existence
    of someone else's draft via a 403)."""
    project = await service.get_project(handle, slug)
    if project is None or project.owner_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Mod project not found")
    return project


def _valid_status(status: str) -> str:
    if status not in ("draft", "published"):
        raise APIError(400, ErrorCode.bad_request, "status must be draft or published")
    return status


@mods_hub_write_router.get("/me/projects")
async def my_projects(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_owned(user)}


@mods_hub_write_router.get("/me/starred")
async def my_starred(user: SiteUser = _USER) -> dict:
    return {"items": await service.list_starred(user)}


@mods_hub_write_router.patch("/me/profile")
async def update_my_profile(req: UpdateProfileRequest, user: SiteUser = _USER) -> dict:
    """Edit the signed-in modder's profile page (`/mods/<handle>`)."""
    return await service.update_profile(user, **req.model_dump(exclude_unset=True))


@mods_hub_write_router.post("/me/profile/avatar")
async def upload_my_avatar(file: UploadFile = File(...), user: SiteUser = _USER) -> dict:
    """Set a custom profile picture."""
    asset = await service.store_image(user, await file.read(), file.content_type)
    return await service.set_profile_image(user, asset.sha, banner=False)


@mods_hub_write_router.post("/me/profile/banner")
async def upload_my_profile_banner(file: UploadFile = File(...), user: SiteUser = _USER) -> dict:
    """Set the profile banner image."""
    asset = await service.store_image(user, await file.read(), file.content_type)
    return await service.set_profile_image(user, asset.sha, banner=True)


@mods_hub_write_router.get("/me/git-tokens")
async def list_git_tokens(user: SiteUser = _USER) -> dict:
    """The caller's active git access tokens (no plaintext - that's shown once)."""
    return {"items": await service.list_git_tokens(user)}


@mods_hub_write_router.post("/me/git-tokens", status_code=201)
async def create_git_token(req: GitTokenRequest, user: SiteUser = _USER) -> dict:
    """Mint a git access token. Paste it as the git *password* (any username) to
    clone/pull/push. The plaintext is returned ONCE - it can't be retrieved later."""
    dto, raw = await service.create_git_token(user, req.name)
    return {**dto, "token": raw}


@mods_hub_write_router.delete("/me/git-tokens/{token_id}", status_code=204)
async def revoke_git_token(token_id: str, user: SiteUser = _USER) -> Response:
    await service.revoke_git_token(user, token_id)
    return Response(status_code=204)


@mods_hub_write_router.post("/projects", status_code=201)
async def create_project(req: CreateProjectRequest, user: SiteUser = _USER) -> dict:
    project = await service.create_project(
        user, title=req.title, summary=req.summary, description=req.description,
        tags=req.tags, visibility=req.visibility, mode=req.mode,
        source_visibility=req.source_visibility, inspired_by=req.inspired_by,
    )
    return await service.project_detail(project, user)


@mods_hub_write_router.post("/projects/{handle}/{slug}/fork", status_code=201)
async def fork_project(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """Fork a mod into a new project of your own, copying its current files and
    crediting the original. The source must be viewable (public/unlisted/yours)."""
    original = await service.get_for_view(handle, slug, user)
    fork = await service.fork_project(user, original)
    return await service.project_detail(fork, user)


@mods_hub_write_router.patch("/projects/{handle}/{slug}")
async def update_project(
    handle: str, slug: str, req: UpdateProjectRequest, user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    project = await service.update_project(
        project, user, **req.model_dump(exclude_unset=True),
    )
    return await service.project_detail(project, user)


@mods_hub_write_router.delete("/projects/{handle}/{slug}", status_code=204)
async def delete_project(handle: str, slug: str, user: SiteUser = _USER) -> Response:
    project = await _require_owned(handle, slug, user)
    await service.delete_project(project, user)
    return Response(status_code=204)


@mods_hub_write_router.post("/projects/{handle}/{slug}/banner")
async def upload_banner(
    handle: str, slug: str, file: UploadFile = File(...), user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    asset = await service.store_image(user, await file.read(), file.content_type)
    project = await service.set_banner(project, user, asset.sha)
    return {"banner_sha": project.banner_sha}


@mods_hub_write_router.post("/projects/{handle}/{slug}/previews")
async def upload_previews(
    handle: str, slug: str, files: list[UploadFile] = File(...), user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    for f in files:
        asset = await service.store_image(user, await f.read(), f.content_type)
        project = await service.add_preview(project, user, asset.sha)
    return {"preview_shas": project.preview_shas}


@mods_hub_write_router.delete("/projects/{handle}/{slug}/previews/{sha}")
async def delete_preview(handle: str, slug: str, sha: str, user: SiteUser = _USER) -> dict:
    project = await _require_owned(handle, slug, user)
    project = await service.remove_preview(project, user, sha)
    return {"preview_shas": project.preview_shas}


@mods_hub_write_router.post("/projects/{handle}/{slug}/branches", status_code=201)
async def create_branch(
    handle: str, slug: str, req: CreateBranchRequest, user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    return await service.create_branch(project, user, req.name, req.from_ref)


@mods_hub_write_router.delete("/projects/{handle}/{slug}/branches/{name}", status_code=204)
async def delete_branch(handle: str, slug: str, name: str, user: SiteUser = _USER) -> Response:
    project = await _require_owned(handle, slug, user)
    await service.delete_branch(project, user, name)
    return Response(status_code=204)


@mods_hub_write_router.post("/projects/{handle}/{slug}/commits", status_code=201)
async def create_commit(
    handle: str, slug: str,
    branch: str = Form(...),
    message: str = Form(...),
    files: list[UploadFile] = File(default=[]),
    paths: list[str] = Form(default=[]),
    deletes: list[str] = Form(default=[]),
    user: SiteUser = _USER,
) -> dict:
    """Commit a set of file changes. ``files`` carry the bytes; the parallel
    ``paths`` give each file's mod-internal path (falls back to the upload
    filename). ``deletes`` removes paths from the parent tree."""
    project = await _require_owned(handle, slug, user)
    adds: list[tuple[str, bytes]] = []
    for i, f in enumerate(files):
        path = paths[i] if i < len(paths) else (f.filename or f"file-{i}")
        adds.append((path, await f.read()))
    return await service.commit_files(
        project, user, branch_name=branch, message=message,
        adds=adds, deletes=deletes,
    )


@mods_hub_write_router.post("/projects/{handle}/{slug}/releases", status_code=201)
async def create_release(
    handle: str, slug: str, req: CreateReleaseRequest, user: SiteUser = _USER,
) -> dict:
    """Cut a release by compiling a commit's file tree server-side into the
    chosen ``format`` (``tmod`` or ``zip``)."""
    project = await _require_owned(handle, slug, user)
    return await service.create_release_from_commit(
        project, user, tag=req.tag, title=req.title, changelog=req.changelog,
        ref=req.ref, status=req.status, fmt=req.format, preview_sha=req.preview_sha,
        author=req.author,
    )


@mods_hub_write_router.post("/projects/{handle}/{slug}/releases/upload", status_code=201)
async def upload_release(
    handle: str, slug: str,
    tag: str = Form(...),
    title: str = Form(default=""),
    changelog: str = Form(default=""),
    status: str = Form(default="published"),
    branch: str = Form(default=""),
    file: UploadFile = File(...),
    user: SiteUser = _USER,
) -> dict:
    """Cut a release from an already-built ``.tmod`` or ``.zip`` upload (validated
    on ingest). ``branch`` tags it to a variant (defaults to the default branch)."""
    project = await _require_owned(handle, slug, user)
    return await service.create_release_from_upload(
        project, user, tag=tag, title=title, changelog=changelog,
        status=_valid_status(status), branch=branch,
        filename=file.filename or "mod.tmod", data=await file.read(),
    )


@mods_hub_write_router.patch("/releases/{release_id}")
async def update_release(
    release_id: str, req: UpdateReleaseRequest, user: SiteUser = _USER,
) -> dict:
    release = await service.get_release(release_id)
    if release is None:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    project = await service.get_project_by_id(release.project_id)
    if project is None or project.owner_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    return await service.update_release(
        release, project, user, **req.model_dump(exclude_unset=True),
    )


@mods_hub_write_router.delete("/releases/{release_id}", status_code=204)
async def delete_release(release_id: str, user: SiteUser = _USER) -> Response:
    release = await service.get_release(release_id)
    if release is None:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    project = await service.get_project_by_id(release.project_id)
    if project is None or project.owner_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    await service.delete_release(release, project, user)
    return Response(status_code=204)


@mods_hub_write_router.post("/projects/{handle}/{slug}/fix-placement", status_code=201)
async def fix_placement(
    handle: str, slug: str, branch: str = Query(default=""), user: SiteUser = _USER,
) -> dict:
    """Move every misplaced file to the game's path for it, as one new commit."""
    project = await _require_owned(handle, slug, user)
    return await service.fix_placement(project, user, branch)


@mods_hub_write_router.post("/projects/{handle}/{slug}/star")
async def star_project(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """Star (favourite) a mod. Idempotent; returns the new ``{starred, star_count}``."""
    project = await service.get_for_view(handle, slug, user)
    return await service.star_project(user, project)


@mods_hub_write_router.delete("/projects/{handle}/{slug}/star")
async def unstar_project(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    project = await service.get_for_view(handle, slug, user)
    return await service.unstar_project(user, project)


@mods_hub_write_router.post("/projects/{handle}/{slug}/report", status_code=202)
async def report_project(
    handle: str, slug: str, req: ReportRequest, user: SiteUser = _USER,
) -> dict:
    project = await service.get_for_view(handle, slug, user)
    await service.report_project(project, user, req.reason)
    return {"status": "received"}


@mods_hub_write_router.post("/projects/{handle}/{slug}/claim", status_code=202)
async def claim_project(
    handle: str, slug: str, req: ClaimRequest, user: SiteUser = _USER,
) -> dict:
    """Request to claim a *stray* (imported, unowned) mod as your own. Goes to a
    master for approval; on approval the mod is handed over to you."""
    project = await service.get_for_view(handle, slug, user)
    return await service.create_claim(project, user, req.message)


# ── public catalog API (documented; /v1/mods/*) ─────────────────────────────
# Fixed paths (/popular, /lookup) are declared before /{slug} so they aren't
# captured as a slug. The website-internal hub lives on a separate, longer-path
# router (/v1/mods/hub/*), so there's no collision.

@mods_public_router.get("")
async def list_mods(
    ctx: AccessContext = _PUB,
    q: str | None = Query(default=None, max_length=120,
                          description="Full-text search over title / summary / tags."),
    tag: str | None = Query(default=None, max_length=40, description="Filter by an exact tag."),
    author: str | None = Query(default=None, max_length=80, description="Filter by author username."),
    sort: str = Query(default="recent",
                      description="recent | popular | downloads | stars | new | title"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """List published mods (cards, no releases). Filter by text / tag / author and
    sort; use ``sort=popular`` for the trailing-7-day popularity ranking."""
    items, total = await service.public_list(
        q=q, tag=tag, author=author, sort=sort, limit=limit, offset=offset,
    )
    return {"items": items, "count": len(items), "total": total,
            "limit": limit, "offset": offset}


@mods_public_router.get("/popular")
async def popular_mods(
    ctx: AccessContext = _PUB,
    limit: int = Query(default=25, ge=1, le=25, description="How many to return (max 25)."),
) -> dict:
    """The most popular mods by a 0.0-1.0 popularity score derived mainly from
    downloads in the last 7 days. Returns up to 25."""
    items = await service.public_popular(limit=limit)
    return {"items": items, "count": len(items)}


@mods_public_router.post("/lookup")
async def lookup_mods(req: HashLookupRequest, ctx: AccessContext = _PUB) -> dict:
    """Resolve mod + release metadata from one or more artifact content hashes
    (sha256 hex). ``results`` is keyed by hash; ``unknown`` lists the hashes with
    no public match. Useful for an app to identify installed .tmod files."""
    return await service.lookup_by_hashes(req.hashes)


@mods_public_router.get("/categories")
async def mod_category_vocab(ctx: AccessContext = _PUB) -> dict:
    """The fixed mod-category vocabulary. Each category owns one bit; a mod's
    selected categories are stored as tags AND encoded as a ``flags`` bitmask
    (sum of the bits) on its compiled ``.tmod``. ``flags & bit`` tests membership;
    ``flags`` of 0 means none."""
    return {"categories": mod_categories.categories()}


@mods_public_router.get("/{handle}/{slug}")
async def get_mod(handle: str, slug: str, ctx: AccessContext = _PUB) -> dict:
    """Full metadata + published releases for one mod (404 if it isn't public)."""
    return await service.public_detail(handle, slug)
