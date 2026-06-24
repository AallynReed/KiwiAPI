"""Mods Hub business logic. No FastAPI types in here - the router adapts.

Reads return JSON-ready dicts (datetimes as ISO strings) shared by both the
``/v1/mods/hub/*`` API and the website's ``/site/mods/*`` proxies. Writes take a
``SiteUser`` actor and enforce ownership; publishing additionally requires a
verified account.
"""

from __future__ import annotations

import asyncio
import io
import logging
import math
import re
import secrets
import zipfile
from datetime import datetime, timedelta, timezone

from beanie import PydanticObjectId
from beanie.operators import In, Inc, Set
from pymongo.errors import DuplicateKeyError

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.security import hash_token
from app.core.utils import utcnow
from app.site_auth.models import SiteUser
from app.trove import mod_categories, tmod
from app.trove.mods_hub import gitstore, store, trove_layout
from app.trove.mods_hub.models import (
    ModClaimRequest,
    ModDownloadEvent,
    ModGitToken,
    ModImageAsset,
    ModProfile,
    ModProject,
    ModRelease,
    ModReport,
    ModStar,
    Visibility,
)

logger = logging.getLogger("kiwi.mods_hub")

GIT_TOKEN_PREFIX = "kgit_"

# Reserved URL handle for imported, unclaimed *stray* mods: /mods/stray/<slug>.
# It is not a real username (usernames can't be this - see the create/handle rules),
# so it can't collide with a SiteUser. On claim-handover the mod re-homes to the
# new owner's username.
STRAY_HANDLE = "stray"

# Branches / commits / file trees are NOT stored in Mongo - they live in the
# per-project git repo (gitstore), so a `git push` and a web "Commit files"
# share one history with nothing to sync. Mongo holds project metadata,
# releases (compiled artifacts), images and reports only.

_SORTS = {
    "recent": "-updated_at",
    "downloads": "-download_count",
    "stars": "-star_count",
    "popular": "-popularity_score",
    "new": "-created_at",
    "title": "title",
}


# --- helpers ---------------------------------------------------------------

def _iso(dt: datetime | None) -> str | None:
    return dt.isoformat() if dt else None


def _norm_path(raw: str) -> str:
    """Normalize a mod-internal file path the same way ``build_tmod`` does, and
    reject traversal so a path can never escape the mod root."""
    p = raw.replace("\\", "/").lstrip("/").lower().strip()
    if not p or p.endswith("/"):
        raise APIError(400, ErrorCode.bad_request, f"Invalid file path: {raw!r}")
    parts = p.split("/")
    if any(seg in ("", ".", "..") for seg in parts):
        raise APIError(400, ErrorCode.bad_request, f"Invalid file path: {raw!r}")
    if len(p.encode("utf-8")) > 255:
        raise APIError(400, ErrorCode.bad_request, f"Path too long (>255 bytes): {raw!r}")
    return p


def _slugify(title: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", title.lower()).strip("-")
    return s[:60] or "mod"


async def _unique_slug(owner_id: PydanticObjectId, title: str) -> str:
    """A slug unique within this owner's mods (slugs are per-owner now)."""
    base = _slugify(title)
    candidate = base
    n = 2
    while await ModProject.find_one(
        ModProject.owner_id == owner_id, ModProject.slug == candidate,
    ) is not None:
        candidate = f"{base}-{n}"
        n += 1
    return candidate


def _clean_tags(tags: list[str]) -> list[str]:
    out: list[str] = []
    for t in tags:
        t = re.sub(r"\s+", " ", (t or "").strip().lower())[:30]
        if t and t not in out:
            out.append(t)
    return out[:12]


_URL_RE = re.compile(r"^https?://\S+$", re.IGNORECASE)


def _clean_url(u: str | None, *, field: str = "link") -> str | None:
    """Validate an owner-provided link: http(s) only, ≤300 chars, no spaces.
    Empty -> None (clears it)."""
    u = (u or "").strip()
    if not u:
        return None
    if len(u) > 300 or not _URL_RE.match(u):
        raise APIError(400, ErrorCode.bad_request,
                       f"Invalid {field} URL - it must start with http:// or https://.")
    return u


def _require_owner(project: ModProject, actor: SiteUser) -> None:
    if project.owner_id != actor.id:
        raise APIError(403, ErrorCode.forbidden, "You don't own this mod project.")


def _not_found(what: str = "Mod project not found") -> APIError:
    return APIError(404, ErrorCode.not_found, what)


# --- DTO builders ----------------------------------------------------------

def _lineage(p: ModProject) -> dict:
    """The attribution block - always surfaced so original work stays credited.
    ``handle`` is included so the UI can link to ``/mods/<handle>/<slug>``."""
    forked_from = inspired_by = None
    if p.forked_from_slug:
        forked_from = {"slug": p.forked_from_slug, "handle": p.forked_from_handle,
                       "title": p.forked_from_title, "owner": p.forked_from_owner}
    if p.inspired_by_slug:
        inspired_by = {"slug": p.inspired_by_slug, "handle": p.inspired_by_handle,
                       "title": p.inspired_by_title, "owner": p.inspired_by_owner}
    return {"forked_from": forked_from, "inspired_by": inspired_by,
            "fork_count": p.fork_count}


def project_card(p: ModProject) -> dict:
    return {
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "tags": p.tags,
        "owner_username": p.owner_username,
        "visibility": p.visibility,
        "mode": p.mode,
        "banner_sha": p.banner_sha,
        # First preview, so a card with no banner can fall back to it (cards only -
        # the mod page header itself never falls back).
        "preview_sha": p.preview_shas[0] if p.preview_shas else None,
        "download_count": p.download_count,
        "star_count": p.star_count,
        # "Stray" = an unclaimed mod uploaded via contributions, not tied to a user
        # yet. The UI shows a "Stray" badge + an "is this yours? claim it" affordance.
        # The source/origin is deliberately NOT exposed publicly (admin-only).
        "is_stray": p.is_stray,
        "author": p.author or p.owner_username,
        "updated_at": _iso(p.updated_at),
        "created_at": _iso(p.created_at),
        **_lineage(p),
    }


def _commit_dto(meta: dict, branch: str | None = None) -> dict:
    """Build a commit DTO from a gitstore commit meta (sha/author/message/time…)."""
    return {
        "id": meta["sha"],
        "short": meta["sha"][:7],
        "branch": branch,
        "parents": meta.get("parents", []),
        "author_username": meta["author"],
        "message": meta["message"],
        "file_count": meta["file_count"],
        "created_at": datetime.fromtimestamp(meta["time"], tz=timezone.utc).isoformat(),
    }


def _release_dto(r: ModRelease) -> dict:
    return {
        "id": str(r.id),
        "tag": r.tag,
        "branch": r.branch,
        "title": r.title,
        "changelog": r.changelog,
        "status": r.status,
        "tmod_filename": release_download_filename(r),   # the .tmod's internal title
        "tmod_size": r.tmod_size,
        "download_count": r.download_count,
        "format": r.release_format,
        "source_commit_sha": r.source_commit_sha,
        "banner_sha": r.banner_sha,
        "published_at": _iso(r.published_at),
        "created_at": _iso(r.created_at),
    }


def release_media_type(release: ModRelease) -> str:
    return "application/zip" if release.release_format == "zip" else "application/octet-stream"


def _build_zip(files: list[tuple[str, bytes]]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
        for path, content in files:
            zf.writestr(path, content)
    return buf.getvalue()


def _branch_dto(b: dict) -> dict:
    """``b`` is a gitstore branch dict (name + head sha), or a synthetic default."""
    return {"name": b["name"], "head_commit_id": b.get("head")}


# --- projects --------------------------------------------------------------

async def _resolve_attribution(
    ref: str | None, viewer: SiteUser | None,
) -> tuple[str, str, str, str] | None:
    """Resolve an ``inspired_by`` ``<handle>/<slug>`` ref to (slug, handle, title,
    owner) if it exists and is viewable; ``None`` for an empty ref. Raises 404 for a
    bad/hidden ref."""
    ref = (ref or "").strip().strip("/")
    if not ref:
        return None
    if "/" not in ref:
        raise APIError(404, ErrorCode.not_found,
                       f"Credit a mod by '<handle>/<slug>' (got '{ref}').")
    handle, _, slug = ref.partition("/")
    other = await get_project(handle, slug)
    if other is None or not can_view(other, viewer):
        raise APIError(404, ErrorCode.not_found, f"No mod '{ref}' to credit.")
    return other.slug, other.owner_handle, other.title, other.owner_username


async def create_project(
    actor: SiteUser, *, title: str, summary: str, description: str,
    tags: list[str], visibility: Visibility, mode: str = "files",
    source_visibility: str = "public", inspired_by: str | None = None,
) -> ModProject:
    slug = await _unique_slug(actor.id, title)
    insp = await _resolve_attribution(inspired_by, actor)
    project = ModProject(
        slug=slug, title=title.strip(), summary=summary.strip(),
        description=description, tags=_clean_tags(tags), visibility=visibility,
        mode=mode, source_visibility=source_visibility,
        owner_id=actor.id, owner_username=actor.display_name or actor.username,
        owner_handle=actor.username,
        inspired_by_slug=insp[0] if insp else None,
        inspired_by_handle=insp[1] if insp else None,
        inspired_by_title=insp[2] if insp else None,
        inspired_by_owner=insp[3] if insp else None,
    )
    await project.insert()
    # Initialise the per-project git repo (the file/history store). The default
    # branch is created on the first commit.
    await gitstore.ensure_repo(str(project.id))
    return project


async def fork_project(actor: SiteUser, original: ModProject) -> ModProject:
    """Create a NEW project owned by ``actor`` that copies the original's current
    default-branch files and points back at it. Starts as a draft.

    Forking copies the source, so it's only allowed when the source is visible to
    the forker. A **source-locked** mod (private source, or releases-only with no
    source) can't be forked - it can only be credited as inspiration."""
    if not source_visible(original, actor):
        raise APIError(
            403, ErrorCode.forbidden,
            "This mod's source is locked - you can credit it as inspiration instead of forking.",
        )
    slug = await _unique_slug(actor.id, original.title)
    fork = ModProject(
        slug=slug, title=original.title, summary=original.summary,
        description=original.description, tags=list(original.tags),
        visibility="draft", mode=original.mode, owner_id=actor.id,
        owner_username=actor.display_name or actor.username,
        owner_handle=actor.username,
        forked_from_id=original.id, forked_from_slug=original.slug,
        forked_from_handle=original.owner_handle, forked_from_title=original.title,
        forked_from_owner=original.owner_username,
    )
    await fork.insert()
    # Copy the original's default-branch HEAD tree into the fork's repo as an
    # initial commit (git object copy - cheap, no working-tree materialisation).
    await gitstore.ensure_repo(str(fork.id))
    await gitstore.fork(
        str(original.id), str(fork.id),
        src_branch=original.default_branch, branch=fork.default_branch,
        author=gitstore.author_ident(fork.owner_username),
        message=f"Fork of {original.slug}", ts=utcnow().timestamp(),
    )
    original.fork_count += 1
    await original.save()
    return fork


async def list_forks(original: ModProject) -> list[dict]:
    # Match by the original's stable id (slugs are per-owner + handles can change).
    docs = await ModProject.find(
        ModProject.forked_from_id == original.id,
        ModProject.visibility == "public", ModProject.taken_down == False,  # noqa: E712
    ).sort("-updated_at").to_list()
    return [project_card(p) for p in docs]


# --- stars (favourites) ----------------------------------------------------

async def has_starred(viewer: SiteUser | None, project: ModProject) -> bool:
    if viewer is None:
        return False
    return await ModStar.find_one(
        ModStar.project_id == project.id, ModStar.site_user_id == viewer.id,
    ) is not None


async def star_project(actor: SiteUser, project: ModProject) -> dict:
    """Star a mod (idempotent). The count is bumped with an atomic ``$inc`` so
    concurrent stars from different users can't lose an update."""
    if await ModStar.find_one(
        ModStar.project_id == project.id, ModStar.site_user_id == actor.id,
    ) is None:
        try:
            await ModStar(project_id=project.id, site_user_id=actor.id).insert()
        except DuplicateKeyError:
            pass   # raced another request - already starred, don't double-count
        else:
            await ModProject.find_one(ModProject.id == project.id).update(
                Inc({ModProject.star_count: 1}))
            project.star_count += 1
    return {"starred": True, "star_count": project.star_count}


async def unstar_project(actor: SiteUser, project: ModProject) -> dict:
    star = await ModStar.find_one(
        ModStar.project_id == project.id, ModStar.site_user_id == actor.id)
    if star is not None:
        await star.delete()
        await ModProject.find_one(ModProject.id == project.id).update(
            Inc({ModProject.star_count: -1}))
        project.star_count = max(0, project.star_count - 1)
    return {"starred": False, "star_count": project.star_count}


async def list_starred(actor: SiteUser) -> list[dict]:
    """The projects ``actor`` has starred (newest star first), filtered to ones
    still viewable to them."""
    stars = await ModStar.find(
        ModStar.site_user_id == actor.id).sort("-created_at").to_list()
    if not stars:
        return []
    by_id = {
        p.id: p for p in await ModProject.find(
            In(ModProject.id, [s.project_id for s in stars])).to_list()
    }
    out: list[dict] = []
    for s in stars:                       # preserve star recency order
        p = by_id.get(s.project_id)
        if p is not None and can_view(p, actor):
            out.append(project_card(p))
    return out


async def get_project(handle: str, slug: str) -> ModProject | None:
    """Resolve a mod by ``<owner_handle>/<slug>``. The handle is the owner's
    canonical lowercase username, so we resolve it to the owner (stable id) and
    then match the slug within that owner - this keeps URLs tracking the owner's
    current username and is unambiguous even if a freed username is later reused.

    The reserved handle ``stray`` addresses imported, unclaimed mods (owner_id=None)
    by slug instead - they have no owning user."""
    if (handle or "").strip().lower() == STRAY_HANDLE:
        return await ModProject.find_one(
            ModProject.is_stray == True, ModProject.slug == slug,  # noqa: E712
        )
    user = await SiteUser.find_one(SiteUser.username == (handle or "").strip().lower())
    if user is None:
        return None
    return await ModProject.find_one(
        ModProject.owner_id == user.id, ModProject.slug == slug,
    )


def can_view(project: ModProject, viewer: SiteUser | None) -> bool:
    """Visibility gate. Owners always see their own projects (incl. drafts and
    taken-down ones, flagged); everyone else is bound by visibility + takedown."""
    if viewer is not None and project.owner_id == viewer.id:
        return True
    if project.taken_down:
        return False
    return project.visibility in ("public", "unlisted")


def source_visible(project: ModProject, viewer: SiteUser | None) -> bool:
    """Whether the *source* (files view + git clone) is visible to ``viewer``.

    The owner always sees their own source (in files mode). For everyone else it
    needs files mode + public source + a viewable project. Releases-only projects
    have NO source, so this is False even for the owner."""
    if project.mode != "files":
        return False
    if viewer is not None and project.owner_id == viewer.id:
        return True
    if project.taken_down or project.source_visibility != "public":
        return False
    return project.visibility in ("public", "unlisted")


def ensure_source_visible(project: ModProject, viewer: SiteUser | None) -> None:
    if not source_visible(project, viewer):
        raise _not_found()


def _require_files_mode(project: ModProject) -> None:
    if project.mode != "files":
        raise APIError(400, ErrorCode.bad_request,
                       "This mod is in releases-only mode - it has no file versioning.")


async def get_for_view(handle: str, slug: str, viewer: SiteUser | None) -> ModProject:
    project = await get_project(handle, slug)
    if project is None or not can_view(project, viewer):
        raise _not_found()
    return project


async def _branches_or_default(project: ModProject) -> list[dict]:
    branches = await gitstore.list_branches(str(project.id))
    if not branches:                       # empty repo: surface the default branch
        return [{"name": project.default_branch, "head": None}]
    return branches


async def project_detail(project: ModProject, viewer: SiteUser | None) -> dict:
    is_owner = viewer is not None and project.owner_id == viewer.id
    src_visible = source_visible(project, viewer)
    # Releases are grouped by branch (variant) client-side; non-owners don't see
    # drafts OR releases from hidden variants.
    releases = await list_releases(
        project, include_drafts=is_owner, include_hidden=is_owner,
    )
    # Branches / commit history / clone URL are only exposed when the source is
    # visible to this viewer (releases-only or private-source mods hide them).
    branches: list[dict] = []
    commit_count = 0
    clone_url = None
    if src_visible:
        branches = [_branch_dto(b) for b in await _branches_or_default(project)]
        commit_count = await gitstore.count_commits(str(project.id), project.default_branch)
        clone_url = f"{settings.api_url.rstrip('/')}/git/mods/{project.owner_handle}/{project.slug}.git"
    return {
        **project_card(project),
        "description": project.description,
        "readme_text": project.readme_text,
        "warnings": project.warnings,
        "default_branch": project.default_branch,
        "preview_shas": project.preview_shas,
        "discord_url": project.discord_url,
        "website_url": project.website_url,
        "donation_urls": project.donation_urls,
        "taken_down": project.taken_down,
        "takedown_reason": project.takedown_reason if is_owner else None,
        "is_owner": is_owner,
        "starred": await has_starred(viewer, project),
        "mode": project.mode,
        "source_visibility": project.source_visibility,
        "source_visible": src_visible,
        "hidden_release_branches": project.hidden_release_branches,
        "branch_order": project.branch_order,
        "commit_count": commit_count,
        "clone_url": clone_url,
        "branches": branches,
        "releases": releases,
    }


async def list_public(
    *, q: str | None = None, tag: str | None = None, author: str | None = None,
    sort: str = "recent", limit: int = 30, offset: int = 0,
) -> tuple[list[dict], int]:
    if sort == "popular":
        await ensure_popularity_fresh()
    query: dict = {"visibility": "public", "taken_down": False}
    if tag:
        query["tags"] = tag.strip().lower()
    if author:
        query["owner_username"] = author
    if q:
        # Text index over title/summary/tags; falls back gracefully if a stop-word.
        query["$text"] = {"$search": q}
    sort_key = _SORTS.get(sort, "-updated_at")
    total = await ModProject.find(query).count()
    docs = await ModProject.find(query).sort(sort_key).skip(offset).limit(limit).to_list()
    return [project_card(p) for p in docs], total


async def tag_facets() -> dict:
    """Tag counts across the public catalog, split for the tag filter UI: the fixed
    **categories** first (in vocab order, canonical labels), then **custom** tags by
    descending count. Each entry is ``{tag, count}``; only tags on at least one
    public mod appear. Category matching is case-insensitive (tags are stored
    lowercased), so a stored ``"gui"`` counts under the ``"GUI"`` category."""
    rows = await ModProject.aggregate([
        {"$match": {"visibility": "public", "taken_down": False}},
        {"$unwind": "$tags"},
        {"$group": {"_id": "$tags", "n": {"$sum": 1}}},
    ]).to_list()
    counts: dict[str, int] = {}
    for r in rows:
        tag = str(r.get("_id") or "").strip()
        if tag:
            counts[tag] = counts.get(tag, 0) + int(r["n"])
    lower_sum: dict[str, int] = {}
    for tag, n in counts.items():
        lower_sum[tag.lower()] = lower_sum.get(tag.lower(), 0) + n
    categories, used = [], set()
    for name in mod_categories.category_names():
        n = lower_sum.get(name.lower(), 0)
        if n > 0:
            categories.append({"tag": name, "count": n})
            used.add(name.lower())
    custom = [{"tag": tag, "count": n} for tag, n in counts.items() if tag.lower() not in used]
    custom.sort(key=lambda x: (-x["count"], x["tag"].lower()))
    return {"categories": categories, "custom": custom}


async def update_project(
    project: ModProject, actor: SiteUser, *, title=None, summary=None,
    description=None, readme_text=None, warnings=None, tags=None, visibility=None,
    mode=None, source_visibility=None, hidden_release_branches=None, branch_order=None,
    discord_url=None, website_url=None, donation_urls=None, inspired_by=None,
) -> ModProject:
    _require_owner(project, actor)
    if title is not None:
        project.title = title.strip()
    if summary is not None:
        project.summary = summary.strip()
    if description is not None:
        project.description = description
    if readme_text is not None:
        project.readme_text = readme_text
    if warnings is not None:
        project.warnings = warnings
    if tags is not None:
        project.tags = _clean_tags(tags)
    if visibility is not None:
        project.visibility = visibility
    if mode is not None:
        project.mode = mode
    if source_visibility is not None:
        project.source_visibility = source_visibility
    if hidden_release_branches is not None:
        # De-dup + trim; an empty list un-hides everything.
        project.hidden_release_branches = list(dict.fromkeys(
            b.strip() for b in hidden_release_branches if b and b.strip()
        ))
    if branch_order is not None:
        # De-dup + trim; branches not listed render after these, alphabetically.
        project.branch_order = list(dict.fromkeys(
            b.strip() for b in branch_order if b and b.strip()
        ))
    if discord_url is not None:
        project.discord_url = _clean_url(discord_url, field="Discord invite")
    if website_url is not None:
        project.website_url = _clean_url(website_url, field="website")
    if donation_urls is not None:
        cleaned: list[str] = []
        for u in donation_urls[:5]:
            cu = _clean_url(u, field="donation")
            if cu and cu not in cleaned:
                cleaned.append(cu)
        project.donation_urls = cleaned
    if inspired_by is not None:
        # Empty string clears the credit; a <handle>/<slug> ref (re)sets it.
        insp = await _resolve_attribution(inspired_by, actor)
        project.inspired_by_slug = insp[0] if insp else None
        project.inspired_by_handle = insp[1] if insp else None
        project.inspired_by_title = insp[2] if insp else None
        project.inspired_by_owner = insp[3] if insp else None
    # Keep the URL handle current with the owner's username (Discord renames).
    project.owner_handle = actor.username
    project.updated_at = utcnow()
    await project.save()
    return project


async def _purge_project(project: ModProject) -> None:
    # Metadata + the git repo are removed; CAS blobs (images, compiled .tmod) are
    # content-addressed + shared, so they're left for a future GC pass rather than
    # risk deleting a blob another project still references.
    await ModRelease.find(ModRelease.project_id == project.id).delete()
    await ModReport.find(ModReport.project_id == project.id).delete()
    await ModStar.find(ModStar.project_id == project.id).delete()
    await gitstore.delete_repo(str(project.id))
    await project.delete()


async def delete_project(project: ModProject, actor: SiteUser) -> None:
    _require_owner(project, actor)
    await _purge_project(project)


# --- branches (git refs) ---------------------------------------------------

async def list_branches(project: ModProject) -> list[dict]:
    return [_branch_dto(b) for b in await _branches_or_default(project)]


def _valid_branch_name(name: str) -> str:
    name = name.strip()
    if not re.fullmatch(r"[A-Za-z0-9._/-]{1,80}", name) or name.startswith("/") or ".." in name:
        raise APIError(400, ErrorCode.bad_request, "Invalid branch name.")
    return name


async def create_branch(
    project: ModProject, actor: SiteUser, name: str, from_ref: str | None,
) -> dict:
    _require_owner(project, actor)
    _require_files_mode(project)
    name = _valid_branch_name(name)
    existing = {b["name"] for b in await gitstore.list_branches(str(project.id))}
    if name in existing:
        raise APIError(409, ErrorCode.conflict, f"Branch '{name}' already exists.")
    try:
        head = await gitstore.create_branch(str(project.id), name, from_ref or project.default_branch)
    except gitstore.GitStoreError as e:
        raise APIError(400, ErrorCode.bad_request, str(e))
    return _branch_dto({"name": name, "head": head})


async def delete_branch(project: ModProject, actor: SiteUser, name: str) -> None:
    _require_owner(project, actor)
    if name == project.default_branch:
        raise APIError(400, ErrorCode.bad_request, "Can't delete the default branch.")
    await gitstore.delete_branch(str(project.id), name)


# --- commits (git) ---------------------------------------------------------

async def _resolve_ref(project: ModProject, ref: str) -> str:
    """Resolve a ref (branch name or commit sha; empty = default branch) to a sha."""
    target = ref or project.default_branch
    sha = await gitstore.resolve(str(project.id), target)
    if sha is None:
        raise _not_found(f"No branch or commit '{target}', or no commits yet.")
    return sha


async def commit_files(
    project: ModProject, actor: SiteUser, *, branch_name: str, message: str,
    adds: list[tuple[str, bytes]], deletes: list[str],
) -> dict:
    _require_owner(project, actor)
    _require_files_mode(project)
    if not message.strip():
        raise APIError(400, ErrorCode.bad_request, "A commit needs a message.")
    if len(adds) > settings.mods_hub_max_files_per_commit:
        raise APIError(400, ErrorCode.bad_request,
                       f"Too many files in one commit (max {settings.mods_hub_max_files_per_commit}).")
    branch_name = _valid_branch_name(branch_name)
    adds_norm: list[tuple[str, bytes]] = []
    for raw, content in adds:
        if len(content) > settings.mods_hub_max_file_bytes:
            raise APIError(413, ErrorCode.bad_request,
                           f"File {raw!r} exceeds the {settings.mods_hub_max_file_bytes}-byte limit.")
        adds_norm.append((_norm_path(raw), content))
    deletes_norm = [_norm_path(d) for d in deletes]
    try:
        sha = await gitstore.write_commit(
            str(project.id), branch=branch_name, adds=adds_norm, deletes=deletes_norm,
            author=gitstore.author_ident(actor.display_name or actor.username),
            message=message.strip(), ts=utcnow().timestamp(),
        )
    except gitstore.NothingToCommit:
        raise APIError(400, ErrorCode.bad_request, "Nothing to commit - the tree is unchanged.")
    project.updated_at = utcnow()
    await project.save()
    tr = await gitstore.read_tree(str(project.id), sha)
    return _commit_dto(tr[0], branch_name) if tr else {"id": sha, "branch": branch_name}


async def list_commits(
    project: ModProject, branch: str | None, limit: int, offset: int,
) -> tuple[list[dict], int]:
    ref = branch or project.default_branch
    metas = await gitstore.list_commits(str(project.id), ref, offset + limit)
    total = await gitstore.count_commits(str(project.id), ref)
    page = metas[offset:offset + limit]
    return [_commit_dto(m, ref) for m in page], total


async def get_tree(project: ModProject, ref: str) -> dict:
    tr = await gitstore.read_tree(str(project.id), ref or project.default_branch)
    if tr is None:                          # empty repo / unknown ref -> empty tree
        return {"commit": None, "entries": []}
    meta, entries = tr
    return {"commit": _commit_dto(meta), "entries": entries}


async def get_file_bytes(project: ModProject, commit_ref: str, path: str) -> bytes:
    data = await gitstore.read_blob(str(project.id), commit_ref, _norm_path(path))
    if data is None:
        raise _not_found(f"No file '{path}' in that commit")
    return data


async def compare(project: ModProject, base_ref: str, head_ref: str) -> dict:
    base = await gitstore.read_tree(str(project.id), base_ref)
    head = await gitstore.read_tree(str(project.id), head_ref)
    if base is None or head is None:
        raise _not_found("One of the refs doesn't exist on this project.")
    base_map = {e["path"]: e["blob_sha"] for e in base[1]}
    head_map = {e["path"]: e["blob_sha"] for e in head[1]}
    added = sorted(p for p in head_map if p not in base_map)
    removed = sorted(p for p in base_map if p not in head_map)
    modified = sorted(p for p in head_map if p in base_map and base_map[p] != head_map[p])
    return {
        "base": _commit_dto(base[0]), "head": _commit_dto(head[0]),
        "added": added, "removed": removed, "modified": modified,
    }


# --- file placement (Trove folder rules + auto-fix) ------------------------

async def placement_report(project: ModProject, ref: str) -> dict:
    """Inspect a commit's tree against Trove's placement rules: which files will
    compile, which are skipped (root / non-Trove folder / ignored type), and
    which are MISPLACED vs the game's structure (fixable). Pure path rules need
    no game data; the misplaced check needs the updates archive populated."""
    tr = await gitstore.read_tree(str(project.id), ref or project.default_branch)
    if tr is None:
        return {"total": 0, "compilable_count": 0, "skipped": [], "misplaced": [],
                "fix_available": False, "game_index_available": False, "commit": None}
    meta, entries = tr
    paths = [e["path"] for e in entries]
    compilable, skipped = trove_layout.classify(paths)
    game_map = await trove_layout.game_file_map()
    misplaced = trove_layout.find_misplaced(paths, game_map)
    misplaced_set = {m["path"] for m in misplaced}
    # A misplaced file is fixable; don't double-list it as plain "skipped".
    skipped = [s for s in skipped if s["path"] not in misplaced_set]
    return {
        "commit": _commit_dto(meta) if meta else None,
        "total": len(paths),
        "compilable_count": len(compilable),
        "skipped": skipped,
        "misplaced": misplaced,
        "fix_available": bool(misplaced),
        "game_index_available": bool(game_map),
    }


async def fix_placement(project: ModProject, actor: SiteUser, branch_name: str) -> dict:
    """Move every misplaced file to the path the game keeps it at, as a single
    new commit on ``branch_name`` (respecting git history - it's a normal commit,
    add-at-new + delete-old). Owner + files-mode only; needs the game index."""
    _require_owner(project, actor)
    _require_files_mode(project)
    branch_name = _valid_branch_name(branch_name or project.default_branch)
    sha = await gitstore.resolve(str(project.id), branch_name)
    if sha is None:
        raise _not_found(f"Branch '{branch_name}' has no commits to fix.")
    tr = await gitstore.read_tree(str(project.id), sha)
    if tr is None:
        raise _not_found("Nothing to fix.")
    game_map = await trove_layout.game_file_map()
    if not game_map:
        raise APIError(409, ErrorCode.conflict,
                       "The game file index isn't available right now, so placement can't be "
                       "auto-fixed. Try again later.")
    misplaced = trove_layout.find_misplaced([e["path"] for e in tr[1]], game_map)
    if not misplaced:
        raise APIError(400, ErrorCode.bad_request,
                       "Nothing to fix - every file is already in its correct place.")
    adds: list[tuple[str, bytes]] = []
    deletes: list[str] = []
    for m in misplaced:
        data = await gitstore.read_blob(str(project.id), sha, m["path"])
        if data is None:
            continue
        adds.append((_norm_path(m["expected"]), data))
        deletes.append(m["path"])
    if not adds:
        raise APIError(400, ErrorCode.bad_request, "Nothing to fix.")
    try:
        commit_sha = await gitstore.write_commit(
            str(project.id), branch=branch_name, adds=adds, deletes=deletes,
            author=gitstore.author_ident(actor.display_name or actor.username),
            message=f"Fix file placement ({len(adds)} file{'s' if len(adds) != 1 else ''})",
            ts=utcnow().timestamp(),
        )
    except gitstore.NothingToCommit:
        raise APIError(400, ErrorCode.bad_request, "Nothing to fix.")
    project.updated_at = utcnow()
    await project.save()
    new_tr = await gitstore.read_tree(str(project.id), commit_sha)
    return {
        "fixed": len(adds),
        "commit": _commit_dto(new_tr[0], branch_name) if new_tr else {"id": commit_sha},
    }


# --- releases --------------------------------------------------------------

def _clean_author(author: str | None, fallback: str) -> str:
    """Normalize the author(s) string stamped into the .tmod. Accepts one name or
    several comma-separated; falls back to the owner's name if empty."""
    if not author or not author.strip():
        return fallback
    names = [n.strip() for n in author.split(",") if n.strip()]
    return ", ".join(names)[:200] or fallback


def _release_properties(project: ModProject, tag: str, changelog: str,
                        author: str | None = None) -> dict[str, str]:
    props = {
        "title": project.title,
        "author": _clean_author(author, project.owner_username),
        "modVersion": tag,
    }
    if changelog.strip():
        props["notes"] = changelog.strip()
    if project.tags:
        props["tags"] = ",".join(project.tags)
        # Encode category tags twice: the natural `tags` string above + a compact
        # bitmask, so the category set round-trips through one number.
        flags = mod_categories.flags_from_tags(project.tags)
        if flags:
            props["flags"] = str(flags)
    return props


# Image content-type → the extension used for the in-.tmod preview path.
_PREVIEW_EXT = {"image/png": "png", "image/jpeg": "jpg", "image/webp": "webp", "image/gif": "gif"}


async def _inject_preview(
    project: ModProject, preview_sha: str,
    files: list[tuple[str, bytes]], props: dict[str, str],
) -> None:
    """Embed a chosen project preview into the compiled .tmod as ``ui/<slug>.<ext>``
    and stamp the ``previewPath`` header property (the Trove convention BTT uses, so
    the game + mod sites show it). The image lives ONLY in the artifact - it is never
    committed to the repo. ``preview_sha`` must be one of the project's own previews;
    if a tree file already sits at that path the chosen preview replaces it."""
    if preview_sha not in project.preview_shas:
        raise APIError(400, ErrorCode.bad_request, "Pick a preview from this mod's images.")
    got = await get_image(preview_sha)
    if got is None:
        raise APIError(400, ErrorCode.bad_request, "That preview image is missing.")
    data, content_type = got
    path = f"ui/{project.slug}.{_PREVIEW_EXT.get(content_type, 'png')}"
    files[:] = [(p, b) for (p, b) in files
                if p.replace("\\", "/").lstrip("/").lower() != path]
    files.append((path, data))
    props["previewPath"] = path


async def _check_release_tag(project: ModProject, branch: str, tag: str) -> None:
    # Tags are unique per branch (variant) - branch X and branch Y can both have v1.0.
    if await ModRelease.find_one(
        ModRelease.project_id == project.id,
        ModRelease.branch == branch,
        ModRelease.tag == tag,
    ):
        raise APIError(409, ErrorCode.conflict,
                       f"Release '{tag}' already exists on branch '{branch}'.")


async def _ensure_hash_unowned(project: ModProject, sha: str) -> None:
    """A mod's content hash belongs to whoever first released it. Reject a release
    whose exact artifact is already published by a *different* creator (anti-reupload).
    The same owner reusing their own artifact (across branches/projects) is fine."""
    # $nin [None, me]: only a release with a *different, known* owner blocks - a
    # pre-migration release with no owner_id (null) must not falsely flag.
    other = await ModRelease.find_one(
        ModRelease.tmod_sha == sha,
        {"owner_id": {"$nin": [None, project.owner_id]}},
    )
    if other is not None:
        raise APIError(409, ErrorCode.conflict,
                       "This exact mod is already published by another creator, so it "
                       "can't be uploaded here. If it's genuinely yours, contact support.")


def _require_publish_ok(actor: SiteUser, status: str) -> None:
    if status == "published" and not actor.is_verified:
        raise APIError(403, ErrorCode.email_unverified,
                       "Verify your account before publishing a release.")


async def _emit_release_event(project: ModProject, release: ModRelease) -> None:
    """Fire a ``mod_release`` event on the live SSE stream (``/v1/events/stream``)
    so external apps can react to new releases without polling. PUBLISHED-only and
    best-effort: a draft, or any publish failure, never affects release creation.
    Signature = the release id, so each new release announces exactly once."""
    if release.status != "published":
        return
    try:
        from app.events import bus
        api = settings.api_url.rstrip("/")
        site = settings.app_url.rstrip("/")
        data = {
            "project": {"slug": project.slug, "handle": project.owner_handle,
                        "title": project.title, "owner": project.owner_username},
            "release": {
                "id": str(release.id), "tag": release.tag, "title": release.title,
                "branch": release.branch, "format": release.release_format,
                "size": release.tmod_size, "changelog": release.changelog,
                "published_at": _iso(release.published_at),
            },
            "download_url": f"{api}/v1/mods/hub/releases/{release.id}/download",
            "page_url": f"{site}/mods/{project.owner_handle}/{project.slug}",
        }
        await bus.publish("mod_release", str(release.id), data)
    except Exception:
        logger.warning("mods_hub: failed to emit release event", exc_info=True)


async def create_release_from_commit(
    project: ModProject, actor: SiteUser, *, tag: str, title: str,
    changelog: str, ref: str, status: str, fmt: str = "tmod",
    preview_sha: str | None = None, author: str | None = None,
) -> dict:
    _require_owner(project, actor)
    _require_files_mode(project)
    _require_publish_ok(actor, status)
    tag = tag.strip()
    commit_sha = await _resolve_ref(project, ref)
    # The release belongs to the branch (variant) it was compiled from; tags are
    # checked per-branch so each variant keeps its own version timeline.
    branch_name = ref if ref and ref in {
        b["name"] for b in await gitstore.list_branches(str(project.id))
    } else project.default_branch
    await _check_release_tag(project, branch_name, tag)
    tr = await gitstore.read_tree(str(project.id), commit_sha)
    if tr is None or not tr[1]:
        raise APIError(400, ErrorCode.bad_request, "That commit has no files to compile.")
    # Only files inside a known Trove folder are compiled - root files and
    # non-Trove folders (bin/, etc.) are ignored, matching the game's override
    # rules (see trove_layout).
    compilable = set(trove_layout.classify([e["path"] for e in tr[1]])[0])
    if not compilable:
        raise APIError(400, ErrorCode.bad_request,
                       "No files inside a Trove folder to compile. Root files and non-Trove "
                       "folders (like 'bin') are ignored - put files under blueprints/, ui/, "
                       "prefabs/, etc.")
    files = []
    for e in tr[1]:
        if e["path"] not in compilable:
            continue
        data = await gitstore.read_blob(str(project.id), commit_sha, e["path"])
        if data is None:
            raise APIError(500, ErrorCode.internal_error, f"Missing blob for {e['path']}")
        files.append((e["path"], data))
    if fmt == "zip":
        artifact = await asyncio.to_thread(_build_zip, files)
        props: dict[str, str] = {}
    else:
        fmt = "tmod"
        props = _release_properties(project, tag, changelog, author=author)
        # A chosen preview is embedded as ui/<slug>.<ext> (not committed to the repo).
        # Zips carry no header properties, so the preview only applies to .tmod.
        if preview_sha:
            await _inject_preview(project, preview_sha, files, props)
        artifact = await asyncio.to_thread(tmod.build_tmod, 1, props, files)
    sha, _ = await store.put_blob(artifact)
    await _ensure_hash_unowned(project, sha)
    return await _insert_release(
        project, tag=tag, branch=branch_name, title=title, changelog=changelog,
        status=status, tmod_sha=sha, tmod_size=len(artifact), properties=props,
        source_commit_sha=commit_sha, release_format=fmt,
    )


async def create_release_from_upload(
    project: ModProject, actor: SiteUser, *, tag: str, title: str,
    changelog: str, status: str, filename: str, data: bytes, branch: str = "",
) -> dict:
    _require_owner(project, actor)
    _require_publish_ok(actor, status)
    tag = tag.strip()
    branch_name = branch.strip() or project.default_branch
    await _check_release_tag(project, branch_name, tag)
    # Accept an already-compiled .tmod OR a .zip (both modes can upload these).
    is_zip = data[:2] == b"PK" or (filename or "").lower().endswith(".zip")
    props: dict[str, str] = {}
    if is_zip:
        fmt = "zip"
        if not zipfile.is_zipfile(io.BytesIO(data)):
            raise APIError(400, ErrorCode.bad_request, "Not a valid .zip file.")
    else:
        fmt = "tmod"
        try:
            parsed = tmod.read_tmod(data, metadata_only=True)
        except tmod.TmodError as e:
            raise APIError(400, ErrorCode.bad_request, f"Not a valid .tmod file: {e}")
        props = {str(k): str(v) for k, v in parsed.get("properties", {}).items()}
    sha, _ = await store.put_blob(data)
    await _ensure_hash_unowned(project, sha)
    return await _insert_release(
        project, tag=tag, branch=branch_name,
        title=title, changelog=changelog, status=status,
        tmod_sha=sha, tmod_size=len(data), properties=props,
        source_commit_sha=None, release_format=fmt,
    )


def _safe_filename(name: str) -> str:
    """Strip characters illegal in a download filename, keeping the rest (incl.
    spaces) intact. Returns 'mod' for an empty/all-illegal name."""
    cleaned = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", str(name or "")).strip().strip(".")
    return cleaned[:120] or "mod"


def release_download_filename(release: ModRelease) -> str:
    """The download name for a release. For a ``.tmod`` this MUST be the artifact's
    internal ``title`` property (Trove matches a mod by that title in-game, and a
    mismatched filename breaks it) - not the slug/tag. Falls back to the stored
    name if the title is missing; zips keep their stored name."""
    if release.release_format == "zip":
        return release.tmod_filename
    title = (release.tmod_properties or {}).get("title", "").strip()
    return f"{_safe_filename(title)}.tmod" if title else release.tmod_filename


async def _insert_release(
    project: ModProject, *, tag: str, branch: str, title: str, changelog: str,
    status: str, tmod_sha: str, tmod_size: int, properties: dict, source_commit_sha,
    release_format: str = "tmod",
) -> dict:
    # The download name is the .tmod's internal `title` (Trove matches on it),
    # falling back to slug-tag; zips keep the slug-tag name.
    if release_format == "zip":
        filename = f"{project.slug}-{tag}.zip"
    else:
        mod_title = (properties or {}).get("title", "").strip()
        filename = f"{_safe_filename(mod_title)}.tmod" if mod_title else f"{project.slug}-{tag}.tmod"
    release = ModRelease(
        project_id=project.id, owner_id=project.owner_id, tag=tag, branch=branch,
        title=title.strip(), changelog=changelog, source_commit_sha=source_commit_sha,
        release_format=release_format, tmod_sha=tmod_sha, tmod_size=tmod_size,
        tmod_filename=filename, tmod_properties=properties,
        banner_sha=project.banner_sha, status=status,
        published_at=utcnow() if status == "published" else None,
    )
    await release.insert()
    project.updated_at = utcnow()
    await project.save()
    await _emit_release_event(project, release)   # SSE: announce if published
    return _release_dto(release)


async def list_releases(
    project: ModProject, *, include_drafts: bool, include_hidden: bool = True,
) -> list[dict]:
    query: dict = {"project_id": project.id}
    if not include_drafts:
        query["status"] = "published"
    if not include_hidden and project.hidden_release_branches:
        query["branch"] = {"$nin": project.hidden_release_branches}
    docs = await ModRelease.find(query).sort("-created_at").to_list()
    return [_release_dto(r) for r in docs]


async def get_release(release_id: str) -> ModRelease | None:
    try:
        return await ModRelease.get(PydanticObjectId(release_id))
    except Exception:
        return None


async def get_project_by_id(pid: PydanticObjectId) -> ModProject | None:
    return await ModProject.get(pid)


async def release_with_project(
    release_id: str, viewer: SiteUser | None,
) -> tuple[ModRelease, ModProject]:
    """Load a release + its project, enforcing visibility. Drafts are visible
    only to the owner; everything maps to a uniform 404 so existence doesn't
    leak."""
    release = await get_release(release_id)
    if release is None:
        raise _not_found("Release not found")
    project = await ModProject.get(release.project_id)
    if project is None or not can_view(project, viewer):
        raise _not_found("Release not found")
    is_owner = viewer is not None and project.owner_id == viewer.id
    if release.status != "published" and not is_owner:
        raise _not_found("Release not found")
    return release, project


async def update_release(
    release: ModRelease, project: ModProject, actor: SiteUser, *,
    title=None, changelog=None, status=None,
) -> dict:
    _require_owner(project, actor)
    if title is not None:
        release.title = title.strip()
    if changelog is not None:
        release.changelog = changelog
    became_published = False
    if status is not None and status != release.status:
        _require_publish_ok(actor, status)
        release.status = status
        if status == "published":
            became_published = True
            if release.published_at is None:
                release.published_at = utcnow()
    release.updated_at = utcnow()
    await release.save()
    if became_published:                          # draft -> published: announce on SSE
        await _emit_release_event(project, release)
    return _release_dto(release)


async def delete_release(release: ModRelease, project: ModProject, actor: SiteUser) -> None:
    _require_owner(project, actor)
    await release.delete()


async def record_download(release: ModRelease, project: ModProject) -> bytes:
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        raise _not_found("Release file is missing from storage")
    release.download_count += 1
    project.download_count += 1
    await release.save()
    await project.save()
    # Log the event for the trailing-7-day "popular" metric (best-effort - a
    # logging hiccup must never block the actual download).
    try:
        await ModDownloadEvent(project_id=project.id, release_id=release.id).insert()
    except Exception:
        logger.warning("mods_hub: failed to record download event", exc_info=True)
    return data


# --- images ----------------------------------------------------------------

async def store_image(actor: SiteUser, data: bytes, declared_ct: str | None) -> ModImageAsset:
    if len(data) > settings.mods_image_max_bytes:
        raise APIError(413, ErrorCode.bad_request,
                       f"Image exceeds the {settings.mods_image_max_bytes}-byte limit.")
    sniffed = store.sniff_image(data)
    if sniffed is None:
        raise APIError(400, ErrorCode.bad_request,
                       "Unsupported image - use PNG, JPEG, WebP or GIF.")
    content_type, w, h = sniffed
    sha, _ = await store.put_blob(data)
    existing = await ModImageAsset.find_one(ModImageAsset.sha == sha)
    if existing is not None:
        return existing
    asset = ModImageAsset(
        sha=sha, content_type=content_type, byte_size=len(data),
        owner_id=actor.id, width=w, height=h,
    )
    await asset.insert()
    return asset


async def get_image(sha: str) -> tuple[bytes, str] | None:
    asset = await ModImageAsset.find_one(ModImageAsset.sha == sha)
    if asset is None:
        return None
    data = await store.get_blob(sha)
    if data is None:
        return None
    return data, asset.content_type


async def set_banner(project: ModProject, actor: SiteUser, sha: str) -> ModProject:
    _require_owner(project, actor)
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        raise _not_found("No such uploaded image")
    project.banner_sha = sha
    project.updated_at = utcnow()
    await project.save()
    return project


async def add_preview(project: ModProject, actor: SiteUser, sha: str) -> ModProject:
    _require_owner(project, actor)
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        raise _not_found("No such uploaded image")
    if sha not in project.preview_shas:
        if len(project.preview_shas) >= 12:
            raise APIError(400, ErrorCode.bad_request, "Too many preview images (max 12).")
        project.preview_shas.append(sha)
        project.updated_at = utcnow()
        await project.save()
    return project


async def remove_preview(project: ModProject, actor: SiteUser, sha: str) -> ModProject:
    _require_owner(project, actor)
    if sha in project.preview_shas:
        project.preview_shas.remove(sha)
        project.updated_at = utcnow()
        await project.save()
    return project


# --- ownership listing (dashboard) ----------------------------------------

async def list_owned(actor: SiteUser) -> list[dict]:
    docs = await ModProject.find(ModProject.owner_id == actor.id).sort("-updated_at").to_list()
    # Opportunistically resync the URL handle to the owner's current username, so a
    # Discord rename propagates to their mod links the next time they open My Mods.
    for p in docs:
        if p.owner_handle != actor.username:
            p.owner_handle = actor.username
            await p.save()
    return [project_card(p) for p in docs]


# --- moderation ------------------------------------------------------------

async def report_project(project: ModProject, reporter: SiteUser, reason: str) -> None:
    await ModReport(
        project_id=project.id, project_slug=project.slug, project_handle=project.owner_handle,
        reporter_id=reporter.id, reporter_username=reporter.username,
        reason=reason.strip(),
    ).insert()


async def _get_by_id(project_id: str) -> ModProject | None:
    """Fetch a project by its ObjectId string (master actions address by id, since
    slugs are only unique per owner)."""
    try:
        return await ModProject.get(PydanticObjectId(project_id))
    except Exception:
        return None


async def take_down(project_id: str, reason: str) -> ModProject:
    project = await _get_by_id(project_id)
    if project is None:
        raise _not_found()
    project.taken_down = True
    project.takedown_reason = reason.strip() or "Removed by a moderator."
    project.updated_at = utcnow()
    await project.save()
    await ModReport.find(ModReport.project_id == project.id).update(Set({ModReport.resolved: True}))
    return project


async def restore(project_id: str) -> ModProject:
    project = await _get_by_id(project_id)
    if project is None:
        raise _not_found()
    project.taken_down = False
    project.takedown_reason = None
    project.updated_at = utcnow()
    await project.save()
    return project


async def master_list_projects(
    *, q: str | None = None, owner: str | None = None, visibility: str | None = None,
    limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """ALL projects (drafts + taken-down included) for master oversight - no
    visibility gate. Used by the dev-portal Mods-hub admin tab."""
    query: dict = {}
    if q:
        query["$text"] = {"$search": q}
    if owner:
        query["owner_username"] = owner
    if visibility:
        query["visibility"] = visibility
    total = await ModProject.find(query).count()
    docs = await ModProject.find(query).sort("-updated_at").skip(offset).limit(limit).to_list()
    items = [{**project_card(p), "id": str(p.id), "taken_down": p.taken_down,
              "owner_id": str(p.owner_id) if p.owner_id else None} for p in docs]
    return items, total


async def master_delete_project(project_id: str) -> None:
    """Force-delete any project (master). Bypasses ownership."""
    project = await _get_by_id(project_id)
    if project is None:
        raise _not_found()
    await _purge_project(project)


# --- stray (imported) mods: admin approval queue + claim/handover ----------

def _stray_card(p: ModProject) -> dict:
    # The import origin is stored on the model (for idempotent re-imports) but is NOT
    # surfaced anywhere - not even in the admin response.
    return {**project_card(p), "id": str(p.id), "stray_status": p.stray_status,
            "taken_down": p.taken_down}


async def master_list_stray(
    *, status: str | None = None, q: str | None = None, limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """Imported stray mods for the admin panel, filtered by ``stray_status``
    (pending / approved / rejected). Newest-touched first."""
    query: dict = {"is_stray": True}
    if status:
        query["stray_status"] = status
    if q:
        query["$text"] = {"$search": q}
    total = await ModProject.find(query).count()
    docs = await ModProject.find(query).sort("-updated_at").skip(offset).limit(limit).to_list()
    return [_stray_card(p) for p in docs], total


async def _get_stray(project_id: str) -> ModProject:
    project = await _get_by_id(project_id)
    if project is None or not project.is_stray:
        raise _not_found("Stray mod not found")
    return project


async def approve_stray(project_id: str) -> dict:
    """Approve a pending stray mod -> it becomes publicly visible in the catalog."""
    project = await _get_stray(project_id)
    project.stray_status = "approved"
    project.visibility = "public"
    project.updated_at = utcnow()
    await project.save()
    return _stray_card(project)


async def reject_stray(project_id: str) -> dict:
    """Reject a stray mod -> hidden, and skipped on future resyncs."""
    project = await _get_stray(project_id)
    project.stray_status = "rejected"
    project.visibility = "draft"
    project.updated_at = utcnow()
    await project.save()
    return _stray_card(project)


async def handover_stray(project: ModProject, user: SiteUser) -> ModProject:
    """Hand a stray mod over to a real site user: it stops being stray and becomes an
    ordinary mod owned by ``user`` (re-homed to /mods/<username>/<slug>)."""
    # Keep the current slug if it's free under the new owner, else make a fresh one.
    clash = await ModProject.find_one(
        ModProject.owner_id == user.id, ModProject.slug == project.slug)
    project.owner_id = user.id
    project.owner_username = user.display_name or user.username
    project.owner_handle = user.username
    project.is_stray = False
    project.stray_status = None
    project.visibility = "public"
    if clash is not None:
        project.slug = await _unique_slug(user.id, project.title)
    project.updated_at = utcnow()
    await project.save()
    # Reattach the mirrored release(s) to the new owner.
    await ModRelease.find(ModRelease.project_id == project.id).update(
        Set({ModRelease.owner_id: user.id}))
    return project


def _claim_dto(c: ModClaimRequest) -> dict:
    return {
        "id": str(c.id), "project_id": str(c.project_id), "project_slug": c.project_slug,
        "project_title": c.project_title, "claimant_username": c.claimant_username,
        "message": c.message, "status": c.status, "created_at": _iso(c.created_at),
        "resolved_at": _iso(c.resolved_at),
    }


async def create_claim(project: ModProject, actor: SiteUser, message: str) -> dict:
    """A site user requests to claim a stray mod as their own (admin approves)."""
    if not project.is_stray:
        raise APIError(400, ErrorCode.bad_request, "This mod isn't available to claim.")
    existing = await ModClaimRequest.find_one(
        ModClaimRequest.project_id == project.id,
        ModClaimRequest.claimant_id == actor.id,
        ModClaimRequest.status == "pending",
    )
    if existing is not None:
        return {**_claim_dto(existing), "already": True}
    claim = ModClaimRequest(
        project_id=project.id, project_slug=project.slug, project_title=project.title,
        claimant_id=actor.id, claimant_username=actor.username, message=(message or "")[:2000],
    )
    await claim.insert()
    return {**_claim_dto(claim), "already": False}


async def list_claims(status: str | None = "pending", limit: int = 100) -> list[dict]:
    query: dict = {}
    if status:
        query["status"] = status
    docs = await ModClaimRequest.find(query).sort("-created_at").limit(limit).to_list()
    return [_claim_dto(c) for c in docs]


async def approve_claim(claim_id: str, master_id: PydanticObjectId) -> dict:
    """Approve a claim: hand the stray mod over to the claimant. Other open claims on
    the same mod are auto-rejected."""
    claim = await _get_claim(claim_id)
    if claim.status != "pending":
        raise APIError(400, ErrorCode.bad_request, "This claim is already resolved.")
    project = await ModProject.get(claim.project_id)
    if project is None or not project.is_stray:
        raise APIError(400, ErrorCode.bad_request, "This mod is no longer claimable.")
    user = await SiteUser.get(claim.claimant_id)
    if user is None:
        raise _not_found("The claiming user no longer exists.")
    await handover_stray(project, user)
    claim.status = "approved"
    claim.resolved_by = master_id
    claim.resolved_at = utcnow()
    await claim.save()
    # Any other open claims for this mod can't succeed now - reject them.
    await ModClaimRequest.find(
        ModClaimRequest.project_id == project.id,
        ModClaimRequest.status == "pending",
    ).update(Set({ModClaimRequest.status: "rejected",
                  ModClaimRequest.resolved_at: utcnow()}))
    return {**_claim_dto(claim), "handle": project.owner_handle, "slug": project.slug}


async def reject_claim(claim_id: str, master_id: PydanticObjectId) -> dict:
    claim = await _get_claim(claim_id)
    claim.status = "rejected"
    claim.resolved_by = master_id
    claim.resolved_at = utcnow()
    await claim.save()
    return _claim_dto(claim)


async def _get_claim(claim_id: str) -> ModClaimRequest:
    try:
        claim = await ModClaimRequest.get(PydanticObjectId(claim_id))
    except Exception:
        claim = None
    if claim is None:
        raise _not_found("Claim not found")
    return claim


async def backfill_owner_handles() -> None:
    """One-time-ish: set ``owner_handle`` on mods created before per-owner slugs
    (the field defaulted to ""). Safe to run every boot - it only touches rows
    still missing a handle."""
    missing = await ModProject.find(
        {"$or": [{"owner_handle": ""}, {"owner_handle": {"$exists": False}}]}
    ).to_list()
    for proj in missing:
        user = await SiteUser.get(proj.owner_id)
        if user is not None:
            proj.owner_handle = user.username
            await proj.save()
    if missing:
        logger.info("mods_hub: backfilled owner_handle on %d project(s)", len(missing))


# --- git access tokens (PATs for git clone/pull/push) ----------------------

def _git_token_dto(d: ModGitToken) -> dict:
    return {
        "id": str(d.id), "name": d.name, "prefix": d.prefix,
        "created_at": _iso(d.created_at), "last_used_at": _iso(d.last_used_at),
    }


async def create_git_token(actor: SiteUser, name: str) -> tuple[dict, str]:
    """Mint a git access token. Returns (dto, plaintext) - plaintext shown ONCE."""
    raw = GIT_TOKEN_PREFIX + secrets.token_urlsafe(24)
    doc = ModGitToken(
        site_user_id=actor.id, token_hash=hash_token(raw),
        prefix=raw[:12], name=(name or "").strip()[:60],
    )
    await doc.insert()
    return _git_token_dto(doc), raw


async def list_git_tokens(actor: SiteUser) -> list[dict]:
    docs = await ModGitToken.find(
        ModGitToken.site_user_id == actor.id, ModGitToken.revoked == False,  # noqa: E712
    ).sort("-created_at").to_list()
    return [_git_token_dto(d) for d in docs]


async def revoke_git_token(actor: SiteUser, token_id: str) -> None:
    try:
        doc = await ModGitToken.get(PydanticObjectId(token_id))
    except Exception:
        doc = None
    if doc is None or doc.site_user_id != actor.id:
        raise _not_found("Token not found")
    doc.revoked = True
    await doc.save()


async def authenticate_git(token: str) -> SiteUser | None:
    """Resolve a git access token (the HTTP Basic password) to its site user."""
    if not token:
        return None
    doc = await ModGitToken.find_one(
        ModGitToken.token_hash == hash_token(token), ModGitToken.revoked == False,  # noqa: E712
    )
    if doc is None:
        return None
    user = await SiteUser.get(doc.site_user_id)
    if user is None or not user.is_active:
        return None
    doc.last_used_at = utcnow()
    await doc.save()
    return user


async def touch_after_push(project: ModProject) -> None:
    """Called after a successful `git push` - the git repo already has the new
    commits/branches (we read history live from it), so we only bump updated_at."""
    project.updated_at = utcnow()
    await project.save()


async def list_reports(resolved: bool = False, limit: int = 100) -> list[dict]:
    docs = await ModReport.find(ModReport.resolved == resolved).sort("-created_at").limit(limit).to_list()
    return [
        {
            "id": str(r.id), "project_id": str(r.project_id),
            "project_slug": r.project_slug, "project_handle": r.project_handle,
            "reporter_username": r.reporter_username, "reason": r.reason,
            "resolved": r.resolved, "created_at": _iso(r.created_at),
        }
        for r in docs
    ]


# --- Public catalog API (documented, app-facing) ---------------------------
# These power the documented /v1/mods/* endpoints. Unlike the internal hub DTOs
# (which return raw shas for the same-origin website), these return ABSOLUTE
# image / download / page URLs so an external app can consume them directly.

def _public_img_url(sha: str | None) -> str | None:
    if not sha:
        return None
    return f"{settings.api_url.rstrip('/')}/v1/mods/hub/image/{sha}"


def public_release_dto(r: ModRelease) -> dict:
    """A published build, app-facing. ``sha256`` is the artifact's content hash -
    the same value the lookup-by-hash endpoint matches on."""
    return {
        "tag": r.tag,
        "branch": r.branch,
        "title": r.title,
        "changelog": r.changelog,
        "format": r.release_format,
        "filename": release_download_filename(r),   # the .tmod's internal title
        "size": r.tmod_size,
        "sha256": r.tmod_sha,
        "download_count": r.download_count,
        "download_url": f"{settings.api_url.rstrip('/')}/v1/mods/hub/releases/{r.id}/download",
        "published_at": _iso(r.published_at),
    }


def public_mod_dto(p: ModProject, *, releases: list[ModRelease] | None = None) -> dict:
    """Full app-facing metadata for one mod. ``releases`` (published, newest first)
    is attached when provided; list/popular responses omit it for brevity."""
    out = {
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "description": p.description,
        # readme_text = releases-only long-form README; warnings = <br>-split blocks.
        "readme_text": p.readme_text,
        "warnings": p.warnings,
        "tags": p.tags,
        "categories": mod_categories.tags_from_flags(mod_categories.flags_from_tags(p.tags)),
        "flags": mod_categories.flags_from_tags(p.tags),
        "author": p.author or p.owner_username,
        # "Stray" = an unclaimed mod uploaded via contributions (not tied to a user
        # yet). The origin/source is intentionally not exposed in the public API.
        "is_stray": p.is_stray,
        "banner_url": _public_img_url(p.banner_sha),
        "preview_urls": [u for u in (_public_img_url(s) for s in p.preview_shas) if u],
        "download_count": p.download_count,
        "downloads_7d": p.downloads_7d,
        "star_count": p.star_count,
        "popularity_score": p.popularity_score,
        "discord_url": p.discord_url,
        "website_url": p.website_url,
        "donation_urls": p.donation_urls,
        "page_url": f"{settings.app_url.rstrip('/')}/mods/{p.owner_handle}/{p.slug}",
        "created_at": _iso(p.created_at),
        "updated_at": _iso(p.updated_at),
        **_lineage(p),
    }
    if releases is not None:
        out["releases"] = [public_release_dto(r) for r in releases]
    return out


async def _published_releases(project: ModProject) -> list[ModRelease]:
    query: dict = {"project_id": project.id, "status": "published"}
    if project.hidden_release_branches:
        query["branch"] = {"$nin": project.hidden_release_branches}
    return await ModRelease.find(query).sort("-published_at").to_list()


async def public_list(
    *, q: str | None = None, tag: str | None = None, author: str | None = None,
    sort: str = "recent", limit: int = 30, offset: int = 0,
) -> tuple[list[dict], int]:
    """Browse public mods (cards, no releases) with app-facing DTOs."""
    if sort == "popular":
        await ensure_popularity_fresh()
    query: dict = {"visibility": "public", "taken_down": False}
    if tag:
        query["tags"] = tag.strip().lower()
    if author:
        query["owner_username"] = author
    if q:
        query["$text"] = {"$search": q}
    sort_key = _SORTS.get(sort, "-updated_at")
    total = await ModProject.find(query).count()
    docs = await ModProject.find(query).sort(sort_key).skip(offset).limit(limit).to_list()
    return [public_mod_dto(p) for p in docs], total


async def public_detail(handle: str, slug: str) -> dict:
    """Full metadata + published releases for one public/unlisted mod (404 otherwise)."""
    project = await get_for_view(handle, slug, None)   # anon view: public/unlisted, not taken down
    return public_mod_dto(project, releases=await _published_releases(project))


async def public_popular(limit: int = 25) -> list[dict]:
    """The most popular public mods by the trailing-7-day score (top 25 by default)."""
    await ensure_popularity_fresh()
    limit = max(1, min(limit, 25))
    docs = await ModProject.find(
        {"visibility": "public", "taken_down": False},
    ).sort("-popularity_score").limit(limit).to_list()
    return [public_mod_dto(p) for p in docs]


async def lookup_by_hashes(hashes: list[str]) -> dict:
    """Resolve mod metadata for one or more artifact content hashes (sha256 hex).
    Per known hash, returns the matching mod + the specific release (newest if the
    owner reused it); hashes with no public match are listed under ``unknown``.
    Draft / taken-down mods never match."""
    seen: set[str] = set()
    uniq = [h for h in (x.strip().lower() for x in hashes if x and x.strip())
            if not (h in seen or seen.add(h))]
    results: dict[str, dict] = {}
    if uniq:
        releases = await ModRelease.find(
            In(ModRelease.tmod_sha, uniq), ModRelease.status == "published",
        ).sort("-published_at").to_list()
        proj_cache: dict = {}
        for r in releases:
            if r.tmod_sha in results:
                continue  # newest already chosen (sorted desc)
            if r.project_id not in proj_cache:
                proj_cache[r.project_id] = await ModProject.get(r.project_id)
            proj = proj_cache[r.project_id]
            if proj is None or proj.taken_down or proj.visibility == "draft":
                continue
            results[r.tmod_sha] = {
                "mod": public_mod_dto(proj),
                "release": public_release_dto(r),
            }
    return {"results": results, "unknown": [h for h in uniq if h not in results]}


# --- Popularity (trailing-7-day downloads -> 0.0-1.0 score) ----------------

_POP_REFRESH_INTERVAL = timedelta(minutes=10)
_pop_state: dict = {"at": None}


async def ensure_popularity_fresh() -> None:
    """Recompute the popularity snapshot at most once per interval (per worker), so
    the score stays denormalized + index-backed for listing/sorting without a
    dedicated background job."""
    now = utcnow()
    last = _pop_state["at"]
    if last is not None and now - last < _POP_REFRESH_INTERVAL:
        return
    _pop_state["at"] = now   # claim the slot before awaiting (avoid double-compute)
    try:
        await refresh_popularity()
    except Exception:
        logger.warning("mods_hub: popularity refresh failed", exc_info=True)


async def refresh_popularity() -> None:
    """Recompute per-project trailing-7-day downloads + a 0.0-1.0 popularity score,
    denormalized onto each public project. Score = a log-damped blend weighted
    toward recent downloads (with stars + lifetime downloads as a floor), normalized
    so the most popular mod scores ~1.0."""
    since = utcnow() - timedelta(days=7)
    rows = await ModDownloadEvent.aggregate([
        {"$match": {"created_at": {"$gte": since}}},
        {"$group": {"_id": "$project_id", "n": {"$sum": 1}}},
    ]).to_list()
    recent = {r["_id"]: int(r["n"]) for r in rows}
    projects = await ModProject.find(
        {"visibility": "public", "taken_down": False},
    ).to_list()
    raw = {
        p.id: 4.0 * math.log1p(recent.get(p.id, 0))
        + 2.0 * math.log1p(p.star_count)
        + 1.0 * math.log1p(p.download_count)
        for p in projects
    }
    top = max(raw.values(), default=0.0) or 1.0
    for p in projects:
        d7 = recent.get(p.id, 0)
        score = round(raw[p.id] / top, 4)
        if p.downloads_7d != d7 or p.popularity_score != score:
            # Targeted $set (not save()) so a concurrent owner edit isn't clobbered.
            await p.update(Set({ModProject.downloads_7d: d7,
                                ModProject.popularity_score: score}))


# --- modder profiles (/mods/<handle>) --------------------------------------

_DISCORD_CDN = "https://cdn.discordapp.com"


def _discord_avatar_url(user: SiteUser) -> str | None:
    """A Discord CDN avatar URL from the stored hash (falls back to the account's
    default embed avatar). Used when a modder hasn't uploaded a custom one."""
    if user.discord_id is None:
        return None
    if user.discord_avatar:
        ext = "gif" if user.discord_avatar.startswith("a_") else "png"
        return f"{_DISCORD_CDN}/avatars/{user.discord_id}/{user.discord_avatar}.{ext}?size=256"
    return f"{_DISCORD_CDN}/embed/avatars/{(user.discord_id >> 22) % 6}.png"


def _profile_avatar_url(user: SiteUser, profile: ModProfile | None) -> str | None:
    if profile is not None and profile.avatar_sha:
        return _public_img_url(profile.avatar_sha)
    return _discord_avatar_url(user)


def profile_dto(user: SiteUser, profile: ModProfile | None, is_owner: bool,
                mods: list[ModProject]) -> dict:
    p = profile
    name = (p.display_name.strip() if (p and p.display_name) else "") \
        or (user.display_name or "").strip() or user.username
    # Apply the owner-chosen mod order (their slugs); the rest keep recency order.
    order = (p.mod_order if p else []) or []
    by_slug = {m.slug: m for m in mods}
    in_order = set(order)
    ordered = [by_slug[s] for s in order if s in by_slug] \
        + [m for m in mods if m.slug not in in_order]
    featured_slug = p.featured_slug if p else None
    featured = project_card(by_slug[featured_slug]) \
        if (featured_slug and featured_slug in by_slug) else None
    return {
        "handle": user.username,
        "display_name": name,
        "tagline": p.tagline if p else "",
        "readme": p.readme if p else "",
        "avatar_url": _profile_avatar_url(user, profile),
        "avatar_sha": p.avatar_sha if p else None,
        "banner_url": _public_img_url(p.banner_sha) if (p and p.banner_sha) else None,
        "banner_sha": p.banner_sha if p else None,
        "discord_url": p.discord_url if p else None,
        "website_url": p.website_url if p else None,
        "donation_urls": p.donation_urls if p else [],
        "is_owner": is_owner,
        "joined_at": _iso(user.created_at),
        "page_url": f"{settings.app_url.rstrip('/')}/mods/{user.username}",
        "mod_count": len(ordered),
        "mod_order": [m.slug for m in ordered],
        "featured_slug": featured.get("slug") if featured else None,
        "featured": featured,
        "mods": [project_card(m) for m in ordered],
    }


async def profile_view(handle: str, viewer: SiteUser | None) -> dict | None:
    """A modder's profile + their mods. Owner sees all their mods (drafts incl.);
    everyone else sees their public, non-taken-down ones. ``None`` (→ 404) if there's
    no such user OR the modder has **no public mod** - a profile only exists once they
    have published at least one public, non-taken-down mod (nothing to showcase
    otherwise)."""
    user = await SiteUser.find_one(SiteUser.username == (handle or "").strip().lower())
    if user is None or not user.is_active:
        return None
    has_public = await ModProject.find_one(
        ModProject.owner_id == user.id, ModProject.visibility == "public",
        ModProject.taken_down == False,  # noqa: E712
    ) is not None
    if not has_public:
        return None
    profile = await ModProfile.find_one(ModProfile.site_user_id == user.id)
    is_owner = viewer is not None and viewer.id == user.id
    query: dict = {"owner_id": user.id}
    if not is_owner:
        query.update({"visibility": "public", "taken_down": False})
    mods = await ModProject.find(query).sort("-updated_at").to_list()
    return profile_dto(user, profile, is_owner, mods)


async def _get_or_make_profile(actor: SiteUser) -> ModProfile:
    profile = await ModProfile.find_one(ModProfile.site_user_id == actor.id)
    if profile is None:
        profile = ModProfile(site_user_id=actor.id, handle=actor.username)
        await profile.insert()
    return profile


async def update_profile(
    actor: SiteUser, *, display_name=None, tagline=None, readme=None,
    discord_url=None, website_url=None, donation_urls=None,
    mod_order=None, featured_slug=None,
) -> dict:
    profile = await _get_or_make_profile(actor)
    if mod_order is not None:
        profile.mod_order = list(dict.fromkeys(s.strip() for s in mod_order if s and s.strip()))
    if featured_slug is not None:
        fs = featured_slug.strip()   # empty string clears the highlight
        owns = await ModProject.find_one(
            ModProject.owner_id == actor.id, ModProject.slug == fs) if fs else None
        profile.featured_slug = fs if owns is not None else None
    if display_name is not None:
        profile.display_name = display_name.strip()[:80]
    if tagline is not None:
        profile.tagline = tagline.strip()[:160]
    if readme is not None:
        profile.readme = readme
    if discord_url is not None:
        profile.discord_url = _clean_url(discord_url, field="Discord")
    if website_url is not None:
        profile.website_url = _clean_url(website_url, field="website")
    if donation_urls is not None:
        cleaned: list[str] = []
        for u in donation_urls[:5]:
            cu = _clean_url(u, field="donation")
            if cu and cu not in cleaned:
                cleaned.append(cu)
        profile.donation_urls = cleaned
    profile.handle = actor.username   # resync the URL handle
    profile.updated_at = utcnow()
    await profile.save()
    return await profile_view(actor.username, actor)


async def set_profile_image(actor: SiteUser, sha: str, *, banner: bool) -> dict:
    """Set the profile avatar (or banner) to an already-uploaded image."""
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        raise _not_found("No such uploaded image")
    profile = await _get_or_make_profile(actor)
    if banner:
        profile.banner_sha = sha
    else:
        profile.avatar_sha = sha
    profile.handle = actor.username
    profile.updated_at = utcnow()
    await profile.save()
    return await profile_view(actor.username, actor)
