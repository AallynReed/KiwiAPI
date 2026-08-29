"""HTTP surface for the Mods Hub: ``/v1/mods/hub/*``.

Two routers share the prefix:
  - ``mods_hub_router`` - PUBLIC reads, tokenless via ``public_scope("mods:read")``
    (in the OpenAPI reference). API consumers see only public/unlisted projects.
  - ``mods_hub_write_router`` - writes, gated by ``get_mod_write_user``: either the
    creator's own Dashboard session, or a dev-portal API token with ``mods:write``
    whose account the creator connected (see ``creators.py`` / ``write_auth.py``).
    Either way the route body receives the creator's ``SiteUser`` and behaves
    identically. Mounted ``include_in_schema=False`` - the website studio is the
    primary driver and the API contract is documented in the guide.

The website's browse + owner-draft reveal goes through the same-origin
``/site/mods/*`` proxies in ``app/site/router.py`` (which pass the *site* user as
the viewer); these ``/v1`` reads always view as anonymous.
"""

from __future__ import annotations

import base64
import binascii

from fastapi import APIRouter, Depends, File, Form, Query, Request, Response, UploadFile

from app.auth.models import User
from app.core.config import settings
from app.core.dependencies import AccessContext, get_current_user, public_scope
from app.core.errors import COMMON_ERROR_RESPONSES, APIError, ErrorCode
from app.core.features import require_mod_issues_enabled
from app.site_auth.models import SiteUser
from app.trove import mod_categories
from app.trove.mods_hub import creators, issues, service, store
from app.trove.mods_hub.schemas import (
    ClaimRequest,
    CollaboratorRequest,
    CreateBranchRequest,
    CreateIssueRequest,
    CreateProjectRequest,
    CreateReleaseRequest,
    CreatorLinkRequest,
    GitTokenRequest,
    HashLookupRequest,
    IssueCommentRequest,
    IssueStatusRequest,
    UpdateProfileRequest,
    UpdateProjectRequest,
    UpdateReleaseRequest,
)
from app.trove.mods_hub.write_auth import get_mod_write_user
from app.trove.render import bp_cache
from app.trove.swf import service as swf_service

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
# ── the "creators" surface (documented in the API reference) ───────────────
# Everything a *connected API account* can do, split off from the website-only
# writes so the reference divides read-anything endpoints from control ones. This
# router's route set IS the API allowlist in write_auth._API_ROUTES - a test
# asserts the two agree, so a route can't be documented as callable without
# actually being callable, or vice versa.
mods_creator_write_router = APIRouter(
    prefix="/v1/mods/hub", tags=["creators"], responses=COMMON_ERROR_RESPONSES,
)
# Creator connections, managed from the DEV PORTAL, so these authenticate with the
# portal's session JWT (not an API token): they're account plumbing, like /tokens.
mods_creator_router = APIRouter(
    prefix="/v1/mods/hub/creator-links", tags=["creators"],
    responses=COMMON_ERROR_RESPONSES,
)

# Browsing the hub is a FAN-OUT, not one call: a page of cards, then per-card
# detail / releases / file lists. At the shared 30/min anon cap page one already
# 429'd, so the whole browse surface meters in its own widened bucket.
_PUB = Depends(public_scope(
    "mods:read", rate_multiplier=settings.mods_rate_limit_multiplier,
    bucket="mods:browse",
))
# Images and downloads burst harder still (one <img> per card, one install pulling
# a .tmod plus its previews) and are pure static blob serving, so they get a
# wider bucket of their OWN - a thumbnail grid must not starve the browse calls.
_ASSET = Depends(public_scope(
    "mods:read", rate_multiplier=settings.mods_asset_rate_limit_multiplier,
    bucket="mods:asset",
))
# Writes: the creator's Dashboard session, or a connected API account's token
# carrying mods:write. Both resolve to the creator's SiteUser - see write_auth.py.
_USER = Depends(get_mod_write_user)
_PORTAL = Depends(get_current_user)

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
    handle: str, slug: str, commit_ref: str, path: str, ctx: AccessContext = _ASSET,
) -> Response:
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    data = await service.get_file_bytes(project, commit_ref, path)
    return Response(content=data, media_type="application/octet-stream")


@mods_hub_router.get("/projects/{handle}/{slug}/archive")
async def get_source_archive(
    handle: str, slug: str, ctx: AccessContext = _ASSET, ref: str = Query(default=""),
) -> Response:
    """Download a commit's whole file tree as a ``.zip`` (source, not a build)."""
    project = await service.get_for_view(handle, slug, None)
    service.ensure_source_visible(project, None)
    filename, data = await service.source_archive(project, ref)
    return Response(content=data, media_type="application/zip", headers={
        "Content-Disposition": f'attachment; filename="{filename}"'})


# ── issues & requests ─────────────────────────────────────────────────────
# Reads are public (the threads are part of the mod's page); writes need a
# Dashboard session and live on the website-only write router below. Both sides
# 404 when the site-wide switch is off OR the creator turned issues off for the
# mod - "not taking issues" should look the same as "never had issues".

_ISSUES = [Depends(require_mod_issues_enabled)]


@mods_hub_router.get("/projects/{handle}/{slug}/issues", dependencies=_ISSUES)
async def get_issues(
    handle: str, slug: str, ctx: AccessContext = _PUB,
    status: str = Query(default="open", pattern="^(open|closed|all)$"),
    limit: int = Query(default=30, ge=1, le=100),
    offset: int = Query(default=0, ge=0),
) -> dict:
    """The mod's issues + requests, newest activity first."""
    project = await service.get_for_view(handle, slug, None)
    return await issues.list_issues(
        project, None, status=status, limit=limit, offset=offset)


@mods_hub_router.get("/projects/{handle}/{slug}/issues/{number}", dependencies=_ISSUES)
async def get_issue(handle: str, slug: str, number: int, ctx: AccessContext = _PUB) -> dict:
    """One thread with its full timeline (replies + close/reopen records)."""
    project = await service.get_for_view(handle, slug, None)
    return await issues.get_issue(project, number, None)


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
async def download_release(release_id: str, ctx: AccessContext = _ASSET) -> Response:
    """Download a release's compiled ``.tmod``. Public; bumps the download count."""
    release, project = await service.release_with_project(release_id, None)
    data = await service.record_download(release, project)
    return Response(
        content=data, media_type=service.release_media_type(release),
        headers={"Content-Disposition":
                 f'attachment; filename="{service.release_download_filename(release)}"'},
    )


@mods_hub_router.get("/releases/{release_id}/blueprints")
async def get_release_blueprints(release_id: str, ctx: AccessContext = _PUB) -> dict:
    """List the ``.blueprint`` model files inside a release + whether they assemble
    onto a known creature rig (`rig`, `animations`), for the web 3D viewer."""
    release, _ = await service.release_with_project(release_id, None)
    return await service.list_release_blueprints(release)


@mods_hub_router.get(
    "/releases/{release_id}/assembled",
    responses={200: {"content": {"application/json": {}},
                     "description": "Assembled creature: rest pose + animation metadata."}},
)
async def get_release_assembled(
    request: Request, release_id: str,
    fmt: str = Query(default="json", pattern="^(json|bin)$",
                     description="`json` (default) or `bin` - the compact KVX1 binary container."),
    ctx: AccessContext = _ASSET,
) -> Response:
    """The release's blueprint parts assembled onto their creature rig (rest pose +
    animations) as the web-viewer model payload.

    Assembled once per artifact and cached, then served gzipped with an ``ETag`` -
    send ``If-None-Match`` to skip the transfer on a repeat fetch."""
    release, _ = await service.release_with_project(release_id, None)
    model = await service.assemble_release_model(release, fmt)
    if model is None:
        raise APIError(404, ErrorCode.not_found, "No assemblable creature for this mod.")
    return bp_cache.respond(request, model)


@mods_hub_router.get(
    "/releases/{release_id}/blueprint",
    responses={200: {"content": {"application/json": {}},
                     "description": "Voxel payload: parallel x/y/z/rgb/kind/level"
                                    " (+spec) arrays."}},
)
async def get_release_blueprint(
    request: Request, release_id: str,
    path: str = Query(..., min_length=1, max_length=400),
    fmt: str = Query(default="json", pattern="^(json|bin)$",
                     description="`json` (default) or `bin` - the compact KVX1 binary container."),
    ctx: AccessContext = _ASSET,
) -> Response:
    """Decoded voxel data for one ``.blueprint`` in a release (web 3D viewer).

    Decoded once per artifact and cached, then served gzipped with an ``ETag`` -
    send ``If-None-Match`` to skip the transfer entirely on a repeat fetch."""
    release, _ = await service.release_with_project(release_id, None)
    cached = await service.decode_release_blueprint(release, path, fmt)
    return bp_cache.respond(request, cached)


@mods_hub_router.get("/rigs/{skeleton}/anim/{name}", response_class=Response)
async def get_rig_animation(skeleton: str, name: str, ctx: AccessContext = _ASSET) -> Response:
    """Baked animation clip for a creature rig, lazily (the assembled-model payload
    carries only animation metadata; the viewer fetches clips on demand). Binary
    ``TANIM1``: an 8-byte magic, then ``ap_count``/``frame_count``/``fps``/name-blob
    length as u32, the NUL-separated attach-point keys, then
    ``frame_count x ap_count x 7`` float32 - position xyz then quaternion xyzw."""
    data = await service.load_rig_animation(skeleton, name)
    return Response(content=data, media_type="application/octet-stream",
                    headers={"Cache-Control": "public, max-age=3600"})


@mods_hub_router.get("/releases/{release_id}/files")
async def get_release_files(release_id: str, ctx: AccessContext = _PUB) -> dict:
    """The files inside a release's .tmod (path + size), excluding the preview image."""
    release, _ = await service.release_with_project(release_id, None)
    return await service.list_release_files(release)


@mods_hub_router.get("/releases/{release_id}/file")
async def get_release_file(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    ctx: AccessContext = _ASSET,
) -> Response:
    """Download one file from inside a release's .tmod (preview excluded)."""
    release, _ = await service.release_with_project(release_id, None)
    data, filename = await service.download_release_file(release, path)
    safe = filename.replace('"', '').replace("\r", "").replace("\n", "")
    return Response(content=data, media_type="application/octet-stream",
                    headers={"Content-Disposition": f'attachment; filename="{safe}"'})


@mods_hub_router.get("/releases/{release_id}/swfs")
async def get_release_swfs(release_id: str, ctx: AccessContext = _PUB) -> dict:
    """The ``.swf`` Flash movies packed inside a release's .tmod (path + size), plus
    ``decompiler``: whether this server can turn one back into ActionScript.
    Header-only read; nothing is decompressed."""
    release, _ = await service.release_with_project(release_id, None)
    return await service.list_release_swfs(release)


@mods_hub_router.get("/releases/{release_id}/swf/scripts")
async def get_release_swf_scripts(
    request: Request, release_id: str,
    path: str = Query(..., min_length=1, max_length=400),
    ctx: AccessContext = _PUB,
) -> dict:
    """One packed ``.swf`` decompiled: every ActionScript class in the movie as
    source, keyed by the package path it came from.

    An interface mod's behaviour lives entirely in this bytecode, so this is what
    makes a mod readable before you install it. Decompilation is cached under the
    movie's own content hash - the first call on a cold movie costs a second or
    two, every later one is served from the cache."""
    await swf_service.decompile_throttle(request)
    release, _ = await service.release_with_project(release_id, None)
    return await service.release_swf_scripts(release, path)


@mods_hub_router.get("/releases/{release_id}/inspect")
async def inspect_release(release_id: str, ctx: AccessContext = _PUB) -> dict:
    """A release's artifact decoded: the ``.tmod`` header (version + every property +
    decoded categories) and every packed file with its size - what the mod page's
    build inspector shows. Header-only read; nothing is decompressed."""
    release, _ = await service.release_with_project(release_id, None)
    return await service.inspect_release(release)


@mods_hub_router.get("/releases/{release_id}/cfgs")
async def get_release_cfgs(release_id: str, ctx: AccessContext = _PUB) -> dict:
    """The ``.cfg`` config files packed inside a release's .tmod (path, size and the
    name to save each under) - a mod's config is needed on its own, in the game's
    ModCfgs folder."""
    release, _ = await service.release_with_project(release_id, None)
    return await service.list_release_cfgs(release)


@mods_hub_router.get("/releases/{release_id}/cfg")
async def get_release_cfg(
    release_id: str, path: str = Query(..., min_length=1, max_length=400),
    ctx: AccessContext = _ASSET,
) -> Response:
    """Download one packed ``.cfg``, extracted from the .tmod on the fly."""
    release, _ = await service.release_with_project(release_id, None)
    data, filename = await service.download_release_cfg(release, path)
    safe = filename.replace('"', '').replace("\r", "").replace("\n", "")
    return Response(content=data, media_type="text/plain; charset=utf-8",
                    headers={"Content-Disposition": f'attachment; filename="{safe}"'})


@mods_hub_router.get("/image/{sha}")
async def get_image(sha: str, w: int | None = Query(default=None, description="Downscale to this width (WebP)"),
                    ctx: AccessContext = _ASSET) -> Response:
    if w is not None and w not in store.THUMB_WIDTHS:
        raise APIError(400, ErrorCode.bad_request,
                       f"w must be one of {', '.join(map(str, store.THUMB_WIDTHS))}")
    got = await service.get_image(sha, w)
    if got is None:
        raise APIError(404, ErrorCode.not_found, "Image not found")
    data, content_type = got
    return Response(content=data, media_type=content_type, headers=_IMMUTABLE)


# ── site-login writes ─────────────────────────────────────────────────────

async def _require_owned(handle: str, slug: str, user: SiteUser) -> service.ModProject:
    """Load a project the caller can edit (owner OR collaborator), or 404 (uniform -
    never leaks existence of someone else's draft via a 403). Owner-only actions
    (delete, managing collaborators) re-check primary ownership in the service."""
    project = await service.get_project(handle, slug)
    if project is None or not service.can_edit(project, user):
        raise APIError(404, ErrorCode.not_found, "Mod project not found")
    return project


def _valid_status(status: str) -> str:
    if status not in ("draft", "published"):
        raise APIError(400, ErrorCode.bad_request, "status must be draft or published")
    return status


# ── creator connections (dev portal, session JWT) ─────────────────────────
# The developer's half of the link: paste a creator's token to connect, see who
# you're connected to, drop one. The creator's half (issuing/rotating the token,
# narrowing a connection to specific mods, revoking) lives on the Dashboard, at
# ``/site/mods/creator-*`` in app/site/router.py.

@mods_creator_router.get("")
async def list_creator_links(user: User = _PORTAL) -> dict:
    """The creators whose mods this API account may manage."""
    return {"items": await creators.list_for_developer(user)}


@mods_creator_router.post("", status_code=201)
async def add_creator_link(req: CreatorLinkRequest, user: User = _PORTAL) -> dict:
    """Connect to a creator by pasting the creator token from their Dashboard.

    The connection starts covering all of that creator's mods (including ones they
    add later); they can narrow or revoke it from their side at any time. Calls
    then use a normal API token carrying ``mods:write``."""
    return await creators.connect(user, req.token, req.label)


@mods_creator_router.delete("/{link_id}", status_code=204)
async def remove_creator_link(link_id: str, user: User = _PORTAL) -> Response:
    """Drop a creator from your list (the creator can also revoke from their side)."""
    await creators.disconnect_by_developer(user, link_id)
    return Response(status_code=204)


@mods_creator_write_router.get("/me/projects")
async def my_projects(user: SiteUser = _USER) -> dict:
    """Every mod you own or collaborate on, drafts included."""
    return {"items": await service.list_owned(user)}


# The public reads above always view as an anonymous visitor, so they can't show a
# mod that isn't published yet, or a release still in draft. These mirror them for
# the creator's own mods - the read half of everything the control surface writes.

@mods_creator_write_router.get("/me/projects/{handle}/{slug}")
async def my_project(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """One of your own mods, seen as its creator: drafts included, with every
    release (drafts and hidden variants too) and the branch list attached."""
    project = await _require_owned(handle, slug, user)
    return await service.project_detail(project, user)


@mods_creator_write_router.get("/me/projects/{handle}/{slug}/releases")
async def my_project_releases(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """Every release of your mod - drafts and hidden variants included. This is
    where a release ``id`` comes from, to publish, edit or delete it."""
    project = await _require_owned(handle, slug, user)
    return {"items": await service.list_releases(
        project, include_drafts=True, include_hidden=True)}


@mods_creator_write_router.get("/me/projects/{handle}/{slug}/commits")
async def my_project_commits(
    handle: str, slug: str,
    branch: str | None = Query(default=None),
    limit: int = Query(default=50, ge=1, le=200),
    offset: int = Query(default=0, ge=0),
    user: SiteUser = _USER,
) -> dict:
    """Your mod's commit history - the ``id`` of a commit is what a release's
    ``ref`` compiles from."""
    project = await _require_owned(handle, slug, user)
    items, total = await service.list_commits(project, branch, limit, offset)
    return {"items": items, "count": len(items), "total": total}


@mods_creator_write_router.get("/me/projects/{handle}/{slug}/placement")
async def my_project_placement(
    handle: str, slug: str, ref: str = Query(default=""), user: SiteUser = _USER,
) -> dict:
    """Check a commit's files against Trove's placement rules before cutting a
    release: what compiles, what's skipped, and what's misplaced (the misplaced
    ones are what ``fix-placement`` moves)."""
    project = await _require_owned(handle, slug, user)
    return await service.placement_report(project, ref)


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


@mods_creator_write_router.post("/projects", status_code=201)
async def create_project(req: CreateProjectRequest, user: SiteUser = _USER) -> dict:
    project = await service.create_project(
        user, title=req.title, summary=req.summary, description=req.description,
        tags=req.tags, visibility=req.visibility, is_beta=req.is_beta, mode=req.mode,
        source_visibility=req.source_visibility, inspired_by=req.inspired_by,
        on_behalf=req.on_behalf, credited_author=req.credited_author,
    )
    return await service.project_detail(project, user)


@mods_creator_write_router.post("/projects/{handle}/{slug}/fork", status_code=201)
async def fork_project(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    """Fork a mod into a new project of your own, copying its current files and
    crediting the original. The source must be viewable (public/unlisted/yours)."""
    original = await service.get_for_view(handle, slug, user)
    fork = await service.fork_project(user, original)
    return await service.project_detail(fork, user)


@mods_creator_write_router.patch("/projects/{handle}/{slug}")
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


@mods_creator_write_router.post("/projects/{handle}/{slug}/banner")
async def upload_banner(
    handle: str, slug: str, file: UploadFile = File(...), user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    asset = await service.store_image(user, await file.read(), file.content_type)
    project = await service.set_banner(project, user, asset.sha)
    return {"banner_sha": project.banner_sha}


@mods_creator_write_router.delete("/projects/{handle}/{slug}/banner")
async def delete_banner(handle: str, slug: str, user: SiteUser = _USER) -> dict:
    project = await _require_owned(handle, slug, user)
    project = await service.clear_banner(project, user)
    return {"banner_sha": project.banner_sha}


@mods_creator_write_router.post("/projects/{handle}/{slug}/previews")
async def upload_previews(
    handle: str, slug: str, files: list[UploadFile] = File(...), user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    for f in files:
        asset = await service.store_image(user, await f.read(), f.content_type)
        project = await service.add_preview(project, user, asset.sha)
    return {"preview_shas": project.preview_shas}


@mods_creator_write_router.delete("/projects/{handle}/{slug}/previews/{sha}")
async def delete_preview(handle: str, slug: str, sha: str, user: SiteUser = _USER) -> dict:
    project = await _require_owned(handle, slug, user)
    project = await service.remove_preview(project, user, sha)
    return {"preview_shas": project.preview_shas}


@mods_creator_write_router.post("/projects/{handle}/{slug}/branches", status_code=201)
async def create_branch(
    handle: str, slug: str, req: CreateBranchRequest, user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    return await service.create_branch(project, user, req.name, req.from_ref)


@mods_creator_write_router.delete("/projects/{handle}/{slug}/branches/{name}", status_code=204)
async def delete_branch(handle: str, slug: str, name: str, user: SiteUser = _USER) -> Response:
    project = await _require_owned(handle, slug, user)
    await service.delete_branch(project, user, name)
    return Response(status_code=204)


@mods_creator_write_router.post("/projects/{handle}/{slug}/commits", status_code=201)
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


def _decode_config(encoded: str | None) -> bytes | None:
    """Decode the optional base64 ``.cfg`` on a compile-from-commit release."""
    if not encoded:
        return None
    try:
        return base64.b64decode(encoded, validate=True)
    except (ValueError, binascii.Error):
        raise APIError(400, ErrorCode.bad_request, "config_base64 is not valid base64.")


@mods_creator_write_router.post("/projects/{handle}/{slug}/releases", status_code=201)
async def create_release(
    handle: str, slug: str, req: CreateReleaseRequest, user: SiteUser = _USER,
) -> dict:
    """Cut a release by compiling a commit's file tree server-side into the
    chosen ``format`` (``tmod`` or ``zip``).

    Set ``silent`` to ship the build without announcing it - no live-stream event,
    no webhook, no DM alert, and the mod keeps its existing place in the hub's
    "recently updated" order."""
    project = await _require_owned(handle, slug, user)
    return await service.create_release_from_commit(
        project, user, tag=req.tag, title=req.title, changelog=req.changelog,
        ref=req.ref, status=req.status, fmt=req.format, preview_sha=req.preview_sha,
        author=req.author, config_data=_decode_config(req.config_base64),
        silent=req.silent,
    )


@mods_creator_write_router.post("/projects/{handle}/{slug}/releases/upload", status_code=201)
async def upload_release(
    handle: str, slug: str,
    tag: str = Form(...),
    title: str = Form(default=""),
    changelog: str = Form(default=""),
    status: str = Form(default="published"),
    branch: str = Form(default=""),
    silent: bool = Form(
        default=False,
        description="Ship quietly: no live-stream event (so no webhook and no DM "
                    "alert) and the mod's last-release time is left untouched.",
    ),
    file: UploadFile = File(...),
    config: UploadFile | None = File(
        default=None,
        description="Optional .cfg packed into the build as ui/<title>.cfg. Allowed "
                    "only for a .tmod that ships a Flash UI (.swf).",
    ),
    user: SiteUser = _USER,
) -> dict:
    """Cut a release from an already-built ``.tmod`` or ``.zip`` upload (validated
    on ingest). ``branch`` tags it to a variant (defaults to the default branch)."""
    project = await _require_owned(handle, slug, user)
    return await service.create_release_from_upload(
        project, user, tag=tag, title=title, changelog=changelog,
        status=_valid_status(status), branch=branch,
        filename=file.filename or "mod.tmod", data=await file.read(),
        config_data=await config.read() if config is not None else None,
        silent=silent,
    )


@mods_creator_write_router.post("/releases/{release_id}/config")
async def attach_release_config(
    release_id: str,
    file: UploadFile = File(..., description="The .cfg to pack into the existing build."),
    user: SiteUser = _USER,
) -> dict:
    """Pack a config into an ALREADY-PUBLISHED release, as ``ui/<title>.cfg``.

    Rewrites the build in place: the release gets a new ``sha256`` and the old one is
    remembered, so a copy already installed still resolves to this release and no
    phantom update is raised - but nobody who has it is prompted to re-download.
    Cutting a new release is the way to actually deliver a config to existing
    installs. Same gate as at release time: the build must ship a ``.swf``."""
    release = await service.get_release(release_id)
    if release is None:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    project = await service.get_project_by_id(release.project_id)
    if project is None:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    return await service.attach_config_to_release(release, project, user, await file.read())


@mods_creator_write_router.patch("/releases/{release_id}")
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


@mods_creator_write_router.delete("/releases/{release_id}", status_code=204)
async def delete_release(release_id: str, user: SiteUser = _USER) -> Response:
    """Delete a release. The build is gone from the mod's page and can no longer be
    downloaded; the mod itself, its files and its other releases are untouched.
    Unpublishing (``PATCH`` with ``status=draft``) is the reversible alternative."""
    release = await service.get_release(release_id)
    if release is None:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    project = await service.get_project_by_id(release.project_id)
    if project is None or project.owner_id != user.id:
        raise APIError(404, ErrorCode.not_found, "Release not found")
    await service.delete_release(release, project, user)
    return Response(status_code=204)


@mods_creator_write_router.post("/projects/{handle}/{slug}/fix-placement", status_code=201)
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


@mods_hub_write_router.post("/projects/{handle}/{slug}/issues", status_code=201,
                            dependencies=_ISSUES)
async def create_issue(
    handle: str, slug: str, req: CreateIssueRequest, user: SiteUser = _USER,
) -> dict:
    """File an issue or a request on someone's mod. Any signed-in site user."""
    project = await service.get_for_view(handle, slug, user)
    return await issues.create_issue(
        project, user, kind=req.kind, title=req.title, body=req.body)


@mods_hub_write_router.post("/projects/{handle}/{slug}/issues/{number}/comments",
                            status_code=201, dependencies=_ISSUES)
async def comment_on_issue(
    handle: str, slug: str, number: int, req: IssueCommentRequest,
    user: SiteUser = _USER,
) -> dict:
    project = await service.get_for_view(handle, slug, user)
    return await issues.add_comment(project, number, user, req.body)


@mods_hub_write_router.patch("/projects/{handle}/{slug}/issues/{number}",
                             dependencies=_ISSUES)
async def set_issue_status(
    handle: str, slug: str, number: int, req: IssueStatusRequest,
    user: SiteUser = _USER,
) -> dict:
    """Close or reopen a thread - the mod's creator, or the person who opened it."""
    project = await service.get_for_view(handle, slug, user)
    return await issues.set_status(project, number, user, req.status, req.comment)


@mods_hub_write_router.delete("/projects/{handle}/{slug}/issues/{number}",
                              status_code=204, dependencies=_ISSUES)
async def delete_issue(
    handle: str, slug: str, number: int, user: SiteUser = _USER,
) -> Response:
    project = await service.get_for_view(handle, slug, user)
    await issues.delete_issue(project, number, user)
    return Response(status_code=204)


@mods_hub_write_router.delete(
    "/projects/{handle}/{slug}/issues/{number}/comments/{event_id}",
    status_code=204, dependencies=_ISSUES)
async def delete_issue_comment(
    handle: str, slug: str, number: int, event_id: str, user: SiteUser = _USER,
) -> Response:
    project = await service.get_for_view(handle, slug, user)
    await issues.delete_comment(project, number, event_id, user)
    return Response(status_code=204)


@mods_hub_write_router.get("/me/issue-notifications", dependencies=_ISSUES)
async def my_issue_notifications(user: SiteUser = _USER) -> dict:
    """Activity on every thread this user takes part in - the navbar bell."""
    return await issues.notifications(user)


@mods_hub_write_router.post("/me/issue-notifications/seen", dependencies=_ISSUES)
async def mark_issue_notifications_seen(user: SiteUser = _USER) -> dict:
    """Move the read watermark to now (the panel was opened)."""
    return await issues.mark_seen(user)


@mods_hub_write_router.post("/projects/{handle}/{slug}/collaborators")
async def add_collaborator(
    handle: str, slug: str, req: CollaboratorRequest, user: SiteUser = _USER,
) -> dict:
    """Add a co-owner (collaborator) by username. Owner only; they gain edit rights."""
    project = await _require_owned(handle, slug, user)
    return await service.add_collaborator(project, user, req.username)


@mods_hub_write_router.delete("/projects/{handle}/{slug}/collaborators/{user_id}")
async def remove_collaborator(
    handle: str, slug: str, user_id: str, user: SiteUser = _USER,
) -> dict:
    project = await _require_owned(handle, slug, user)
    return await service.remove_collaborator(project, user, user_id)


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
                          description="Case-insensitive substring search over title / summary / tags / author."),
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
