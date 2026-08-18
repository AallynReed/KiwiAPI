"""Mods Hub business logic. No FastAPI types in here - the router adapts.

Reads return JSON-ready dicts (datetimes as ISO strings) shared by both the
``/v1/mods/hub/*`` API and the website's ``/site/mods/*`` proxies. Writes take a
``SiteUser`` actor and enforce ownership; publishing additionally requires a
verified account.
"""

from __future__ import annotations

import asyncio
import base64
import io
import logging
import math
import re
import secrets
import zipfile
from datetime import datetime, timedelta, timezone

from beanie import PydanticObjectId
from beanie.operators import In, Inc, Or, Set
from pymongo.errors import DuplicateKeyError

from app import i18n as app_i18n
from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.security import hash_token
from app.core.utils import iso as _iso
from app.core.utils import to_oid, utcnow
from app.site_auth.models import SiteUser
from app.trove import mod_categories, tmod
from app.trove.mods_hub import gitstore, store, trove_layout
from app.trove.mods_hub.models import (
    Collaborator,
    ContentReport,
    ModClaimRequest,
    ModDownloadEvent,
    ModGitToken,
    ModImageAsset,
    ModProfile,
    ModProject,
    ModRelease,
    ModStar,
    Visibility,
)
from app.trove.render import bp_cache

logger = logging.getLogger("kiwi.mods_hub")

GIT_TOKEN_PREFIX = "kgit_"

# Reserved URL handle for imported, unclaimed *stray* mods: /mods/stray/<slug>.
# It is not a real username (usernames can't be this - see the create/handle rules),
# so it can't collide with a SiteUser. On claim-handover the mod re-homes to the
# new owner's username.
STRAY_HANDLE = "stray"

# A built .tmod is a release artifact, never versioned source. Committed as a file
# it lands in the Files tab where nobody can install it, so it's refused on every
# write path: here, and on git push (see gitstore's receive-pack guard).
BLOCKED_COMMIT_EXTENSIONS = (".tmod",)
BLOCKED_COMMIT_MESSAGE = (
    "A built .tmod goes in a release, not in your files - use New release to "
    "upload it so people can download and install it."
)

_SORTS = {
    # "Recently updated" means a new build landed, not that someone fixed a typo
    # in the description - so it orders by the last PUBLISHED release.
    "recent": "-last_release_at",
    "downloads": "-download_count",
    "stars": "-star_count",
    "popular": "-popularity_score",
    "new": "-created_at",
    "title": "title",
}


# --- helpers ---------------------------------------------------------------

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


# Languages a mod's text may be translated into = the site's language picker,
# minus English (which is the base field itself, always the fallback).
CONTENT_LANGS: frozenset[str] = frozenset(app_i18n.SUPPORTED) - {app_i18n.DEFAULT_LANG}

# Human wording for the field an error is about (the API field name would read as
# jargon in the modder's editor).
_I18N_LABELS = {
    "title": "title", "summary": "summary", "description": "description",
    "readme_text": "README", "readme": "README", "warnings": "warnings",
    "changelog": "changelog", "tagline": "tagline",
}


def _clean_i18n_map(translations: dict[str, str], *, base: str, max_len: int) -> dict[str, str]:
    """Validate a translation map: known language codes only, English excluded
    (it lives in ``base``), blanks dropped."""
    label = _I18N_LABELS.get(base, base)
    out: dict[str, str] = {}
    for lang, text in translations.items():
        lang = (lang or "").strip()
        if lang == app_i18n.DEFAULT_LANG:
            raise APIError(400, ErrorCode.bad_request,
                           f"The English {label} is `{base}`, not a translation.")
        if lang not in CONTENT_LANGS:
            raise APIError(400, ErrorCode.bad_request,
                           f"'{lang}' isn't a language the site supports.")
        if len(text or "") > max_len:
            raise APIError(400, ErrorCode.bad_request,
                           f"The {lang} {label} is too long (max {max_len:,} characters).")
        text = (text or "").strip()
        if text:
            out[lang] = text
    return out


def _set_i18n(doc, fields: dict[str, tuple[dict | None, str, int]]) -> None:
    """Write ``{attribute: (translations, base field, max length)}`` onto a
    document, skipping the fields this request didn't send."""
    for attr, (translations, base, max_len) in fields.items():
        if translations is not None:
            setattr(doc, attr, _clean_i18n_map(translations, base=base, max_len=max_len))


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


def _collab_ids(project: ModProject) -> set:
    return {c.user_id for c in (project.collaborators or [])}


def can_edit(project: ModProject, actor: SiteUser | None) -> bool:
    """Edit rights = the primary owner OR a collaborator (co-owner)."""
    if actor is None:
        return False
    return actor.id == project.owner_id or actor.id in _collab_ids(project)


def is_primary_owner(project: ModProject, actor: SiteUser | None) -> bool:
    return actor is not None and actor.id == project.owner_id


def _require_owner(project: ModProject, actor: SiteUser) -> None:
    """Edit-level gate: the primary owner or any collaborator. (Named *owner* for
    history; collaborators are co-owners with edit rights.)"""
    if not can_edit(project, actor):
        raise APIError(403, ErrorCode.forbidden, "You don't have edit access to this mod project.")


def _require_primary_owner(project: ModProject, actor: SiteUser) -> None:
    """Stricter gate for owner-only actions (delete, managing collaborators)."""
    if not is_primary_owner(project, actor):
        raise APIError(403, ErrorCode.forbidden,
                       "Only the mod's owner can do this.")


def _not_found(what: str = "Mod project not found") -> APIError:
    return APIError(404, ErrorCode.not_found, what)


def _not_public(what: str = "This mod isn't public yet.") -> APIError:
    # A DISTINCT code (still 404 so an owner's aged-token auto-refresh path is
    # unaffected) that lets the page say "not public yet" instead of "not found"
    # when a real, non-taken-down draft is viewed by someone who can't see it.
    return APIError(404, ErrorCode.not_public, what)


def _search_clause(q: str) -> dict:
    """Case-insensitive SUBSTRING search across the card-visible fields + author.
    (Replaces MongoDB ``$text``, which is case-insensitive but only matches whole
    word/stems - so it missed partial terms; this matches the substring behaviour
    users know from the desktop app.)"""
    rx = {"$regex": re.escape(q.strip()), "$options": "i"}
    return {"$or": [{"title": rx}, {"summary": rx}, {"tags": rx},
                    {"owner_username": rx}, {"author": rx}]}


def _author_eq(author: str) -> dict:
    """Case-insensitive exact match on the owner/author name (the `author` filter)."""
    return {"$regex": f"^{re.escape(author.strip())}$", "$options": "i"}


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
        "title_i18n": p.title_i18n,
        "summary": p.summary,
        "summary_i18n": p.summary_i18n,
        "tags": p.tags,
        "owner_username": p.owner_username,
        "visibility": p.visibility,
        # The creator's own "still in development" flag - a badge for players, not
        # a visibility state (a beta mod is public and downloadable like any other).
        "is_beta": p.is_beta,
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
        # "Uploaded" = an authored account shared a mod made by someone else; the
        # uploader owns it but `author` credits the named creator ("Uploaded by
        # <owner> · Created by <author>"). Distinct from an authored mod (uploader
        # IS the creator, author empty) and a stray (no owner yet).
        "uploaded_on_behalf": p.uploaded_on_behalf,
        "author": p.author or p.owner_username,
        "updated_at": _iso(p.updated_at),
        # The public "updated" moment: when a build last landed. Null until a mod
        # publishes its first release.
        "last_release_at": _iso(p.last_release_at) if p.last_release_at else None,
        "created_at": _iso(p.created_at),
        **_lineage(p),
    }


async def cards_with_avatars(projects: list[ModProject]) -> list[dict]:
    """``project_card`` per mod, plus the owner's picture for the card byline -
    resolved in two queries for the whole page rather than two per mod."""
    cards = [project_card(p) for p in projects]
    owner_ids = list({p.owner_id for p in projects if p.owner_id is not None})
    if not owner_ids:
        return cards
    users, profiles = await asyncio.gather(
        SiteUser.find(In(SiteUser.id, owner_ids)).to_list(),
        ModProfile.find(In(ModProfile.site_user_id, owner_ids)).to_list(),
    )
    by_user = {pr.site_user_id: pr for pr in profiles}
    urls = {u.id: _profile_avatar_url(u, by_user.get(u.id)) for u in users}
    for card, project in zip(cards, projects, strict=True):
        card["owner_avatar_url"] = urls.get(project.owner_id)
    return cards


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
        "title_i18n": r.title_i18n,
        "changelog": r.changelog,
        "changelog_i18n": r.changelog_i18n,
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
    on_behalf: bool = False, credited_author: str | None = None,
    is_beta: bool = False,
) -> ModProject:
    slug = await _unique_slug(actor.id, title)
    # An "uploaded on behalf" mod credits a named third-party creator and is always
    # releases-only (there's no source workflow for code you didn't write). The
    # uploader stays the owner; `author` holds the credited creator's name. It also
    # carries NO inspiration credit - it isn't the uploader's creative lineage.
    author = ""
    if on_behalf:
        author = (credited_author or "").strip()
        if not author:
            raise APIError(400, ErrorCode.bad_request,
                           "Name the creator this mod was made by.")
        mode = "releases"
        inspired_by = None
    insp = await _resolve_attribution(inspired_by, actor)
    project = ModProject(
        slug=slug, title=title.strip(), summary=summary.strip(),
        description=description, tags=_clean_tags(tags), visibility=visibility,
        is_beta=is_beta, mode=mode, source_visibility=source_visibility,
        owner_id=actor.id, owner_username=actor.display_name or actor.username,
        owner_handle=actor.username,
        uploaded_on_behalf=on_behalf, author=author,
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
    """Visibility gate. Owners + collaborators always see their projects (incl.
    drafts and taken-down ones, flagged); everyone else is bound by visibility +
    takedown."""
    if can_edit(project, viewer):
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
    if can_edit(project, viewer):
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
    if project is None:
        raise _not_found()
    if not can_view(project, viewer):
        # A real draft (not taken down) that this viewer just can't see yet reads
        # as "not public yet"; taken-down / anything else stays a plain not-found.
        if project.visibility == "draft" and not project.taken_down:
            raise _not_public()
        raise _not_found()
    return project


async def _branches_or_default(project: ModProject) -> list[dict]:
    branches = await gitstore.list_branches(str(project.id))
    if not branches:                       # empty repo: surface the default branch
        return [{"name": project.default_branch, "head": None}]
    return branches


async def project_detail(project: ModProject, viewer: SiteUser | None) -> dict:
    # "is_owner" = has edit access (primary owner OR a collaborator); the editor UI
    # keys off it. Collaborator-management + delete additionally need is_primary_owner.
    is_owner = can_edit(project, viewer)
    primary = is_primary_owner(project, viewer)
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
        "description_i18n": project.description_i18n,
        "readme_text": project.readme_text,
        "readme_i18n": project.readme_i18n,
        "warnings": project.warnings,
        "warnings_i18n": project.warnings_i18n,
        "default_branch": project.default_branch,
        "preview_shas": project.preview_shas,
        "discord_url": project.discord_url,
        "website_url": project.website_url,
        "donation_urls": project.donation_urls,
        "taken_down": project.taken_down,
        "takedown_reason": project.takedown_reason if is_owner else None,
        "is_owner": is_owner,
        "is_primary_owner": primary,
        "owner_avatar_url": await owner_avatar_url(project),
        "collaborators": [{"id": str(c.user_id), "username": c.username}
                          for c in (project.collaborators or [])],
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
        query["owner_username"] = _author_eq(author)
    if q:
        # Case-insensitive substring across title/summary/tags/author.
        query.update(_search_clause(q))
    sort_key = _SORTS.get(sort, "-updated_at")
    total = await ModProject.find(query).count()
    docs = await ModProject.find(query).sort(sort_key).skip(offset).limit(limit).to_list()
    return await cards_with_avatars(docs), total


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
    project: ModProject, actor: SiteUser, *, title=None, title_i18n=None,
    summary=None, summary_i18n=None, description=None, description_i18n=None,
    readme_text=None, readme_i18n=None, warnings=None, warnings_i18n=None,
    tags=None, visibility=None, is_beta=None,
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
    _set_i18n(project, {
        "title_i18n": (title_i18n, "title", 120),
        "summary_i18n": (summary_i18n, "summary", 280),
        "description_i18n": (description_i18n, "description", 40_000),
        "readme_i18n": (readme_i18n, "readme_text", 60_000),
        "warnings_i18n": (warnings_i18n, "warnings", 4_000),
    })
    if tags is not None:
        project.tags = _clean_tags(tags)
    if visibility is not None:
        project.visibility = visibility
    if is_beta is not None:
        project.is_beta = is_beta
    if mode is not None:
        # An uploaded-on-behalf mod is release-only by definition (you can't own the
        # source of a mod you merely shared) - never let it flip into files mode.
        if project.uploaded_on_behalf and mode != "releases":
            raise APIError(400, ErrorCode.bad_request,
                           "A mod uploaded on someone's behalf stays releases-only.")
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
    # An uploaded-on-behalf mod stays deliberately bare: it isn't the uploader's
    # work, so no personal links, donation buttons, or inspiration credit ride on
    # it (nor can it flip out of releases-only). Force-cleared after any edit,
    # regardless of what the request tried to set.
    if project.uploaded_on_behalf:
        project.discord_url = None
        project.website_url = None
        project.donation_urls = []
        project.inspired_by_slug = None
        project.inspired_by_handle = None
        project.inspired_by_title = None
        project.inspired_by_owner = None
        project.mode = "releases"
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
    await ContentReport.find(
        ContentReport.target_type == "mod", ContentReport.target_id == project.id
    ).delete()
    await ModStar.find(ModStar.project_id == project.id).delete()
    await gitstore.delete_repo(str(project.id))
    await project.delete()


async def delete_project(project: ModProject, actor: SiteUser) -> None:
    _require_primary_owner(project, actor)   # collaborators can't delete the project
    await _purge_project(project)


# --- collaborators (co-owners) ---------------------------------------------

async def add_collaborator(project: ModProject, actor: SiteUser, username: str) -> dict:
    """Add a co-owner by username (primary owner only). They gain edit rights."""
    _require_primary_owner(project, actor)
    uname = (username or "").strip().lstrip("@").lower()
    if not uname:
        raise APIError(400, ErrorCode.bad_request, "Enter a username to collaborate with.")
    user = await SiteUser.find_one(SiteUser.username == uname)
    if user is None:
        raise APIError(404, ErrorCode.not_found, f"No site user '@{uname}'. They must sign in once first.")
    if user.id == project.owner_id:
        raise APIError(400, ErrorCode.bad_request, "That's the owner.")
    if user.id in _collab_ids(project):
        return await project_detail(project, actor)   # already a collaborator (idempotent)
    if len(project.collaborators) >= 20:
        raise APIError(400, ErrorCode.bad_request, "Too many collaborators (max 20).")
    project.collaborators.append(Collaborator(user_id=user.id, username=user.username))
    project.updated_at = utcnow()
    await project.save()
    return await project_detail(project, actor)


async def remove_collaborator(project: ModProject, actor: SiteUser, user_id: str) -> dict:
    """Remove a collaborator (primary owner only)."""
    _require_primary_owner(project, actor)
    uid = to_oid(user_id)
    project.collaborators = [c for c in project.collaborators if c.user_id != uid]
    project.updated_at = utcnow()
    await project.save()
    return await project_detail(project, actor)


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
        path = _norm_path(raw)
        if path.endswith(BLOCKED_COMMIT_EXTENSIONS):
            raise APIError(400, ErrorCode.bad_request, BLOCKED_COMMIT_MESSAGE)
        adds_norm.append((path, content))
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


# Image content-type → the extension used for the in-.tmod preview path. WebP is
# intentionally excluded: Trove itself can't render a WebP preview, so it must
# never be baked into a .tmod (see _inject_preview's guard).
_PREVIEW_EXT = {"image/png": "png", "image/jpeg": "jpg", "image/gif": "gif"}


def pack_preview(
    files: list[tuple[str, bytes]], props: dict[str, str],
    data: bytes, ext: str, stem: str,
) -> str:
    """Pack ``data`` as the build's preview image at ``ui/<stem>.<ext>``, replacing
    anything already sitting there, and stamp the ``previewPath`` header property -
    the Trove convention the game + mod sites read a preview from. Returns the packed
    path. Shared with the Mod Workshop, so a preview means the same thing either way."""
    path = f"ui/{_safe_filename(stem)}.{ext}".lower()
    files[:] = [(p, b) for (p, b) in files
                if p.replace("\\", "/").lstrip("/").lower() != path]
    files.append((path, data))
    props["previewPath"] = path
    return path


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
    if content_type not in _PREVIEW_EXT:
        raise APIError(400, ErrorCode.bad_request,
                       "The .tmod preview must be a PNG, JPEG, or GIF image (Trove can't render WebP).")
    pack_preview(files, props, data, _PREVIEW_EXT[content_type], project.slug)


# --- Attached config (.cfg) injection --------------------------------------
# A Flash-UI mod reads its settings from a .cfg the game keeps in ModCfgs/, named
# after the mod's title. A modder can attach that config when cutting a release and
# it's packed into the build as `ui/<title>.cfg` - the name the game expects - so
# nobody has to place it by hand. Two rules make this safe:
#   * Only a build that actually ships a .swf can carry one. Nothing else reads a
#     config, so for every other mod the option is refused here and hidden in the UI.
#   * It's baked in ONCE, at build time. The stored artifact IS the injected one, so
#     its hash is stable forever (a download-time rewrite would change the bytes on
#     every fetch and break every hash-based update check).
_CONFIG_MAX_BYTES = 256 * 1024          # a .cfg is text; anything bigger isn't one


def _config_packed_path(props: dict[str, str]) -> str:
    """Where an attached config is packed: ``ui/<title>.cfg`` - the mod's own header
    title, lowercased like every other packed game path (the download endpoint
    restores the title's casing when serving it)."""
    title = str(props.get("title", "")).strip()
    if not title:
        raise APIError(400, ErrorCode.bad_request,
                       "This build has no title in its header to name the config after.")
    return f"ui/{_safe_filename(title)}.cfg".lower()


def _inject_config(
    files: list[tuple[str, bytes]], props: dict[str, str], data: bytes,
) -> str:
    """Pack an attached ``.cfg`` into the build as ``ui/<title>.cfg``, replacing
    anything already sitting at that path, and stamp ``configPath`` so the build
    says which of its files IS the config (a build can pack any number of .cfg
    files; only one of them is the config). Returns the packed path.

    Refused unless the build ships a ``.swf`` - a mod with no Flash UI has nothing
    that would read a config, so the file would just be dead weight in the archive."""
    if not data:
        raise APIError(400, ErrorCode.bad_request, "The config file is empty.")
    if len(data) > _CONFIG_MAX_BYTES:
        raise APIError(400, ErrorCode.bad_request,
                       f"A config file can be at most {_CONFIG_MAX_BYTES // 1024} KB.")
    if b"\x00" in data:            # binary bytes = the wrong file was picked
        raise APIError(400, ErrorCode.bad_request,
                       "That doesn't look like a text config file.")
    if not any(p.lower().endswith(".swf") for p, _ in files):
        raise APIError(400, ErrorCode.bad_request,
                       "Only a mod with a Flash UI (.swf) can carry a config file.")
    path = _config_packed_path(props)
    files[:] = [(p, b) for (p, b) in files
                if p.replace("\\", "/").lstrip("/").lower() != path]
    files.append((path, data))
    props["configPath"] = path
    return path


def _repack_with_config(data: bytes, config_data: bytes) -> tuple[bytes, dict[str, str]]:
    """Rebuild an uploaded ``.tmod`` with an attached config packed in. Everything
    else is preserved - header version, properties (including which builder stamped
    it), and the packed paths verbatim - so the only difference from the modder's own
    build is the added file. Sync + deterministic: the same upload and config always
    produce the same bytes. Returns ``(artifact, header properties)``."""
    try:
        parsed = tmod.read_tmod(data)
    except tmod.TmodError as e:
        raise APIError(400, ErrorCode.bad_request, f"Not a valid .tmod file: {e}")
    props = {str(k): str(v) for k, v in parsed.get("properties", {}).items()}
    files = [(f["path"], base64.b64decode(f["content_base64"])) for f in parsed["files"]]
    _inject_config(files, props, config_data)
    artifact = tmod.build_tmod(
        int(parsed.get("version") or 1), props, files,
        mod_loader=props.get("modLoader") or tmod.KIWI_MOD_LOADER,
        lowercase_paths=False,     # keep the uploader's own packed paths untouched
    )
    # Re-read the built header so what we store is exactly what's in the artifact.
    final = tmod.read_tmod(artifact, metadata_only=True)
    return artifact, {str(k): str(v) for k, v in final.get("properties", {}).items()}


async def _check_release_tag(project: ModProject, branch: str, tag: str) -> None:
    # Tags are unique per branch (variant) - branch X and branch Y can both have v1.0.
    if await ModRelease.find_one(
        ModRelease.project_id == project.id,
        ModRelease.branch == branch,
        ModRelease.tag == tag,
    ):
        raise APIError(409, ErrorCode.conflict,
                       f"Release '{tag}' already exists on branch '{branch}'.")


def _hash_match(shas: tuple[str | None, ...]):
    """Query fragment matching a release by its CURRENT artifact hash or any hash it
    used to have (``prior_tmod_shas``, filled when a build is repacked to carry a
    config). Both sides of the comparison have to consider both - otherwise
    attaching a config would launder a known artifact into a "new" one."""
    want = [s for s in shas if s]
    return Or(In(ModRelease.tmod_sha, want), In(ModRelease.prior_tmod_shas, want))


async def release_by_artifact_hash(sha: str) -> ModRelease | None:
    """The release whose artifact hashes to ``sha`` - matching a repacked build on
    its pre-injection hash too, so the modder's own copy of a build still resolves
    to the release that ships it."""
    return await ModRelease.find_one(_hash_match((sha,)))


async def _ensure_hash_unowned(project: ModProject, *shas: str | None) -> None:
    """A mod's content hash belongs to whoever first released it. Reject a release
    whose exact artifact is already published by a *different* creator (anti-reupload).
    The same owner reusing their own artifact (across branches/projects) is fine."""
    # $nin [None, me]: only a release with a *different, known* owner blocks - a
    # pre-migration release with no owner_id (null) must not falsely flag.
    other = await ModRelease.find_one(
        _hash_match(shas),
        {"owner_id": {"$nin": [None, project.owner_id]}},
    )
    if other is not None:
        raise APIError(409, ErrorCode.conflict,
                       "This exact mod is already published by another creator, so it "
                       "can't be uploaded here. If it's genuinely yours, contact support.")


async def _ensure_hash_globally_unique(project: ModProject, *shas: str | None) -> None:
    """Stricter than :func:`_ensure_hash_unowned`, for *uploaded-on-behalf* mods:
    the exact artifact must not already exist ANYWHERE on the hub - not under a
    different owner, not as a stray (owner_id=None), not even under another of the
    uploader's own projects. Anti-duplicate / re-upload detection: if you're
    sharing someone else's build, it can only live in one place. Reuse across
    branches of THIS same project is still fine (same project_id excluded)."""
    other = await ModRelease.find_one(
        _hash_match(shas),
        ModRelease.project_id != project.id,
    )
    if other is not None:
        raise APIError(409, ErrorCode.conflict,
                       "This mod is already on the hub - the exact same build has been "
                       "uploaded before, so it can't be shared again here.")


def _require_publish_ok(actor: SiteUser, status: str) -> None:
    if status == "published" and not actor.is_verified:
        raise APIError(403, ErrorCode.email_unverified,
                       "Verify your account before publishing a release.")


# The bulk markdown is left OUT of the release event: description + readme_text
# can each run to tens of KB, and every SSE subscriber, webhook post and Redis
# pub/sub hop would carry them on every release. The event's `project.api_url`
# points at the detail endpoint that serves them.
_EVENT_PROJECT_OMIT = ("description", "description_i18n", "readme_text", "readme_i18n")


async def _previous_published_release(release: ModRelease) -> ModRelease | None:
    """The published build this one supersedes, or None if it's the first.

    Scoped to the same BRANCH: branches are mod *variants*, so v2 on ``main`` does
    not supersede v1 on ``alt`` - each variant has its own timeline. Ordered by
    ``published_at`` (created_at breaks ties)."""
    query: dict = {
        "project_id": release.project_id, "branch": release.branch,
        "status": "published", "_id": {"$ne": release.id},
    }
    if release.published_at is not None:
        query["published_at"] = {"$lt": release.published_at}
    found = await ModRelease.find(query).sort("-published_at", "-created_at").limit(1).to_list()
    return found[0] if found else None


def _event_project(project: ModProject) -> dict:
    """The mod card for a release event: the app-facing public DTO (banner +
    gallery image URLs, tags, categories, counts, links, lineage) minus the bulk
    markdown, plus ``owner`` (the key the original payload used) and the detail
    endpoint for everything not inlined."""
    dto = {k: v for k, v in public_mod_dto(project).items() if k not in _EVENT_PROJECT_OMIT}
    return {
        **dto,
        "owner": project.owner_username,
        "mode": project.mode,
        "default_branch": project.default_branch,
        "api_url": (f"{settings.api_url.rstrip('/')}"
                    f"/v1/mods/{project.owner_handle}/{project.slug}"),
    }


def _event_release(release: ModRelease) -> dict:
    """One build for a release event: the app-facing release DTO (tag, changelog,
    filename, sha256, size, download URL) plus its identity and the ``.tmod``
    header, so a consumer can announce or mirror it without a follow-up fetch."""
    props = dict(release.tmod_properties or {})
    return {
        **public_release_dto(release),
        "id": str(release.id),
        "status": release.status,
        "source_commit_sha": release.source_commit_sha,
        # The .tmod header stamped into the artifact (title/author/modVersion/…).
        # Empty for a .zip release, which has no header.
        "properties": props,
        "mod_version": props.get("modVersion") or None,
        "mod_author": props.get("author") or None,
        # This build's own preview image (defaults to the mod banner at create).
        "image_url": _public_img_url(release.banner_sha),
        "created_at": _iso(release.created_at),
    }


def _event_change(
    release: ModRelease, previous: ModRelease | None, release_count: int,
) -> dict:
    """How this build relates to the one it supersedes - enough for a consumer to
    say "v1.2 → v1.3, +40 KB, 12 days later" without walking the release history."""
    size_delta = None if previous is None else release.tmod_size - previous.tmod_size
    days = None
    if previous is not None and release.published_at and previous.published_at:
        elapsed = release.published_at - previous.published_at
        days = round(elapsed.total_seconds() / 86400, 2)
    return {
        "kind": "initial" if previous is None else "update",
        "is_first_release": previous is None,
        "from_tag": previous.tag if previous is not None else None,
        "to_tag": release.tag,
        "size_delta": size_delta,
        "days_since_previous": days,
        # Published builds on this branch, this one included.
        "release_count": release_count,
        # A re-tag: identical artifact bytes, so nothing actually changed in-game.
        "artifact_unchanged": previous is not None and previous.tmod_sha == release.tmod_sha,
    }


async def _emit_release_event(project: ModProject, release: ModRelease) -> None:
    """Fire a ``mod_release`` event on the live SSE stream (``/v1/events/stream``)
    so external apps can react to new releases without polling.

    Carries the whole announcement: the public mod card (image URLs included), the
    build's identity + ``.tmod`` header, and the release it supersedes - so a
    consumer can render or mirror it with no follow-up request.

    PUBLIC + PUBLISHED only. A draft, unlisted or taken-down mod is never announced
    on a public firehose: unlisted means link-only, and for a draft the event's own
    download URL 404s for every subscriber anyway. Best-effort - any failure here
    leaves release creation untouched. Signature = the release id, so each release
    announces exactly once."""
    if release.status != "published":
        return
    if project.visibility != "public" or project.taken_down:
        return
    try:
        from app.events import bus
        api = settings.api_url.rstrip("/")
        site = settings.app_url.rstrip("/")
        previous = await _previous_published_release(release)
        release_count = await ModRelease.find({
            "project_id": release.project_id, "branch": release.branch,
            "status": "published",
        }).count()
        release_out = _event_release(release)
        data = {
            "project": _event_project(project),
            "release": release_out,
            "previous": _event_release(previous) if previous is not None else None,
            "change": _event_change(release, previous, release_count),
            # The one image to lead an announcement with: this build's preview,
            # else the mod banner, else the first gallery shot.
            "image_url": (release_out["image_url"]
                          or _public_img_url(project.banner_sha)
                          or next((u for u in (_public_img_url(s)
                                               for s in project.preview_shas) if u), None)),
            # Kept at the top level: the original payload's shape, which existing
            # webhook templates and external consumers are built against.
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
    config_data: bytes | None = None,
    title_i18n: dict | None = None, changelog_i18n: dict | None = None,
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
        if config_data:
            raise APIError(400, ErrorCode.bad_request,
                           "A config file can only be packed into a .tmod build.")
        artifact = await asyncio.to_thread(_build_zip, files)
        props: dict[str, str] = {}
    else:
        fmt = "tmod"
        props = _release_properties(project, tag, changelog, author=author)
        # A chosen preview is embedded as ui/<slug>.<ext> (not committed to the repo).
        # Zips carry no header properties, so the preview only applies to .tmod.
        if preview_sha:
            await _inject_preview(project, preview_sha, files, props)
        # An attached config is packed as ui/<title>.cfg - same deal: it lives in
        # the build only, never in the repo. Refused if the mod ships no .swf.
        if config_data:
            _inject_config(files, props, config_data)
        artifact = await asyncio.to_thread(tmod.build_tmod, 1, props, files)
    sha, _ = await store.put_blob(artifact)
    await _ensure_hash_unowned(project, sha)
    return await _insert_release(
        project, tag=tag, branch=branch_name, title=title, changelog=changelog,
        status=status, tmod_sha=sha, tmod_size=len(artifact), properties=props,
        source_commit_sha=commit_sha, release_format=fmt,
        title_i18n=title_i18n, changelog_i18n=changelog_i18n,
    )


async def create_release_from_upload(
    project: ModProject, actor: SiteUser, *, tag: str, title: str,
    changelog: str, status: str, filename: str, data: bytes, branch: str = "",
    config_data: bytes | None = None,
    title_i18n: dict | None = None, changelog_i18n: dict | None = None,
) -> dict:
    _require_owner(project, actor)
    _require_publish_ok(actor, status)
    tag = tag.strip()
    branch_name = branch.strip() or project.default_branch
    await _check_release_tag(project, branch_name, tag)
    # Accept an already-compiled .tmod OR a .zip (both modes can upload these).
    is_zip = data[:2] == b"PK" or (filename or "").lower().endswith(".zip")
    props: dict[str, str] = {}
    # The hash of what the modder actually uploaded. It only differs from the
    # release's own hash when a config was packed in below - and it's kept so that
    # copy stays recognisable to hash lookup (see ModRelease.prior_tmod_shas).
    base_sha: str | None = None
    if is_zip:
        fmt = "zip"
        if config_data:
            raise APIError(400, ErrorCode.bad_request,
                           "A config file can only be packed into a .tmod build.")
        if not zipfile.is_zipfile(io.BytesIO(data)):
            raise APIError(400, ErrorCode.bad_request, "Not a valid .zip file.")
    else:
        fmt = "tmod"
        try:
            parsed = tmod.read_tmod(data, metadata_only=True)
        except tmod.TmodError as e:
            raise APIError(400, ErrorCode.bad_request, f"Not a valid .tmod file: {e}")
        props = {str(k): str(v) for k, v in parsed.get("properties", {}).items()}
        if config_data:
            base_sha = store.blob_sha(data)
            data, props = await asyncio.to_thread(_repack_with_config, data, config_data)
    sha, _ = await store.put_blob(data)
    if base_sha == sha:               # nothing actually changed; keep one hash
        base_sha = None
    # Uploaded-on-behalf mods (sharing someone else's build) must be globally
    # unique; ordinary mods only can't collide with a *different* owner's artifact.
    # Both hashes are checked: a config must not launder a known build into a new one.
    if project.uploaded_on_behalf:
        await _ensure_hash_globally_unique(project, sha, base_sha)
    else:
        await _ensure_hash_unowned(project, sha, base_sha)
    return await _insert_release(
        project, tag=tag, branch=branch_name,
        title=title, changelog=changelog, status=status,
        tmod_sha=sha, tmod_size=len(data), properties=props,
        source_commit_sha=None, release_format=fmt,
        prior_tmod_shas=[base_sha] if base_sha else [],
        title_i18n=title_i18n, changelog_i18n=changelog_i18n,
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
    release_format: str = "tmod", prior_tmod_shas: list[str] | None = None,
    title_i18n: dict | None = None, changelog_i18n: dict | None = None,
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
        prior_tmod_shas=prior_tmod_shas or [], tmod_filename=filename,
        tmod_properties=properties,
        banner_sha=project.banner_sha, status=status,
        published_at=utcnow() if status == "published" else None,
    )
    _set_i18n(release, {
        "title_i18n": (title_i18n, "title", 160),
        "changelog_i18n": (changelog_i18n, "changelog", 20_000),
    })
    await release.insert()
    project.updated_at = utcnow()
    if release.status == "published":
        project.last_release_at = release.published_at or utcnow()
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
    oid = to_oid(release_id)
    return await ModRelease.get(oid) if oid else None


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


# --- Blueprint (.blueprint voxel model) preview --------------------------------
# A release's .tmod can carry .blueprint models. We surface them for an in-browser
# 3D viewer: list the blueprint paths, and decode one to a compact voxel payload
# (reusing the proven catalog-render decoder in app/trove/render/voxel.py).

def _list_blueprints_sync(tmod_bytes: bytes) -> dict:
    """Sync: the release's non-empty blueprint items + their basenames (``fns``). Rig
    resolution happens in the async caller (it needs Postgres)."""
    from app.trove.render.voxel import is_empty_blueprint

    parsed = tmod.read_tmod(tmod_bytes)
    out: list[dict] = []
    fns: list[str] = []
    for f in parsed["files"]:
        p = f["path"].lower()
        if not p.endswith(".blueprint"):
            continue
        raw = base64.b64decode(f["content_base64"])
        if is_empty_blueprint(raw):
            continue                                 # skip 0-voxel placeholder parts
        out.append({"path": f["path"], "size": f["size"]})
        fns.append(p.split("/")[-1][:-len(".blueprint")])
    return {"items": out, "fns": fns}


async def _resolve_rig(fns: list[str]) -> tuple[str | None, list[str], set[str]]:
    """Which baked creature rig these blueprint basenames assemble onto, its animation
    names, and the SET of basenames that are components of that assembled model.

    AUTHORITATIVE ONLY - the live binfab map (via the codex), which knows each
    blueprint's exact creature + attach point from the game's own prefab data. There is
    NO name-overlap heuristic fallback: if the rig isn't known (creature not in the
    archive, or no baked rig), we return nothing and the page just shows the individual
    blueprints. Better to not render than to render onto a guessed/wrong skeleton."""
    if not fns:
        return None, [], set()
    from app.trove.mods_hub import assembly, rig_index
    skeleton, attach = await rig_index.resolve(fns)
    if skeleton and assembly.has_baked_rig(skeleton):
        return skeleton, assembly.animations_for(skeleton), set(attach)
    return None, [], set()


async def list_release_blueprints(release: ModRelease) -> dict:
    """The NON-EMPTY .blueprint models inside a release's .tmod (for the 3D-view
    affordance) + whether they assemble onto a known creature rig. Each item gets
    ``assembled`` (is it a component of the assembled model) so the UI can fold the
    component blueprints under the 'assembled' button. Empty placeholder blueprints
    (unused body parts, 0 voxels) are skipped. CPU-bound, off the loop."""
    if release.release_format != "tmod":
        return {"items": [], "rig": None, "animations": []}
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return {"items": [], "rig": None, "animations": []}
    try:
        base = await asyncio.to_thread(_list_blueprints_sync, data)
    except tmod.TmodError:
        return {"items": [], "rig": None, "animations": []}
    rig, anims, components = await _resolve_rig(base["fns"])
    for item, fn in zip(base["items"], base["fns"], strict=False):
        item["assembled"] = fn in components
    return {"items": base["items"], "rig": rig, "animations": anims}


async def assemble_release_model(
    release: ModRelease, fmt: str = "json",
) -> bp_cache.Cached | None:
    """Assemble the release's blueprint parts onto their matching creature rig ->
    the web-viewer model payload (rest pose + animations). None if no parts / no rig.

    The rig + per-part attach points are resolved AUTHORITATIVELY from the game's
    prefab binfabs (``rig_index.resolve``). There is NO name-overlap heuristic:
    ``assembly.assemble`` skips any part the binfab map doesn't place (no-guess rig).

    The heaviest payload the viewers ask for - every part decoded and placed - so
    it's cached like a single blueprint, keyed on the artifact plus the rig map's
    signature (see ``bp_cache.key_for_assembly``)."""
    if release.release_format != "tmod":
        return None
    from app.trove.mods_hub import rig_index

    async def build() -> dict:
        data = await store.get_blob(release.tmod_sha)
        if data is None:
            raise bp_cache.NoPayload

        def _read(b: bytes):
            files = tmod.read_tmod(b)["files"]
            basenames = [f["path"].split("/")[-1][:-len(".blueprint")].lower()
                         for f in files if f["path"].lower().endswith(".blueprint")]
            return files, basenames
        files, basenames = await asyncio.to_thread(_read, data)
        skeleton, attach = await rig_index.resolve(basenames)

        def _work():
            from app.trove.mods_hub import assembly
            return assembly.assemble(files, rig_name=skeleton, ap_overrides=attach)
        model = await asyncio.to_thread(_work)
        if model is None:
            raise bp_cache.NoPayload
        return model

    sig = await rig_index.index_signature()
    try:
        if sig is None:                  # no live rig map to pin the result to
            return await bp_cache.build_uncached(build, fmt)
        return await bp_cache.get_or_build(
            bp_cache.key_for_assembly(sig, f"tmod:{release.tmod_sha}"), build, fmt)
    except bp_cache.NoPayload:
        return None


async def load_rig_animation(skeleton: str, name: str) -> bytes:
    """One baked animation clip for a creature rig, lazily (the model viewer fetches these
    on demand). The assembled-model payload only carries animation metadata, so the frames
    live here, keyed by skeleton (shared across every mod using that rig). Returns the raw
    ``TANIM1`` bytes; the viewer decodes them."""
    from app.trove.mods_hub import assembly
    anim = await asyncio.to_thread(assembly.load_animation, skeleton, name)
    if anim is None:
        raise _not_found("No such rig animation")
    return anim


async def load_rig_animation_graph(skeleton: str) -> bytes:
    """The rig's animation state machine as JSON (see assembly.load_animation_graph).
    404s for a rig that has none, which the viewer treats as "just list the clips"."""
    from app.trove.mods_hub import assembly
    graph = await asyncio.to_thread(assembly.load_animation_graph, skeleton)
    if graph is None:
        raise _not_found("No such rig animation graph")
    return graph


def _preview_path(properties: dict) -> str:
    """The release's preview image path inside the .tmod (excluded from the file list),
    lowercased; '' if none."""
    return (properties.get("previewPath") or "").strip().lower()


async def list_release_files(release: ModRelease) -> dict:
    """The files packed inside a release's .tmod (path + size), EXCLUDING the preview
    image - for the per-file download UI. Metadata-only read (no decompression)."""
    if release.release_format != "tmod":
        return {"items": []}
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return {"items": []}

    def _work(b: bytes) -> dict:
        parsed = tmod.read_tmod(b, metadata_only=True)
        preview = _preview_path(parsed["properties"])
        items = [{"path": f["path"], "size": f["size"]} for f in parsed["files"]
                 if f["path"].lower() != preview]
        items.sort(key=lambda f: f["path"].lower())
        return {"items": items}
    try:
        return await asyncio.to_thread(_work, data)
    except tmod.TmodError:
        return {"items": []}


async def inspect_release(release: ModRelease) -> dict:
    """A release's artifact, decoded: the ``.tmod`` header (version, every property,
    the decoded category flags) plus every packed file with its size - or a ``.zip``
    release's entries. This is the whole archive, the preview image included, so the
    mod page's inspector shows what's really in the build; ``preview_path`` and
    ``config_path`` mark the two entries the header gives a meaning to.

    Metadata-only read - the file table lives in the header, so nothing is
    decompressed. ``readable`` is false if the artifact is missing or won't parse."""
    out: dict = {
        "format": release.release_format, "tag": release.tag, "branch": release.branch,
        "filename": release_download_filename(release),
        "sha256": release.tmod_sha, "prior_sha256s": list(release.prior_tmod_shas or []),
        "size": release.tmod_size, "version": None, "properties": {},
        "categories": [], "flags": 0, "preview_path": "", "config_path": "",
        "files": [], "file_count": 0, "total_size": 0, "readable": False,
    }
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return out

    def _work(b: bytes) -> dict:
        if release.release_format == "zip":
            with zipfile.ZipFile(io.BytesIO(b)) as z:
                files = [{"path": i.filename, "size": i.file_size}
                         for i in z.infolist() if not i.is_dir()]
            return {"readable": True, "files": files}
        parsed = tmod.read_tmod(b, metadata_only=True)
        props = {str(k): str(v) for k, v in parsed["properties"].items()}
        return {
            "readable": True,
            "version": parsed.get("version"),
            "properties": props,
            "categories": parsed.get("categories") or [],
            "flags": parsed.get("flags") or 0,
            "preview_path": _preview_path(props),
            "config_path": _declared_config_path(props),
            "files": [{"path": f["path"], "size": f["size"]} for f in parsed["files"]],
        }
    try:
        out.update(await asyncio.to_thread(_work, data))
    except (tmod.TmodError, zipfile.BadZipFile, OSError, ValueError):
        return out
    out["files"].sort(key=lambda f: f["path"].lower())
    out["file_count"] = len(out["files"])
    out["total_size"] = sum(int(f["size"] or 0) for f in out["files"])
    return out


async def download_release_file(release: ModRelease, path: str) -> tuple[bytes, str]:
    """Bytes of ONE file inside a release's .tmod (individual download). The preview
    image is not downloadable here. Returns ``(data, download_filename)``."""
    if release.release_format != "tmod":
        raise _not_found("This release has no individually downloadable files.")
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        raise _not_found("Release artifact not found")
    want = path.strip()

    def _work(b: bytes) -> bytes | None:
        parsed = tmod.read_tmod(b)
        if want.lower() == _preview_path(parsed["properties"]):
            return None                                   # preview is excluded
        target = next((f for f in parsed["files"] if f["path"] == want), None)
        if target is None or "content_base64" not in target:
            return None
        return base64.b64decode(target["content_base64"])
    raw = await asyncio.to_thread(_work, data)
    if raw is None:
        raise _not_found("No such file in this release")
    return raw, want.rsplit("/", 1)[-1]


# --- Packed config (.cfg) --------------------------------------------------
# Some mods ship a .cfg alongside their content (scraper mods especially). It's
# needed on its own - it goes in the game's ModCfgs/ folder, not the mods folder -
# so the mod page offers it as its own download, read out of the .tmod on the fly
# (nothing extra is stored: the artifact in the CAS is the only copy).
#
# `configPath` is the header property that says WHICH packed path is the config.
# It settles the otherwise unanswerable case of a build that packs several .cfg
# files: the declared one is the config, the rest are just files.

def _declared_config_path(props: dict) -> str:
    """The packed path the artifact declares as its config (``configPath``),
    normalized; ``''`` when it declares none."""
    declared = next((str(v) for k, v in (props or {}).items() if k.lower() == "configpath"), "")
    return declared.replace("\\", "/").lstrip("/").strip().lower()


def _cfg_download_name(release: ModRelease, path: str) -> str:
    """The name to save one packed .cfg under. The packed path is authoritative,
    except for CASE: ``build_tmod`` lowercases inner paths, while the game names the
    live cfg after the mod's title. So restore the case we KNOW - from the artifact's
    ``title`` when the packed name is that title, else from a ``configPath`` that
    names this same file - and otherwise serve the packed name verbatim (a guess is
    worse than the name that's actually in the archive)."""
    base = path.rsplit("/", 1)[-1]
    props = release.tmod_properties or {}
    title = str(props.get("title", "")).strip()
    if title and base.lower() == f"{title.lower()}.cfg":
        return f"{_safe_filename(title)}.cfg"
    declared_base = _declared_config_path(props).rsplit("/", 1)[-1]
    if declared_base and declared_base == base.lower():
        # Case comes from the property's own value, not its lowercased form.
        raw = next((str(v) for k, v in props.items() if k.lower() == "configpath"), "")
        return _safe_filename(raw.replace("\\", "/").rsplit("/", 1)[-1].strip())
    return _safe_filename(base)


async def list_release_cfgs(release: ModRelease) -> dict:
    """The ``.cfg`` config files packed inside a release's .tmod (path + size + the
    name to save it under) - drives the mod page's config download button.
    Metadata-only read (no decompression).

    ``declared`` marks the file the artifact's ``configPath`` points at, and that
    one sorts first: a build that packs a hundred .cfg files still has exactly one
    config, and this is how we know which.

    ``has_flash_ui`` says whether the build ships a ``.swf`` - the same gate that
    decides whether a config may be attached at all, so the owner's "attach a
    config" action can be shown or hidden off this one call."""
    empty = {"items": [], "has_flash_ui": False}
    if release.release_format != "tmod":
        return empty
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return empty

    def _work(b: bytes) -> dict:
        parsed = tmod.read_tmod(b, metadata_only=True)
        declared = _declared_config_path(parsed["properties"])
        items = [{"path": f["path"], "size": f["size"],
                  "filename": _cfg_download_name(release, f["path"]),
                  "declared": f["path"].lower() == declared}
                 for f in parsed["files"] if f["path"].lower().endswith(".cfg")]
        items.sort(key=lambda f: (not f["declared"], f["path"].lower()))
        return {"items": items,
                "has_flash_ui": any(f["path"].lower().endswith(".swf")
                                    for f in parsed["files"])}
    try:
        return await asyncio.to_thread(_work, data)
    except tmod.TmodError:
        return empty


async def attach_config_to_release(
    release: ModRelease, project: ModProject, actor: SiteUser, data: bytes,
) -> dict:
    """Repack an EXISTING release's artifact to carry an attached config.

    This rewrites a build that's already published, which is deliberately not what
    cutting a release does - so the UI puts a warning in front of it. What it costs,
    exactly: the release gets a new ``tmod_sha``, and anyone who already installed
    the old bytes is NOT told to re-download. Their copy keeps resolving to this
    release (the superseded hash moves into ``prior_tmod_shas``, which hash lookup
    matches), so the desktop app still reports it as installed and does not invent a
    phantom update - it simply won't hand them the config. Someone who wants the
    config to reach existing installs should cut a NEW release instead.

    The old artifact bytes stay in the content store (immutable, shared, and still
    referenced by anything that pinned that hash); only this release moves on."""
    _require_owner(project, actor)
    if release.release_format != "tmod":
        raise APIError(400, ErrorCode.bad_request,
                       "A config file can only be packed into a .tmod build.")
    current = await store.get_blob(release.tmod_sha)
    if current is None:
        raise _not_found("Release artifact not found")
    artifact, props = await asyncio.to_thread(_repack_with_config, current, data)
    sha, _ = await store.put_blob(artifact)
    if sha == release.tmod_sha:                     # byte-identical: nothing to do
        return {**_release_dto(release), "changed": False}
    # The new bytes must still be nobody else's build.
    if project.uploaded_on_behalf:
        await _ensure_hash_globally_unique(project, sha)
    else:
        await _ensure_hash_unowned(project, sha)
    prior = list(release.prior_tmod_shas or [])
    if release.tmod_sha not in prior:
        prior.append(release.tmod_sha)
    release.prior_tmod_shas = prior
    release.tmod_sha = sha
    release.tmod_size = len(artifact)
    release.tmod_properties = props
    release.tmod_filename = release_download_filename(release)
    release.updated_at = utcnow()
    await release.save()
    return {**_release_dto(release), "changed": True}


async def download_release_cfg(release: ModRelease, path: str) -> tuple[bytes, str]:
    """Bytes of ONE packed ``.cfg``, extracted from the .tmod on the fly. Returns
    ``(data, download_filename)``."""
    want = path.strip()
    if not want.lower().endswith(".cfg"):
        raise _not_found("No such config file in this release")
    data, _ = await download_release_file(release, want)
    return data, _cfg_download_name(release, want)


# ---------------------------------------------------------------------------
# VFX previews (PopcornFX .pkfx) - see app/trove/mods_hub/vfx.py
# ---------------------------------------------------------------------------
# A mod's .pkfx rarely bundles the textures/meshes it needs; those come from the
# live game tree (the updates archive). We hand the web viewer the .pkfx text plus
# every referenced asset, resolving each bundled-first then from the game.
#
# Cache (keyed by the release's content-addressed .tmod sha) of the SET of asset
# basenames any .pkfx in the release references - this authorizes the /asset
# endpoint so it isn't an open game-file proxy.
_VFX_DEPSET_CACHE: dict[str, set[str]] = {}
_VFX_DEPSET_ORDER: list[str] = []
_VFX_DEPSET_MAX = 256


def _vfx_remember_depset(sha: str, deps: set[str]) -> None:
    if sha in _VFX_DEPSET_CACHE:
        return
    _VFX_DEPSET_CACHE[sha] = deps
    _VFX_DEPSET_ORDER.append(sha)
    while len(_VFX_DEPSET_ORDER) > _VFX_DEPSET_MAX:
        _VFX_DEPSET_CACHE.pop(_VFX_DEPSET_ORDER.pop(0), None)


def _tmod_pkfx_and_index(tmod_bytes: bytes) -> tuple[list[dict], dict[str, bytes]]:
    """``(pkfx_items, basename_index)`` for a .tmod. ``pkfx_items`` = [{path,size}] per
    ``.pkfx``; ``basename_index`` = {basename.lower(): raw bytes} for every file, so a
    .pkfx reference resolves to a bundled file by name."""
    parsed = tmod.read_tmod(tmod_bytes)
    pkfx_items: list[dict] = []
    index: dict[str, bytes] = {}
    for f in parsed["files"]:
        p = f["path"]
        bn = p.replace("\\", "/").rsplit("/", 1)[-1].lower()
        if "content_base64" in f:
            index.setdefault(bn, base64.b64decode(f["content_base64"]))
        if p.lower().endswith(".pkfx"):
            pkfx_items.append({"path": p, "size": f["size"]})
    return pkfx_items, index


async def list_release_vfx(release: ModRelease) -> dict:
    """The ``.pkfx`` particle effects inside a release's .tmod (drives the VFX-preview
    affordance). ``{items:[{path,size}]}``; empty for non-.tmod releases."""
    if release.release_format != "tmod":
        return {"items": []}
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return {"items": []}
    try:
        items, _ = await asyncio.to_thread(_tmod_pkfx_and_index, data)
    except tmod.TmodError:
        return {"items": []}
    items.sort(key=lambda f: f["path"].lower())
    return {"items": items}


# ── sound banks ────────────────────────────────────────────────────────────
#
# A mod that ships a ``.bnk`` is a sound mod, and the release page can play it.
# Read in the two steps ``app.trove.audio`` was built for: index the bank (cheap,
# cached under its content hash, decodes nothing), then decode ONE sound when a
# visitor presses play.
#
# The embed viewer (app/embed) drives the same three helpers for its ``release=``
# source, so both surfaces share one implementation and one cache entry.


def _tmod_banks_sync(tmod_bytes: bytes) -> list[dict]:
    """The ``.bnk`` files a mod bundles. Header-only - listing them must not
    decompress the file stream of a sound mod that may be tens of megabytes."""
    parsed = tmod.read_tmod(tmod_bytes, metadata_only=True)
    items = [{"path": f["path"], "size": f["size"]}
             for f in parsed["files"] if f["path"].lower().endswith(".bnk")]
    items.sort(key=lambda f: f["path"].lower())
    return items


def _tmod_bank_bytes_sync(tmod_bytes: bytes, path: str) -> tuple[bytes, str | None] | None:
    """``(bank bytes, sidecar text)`` for one ``.bnk`` bundled in a mod.

    The sidecar is the ``.txt`` Wwise writes beside a bank; it carries every
    sound's name, so a mod that ships one gets named sounds instead of ids."""
    from app.trove.mods_hub import vfx

    files = tmod.read_tmod(tmod_bytes)["files"]
    want = vfx.basename(path or "").lower()
    hit = next((f for f in files if f["path"].lower().endswith(".bnk")
                and vfx.basename(f["path"]).lower() == want), None)
    if hit is None:
        return None
    raw = base64.b64decode(hit["content_base64"])
    stem = hit["path"][: -len(".bnk")].lower()
    side = next((f for f in files if f["path"].lower() == f"{stem}.txt"), None)
    text = (base64.b64decode(side["content_base64"]).decode("utf-8", "replace")
            if side else None)
    return raw, text


async def list_release_banks(release: ModRelease) -> dict:
    """The ``.bnk`` sound banks inside a release's .tmod (drives the sound-preview
    affordance). ``{items:[{path,size}]}``; empty for non-.tmod releases."""
    if release.release_format != "tmod":
        return {"items": []}
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        return {"items": []}
    try:
        return {"items": await asyncio.to_thread(_tmod_banks_sync, data)}
    except tmod.TmodError:
        return {"items": []}


async def tmod_bank_index(tmod_bytes: bytes, path: str) -> dict:
    """The cached sound index for one bank bundled in a .tmod.

    Keyed by the bank's own content hash (plus the sidecar's), so a bank that ships
    unchanged across releases - or is embedded on a dozen partner pages - is indexed
    once, and the index is shared with the ``/updates`` sound browser."""
    import hashlib

    from app.trove.audio import service as audio_service

    try:
        got = await asyncio.to_thread(_tmod_bank_bytes_sync, tmod_bytes, path)
    except tmod.TmodError:
        raise APIError(400, ErrorCode.bad_request,
                       "That file isn't a readable .tmod.") from None
    if got is None:
        raise APIError(404, ErrorCode.not_found, "No such sound bank in this mod.")
    raw, sidecar = got
    sidecar_sha = (hashlib.sha256(sidecar.encode("utf-8", "replace")).hexdigest()
                   if sidecar else None)
    return await audio_service.manifest(
        raw, hashlib.sha256(raw).hexdigest(), sidecar, sidecar_sha)


async def release_bank_index(release: ModRelease, path: str) -> dict:
    """Every sound in one of a release's banks - names, codecs, durations, nothing
    decoded. The store hash each sound is filed under is an internal handle, so it is
    dropped here; ``release_sound`` resolves ids against this same index."""
    data = await _release_tmod_or_404(release)
    payload = await tmod_bank_index(data, path)
    return {
        "path": path,
        "bank": payload.get("bank"),
        "sounds": [{k: v for k, v in s.items() if k != "sha"}
                   for s in payload.get("sounds", [])],
        "count": payload.get("count"),
        "playable": payload.get("playable"),
        "total_duration": payload.get("total_duration"),
    }


async def release_sound(
    release: ModRelease, path: str, sound_id: int, raw: bool = False,
) -> tuple[bytes, str, str, str]:
    """One sound as ``(bytes, media type, filename, etag)``. Decoded to Ogg or WAV
    by default; ``raw`` hands back the game's own ``.wem``."""
    data = await _release_tmod_or_404(release)
    payload = await tmod_bank_index(data, path)
    return await sound_from_index(payload, sound_id, raw)


async def sound_from_index(
    payload: dict, sound_id: int, raw: bool = False,
) -> tuple[bytes, str, str, str]:
    """Decode one sound out of an already-built bank index. Shared with app/embed,
    which reaches the same banks through its own source resolution."""
    from app.trove.audio import service as audio_service

    sound = next((s for s in payload.get("sounds", []) if s["id"] == sound_id), None)
    if sound is None:
        raise APIError(404, ErrorCode.not_found, "No such sound in this bank.")

    if raw:
        data = await audio_service.raw_bytes(sound["sha"])
        media, extension = "audio/vnd.wave", "wem"
    else:
        if sound.get("error"):
            raise APIError(422, ErrorCode.bad_request, sound["error"])
        decoded = await audio_service.audio_bytes(sound["sha"])
        data, media, extension = decoded if decoded else (None, "", "")
    if data is None:
        raise APIError(404, ErrorCode.not_found, "Sound blob missing from the store.")

    stem = sound.get("name") or f"sound_{sound_id}"
    stem = "".join(c if c.isalnum() or c in "._-" else "_" for c in stem) or "sound"
    return data, media, f"{stem}.{extension}", f'"{sound["sha"]}{"-raw" if raw else ""}"'


async def _release_tmod_or_404(release: ModRelease) -> bytes:
    if release.release_format != "tmod":
        raise APIError(404, ErrorCode.not_found, "That release has no sounds to play.")
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        raise APIError(404, ErrorCode.not_found, "Release artifact not found.")
    return data


async def _game_vfx_resolver():
    """``(lookup, read, available)`` for the live game tree:
      - ``lookup(basename_lower) -> game_path | None``
      - ``read(game_path) -> bytes | None``
      - ``available``: can either source serve files?
    Production source is the updates archive (``game_file_map`` -> CAS blob);
    ``settings.pkfx_dev_vfx_dir`` is a local fallback for development."""
    from app.core.config import settings
    from app.trove.mods_hub.trove_layout import LIVE_BRANCH, game_file_map
    from app.trove.updates import read as updates_read
    from app.trove.updates.cas import ContentStore

    fmap = await game_file_map(LIVE_BRANCH)               # {basename.lower(): canonical path}
    cas = ContentStore(settings.trove_update_store_dir)

    dev_index: dict[str, str] = {}
    if settings.pkfx_dev_vfx_dir:
        import os
        for root, _dirs, names in os.walk(settings.pkfx_dev_vfx_dir):
            for n in names:
                dev_index.setdefault(n.lower(), os.path.join(root, n))

    def lookup(bn: str) -> str | None:
        if bn in fmap:
            return fmap[bn]
        if bn in dev_index:
            return "dev::" + dev_index[bn]
        return None

    async def read(game_path: str) -> bytes | None:
        if game_path.startswith("dev::"):
            real = game_path[len("dev::"):]
            return await asyncio.to_thread(lambda: open(real, "rb").read())
        meta = await updates_read.get_file_meta(LIVE_BRANCH, game_path)
        if not meta:
            return None
        return cas.get(meta["content_sha256"])

    return lookup, read, (bool(fmap) or bool(dev_index))


async def _build_vfx_depset(index: dict[str, bytes], lookup, read) -> set[str]:
    """Every asset basename referenced by ANY .pkfx in the release, walked recursively
    (a nested child .pkfx pulls in its own deps); nested effects resolve from the mod
    bundle first, else the game tree."""
    from app.trove.mods_hub import vfx
    deps: set[str] = set()
    seen: set[str] = set()
    queue: list[str] = [bn for bn in index if bn.endswith(".pkfx")]
    while queue:
        bn = queue.pop()
        if bn in seen:
            continue
        seen.add(bn)
        raw = index.get(bn)
        if raw is None:
            gp = lookup(bn)
            raw = await read(gp) if gp else None
        if raw is None:
            continue
        for ref in vfx.extract_refs(raw.decode("utf-8", "replace")):
            rbn = vfx.basename(ref).lower()
            deps.add(rbn)
            if rbn.endswith(".pkfx") and rbn not in seen:
                queue.append(rbn)
    return deps


async def get_release_vfx_manifest(release: ModRelease, path: str) -> dict:
    """One effect's ``.pkfx`` text + its resolved asset dependencies for the web viewer.
    Each direct dep is classified ``mod`` / ``game`` / ``missing`` (matched by basename).
    Also primes the recursive dep-set cache that authorizes ``/asset``."""
    from app.trove.mods_hub import vfx
    if release.release_format != "tmod":
        raise _not_found("This release has no VFX.")
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        raise _not_found("Release artifact not found")
    _items, index = await asyncio.to_thread(_tmod_pkfx_and_index, data)
    pkfx_raw = index.get(vfx.basename(path).lower())
    if pkfx_raw is None:
        raise _not_found("VFX file not found in this release.")
    pkfx_text = pkfx_raw.decode("utf-8", "replace")

    lookup, read, available = await _game_vfx_resolver()
    deps: list[dict] = []
    for ref in vfx.extract_refs(pkfx_text):
        bn = vfx.basename(ref).lower()
        source = "mod" if bn in index else ("game" if lookup(bn) else "missing")
        deps.append({"ref": ref, "basename": vfx.basename(ref), "source": source})

    _vfx_remember_depset(release.tmod_sha, await _build_vfx_depset(index, lookup, read))
    return {
        "path": path,
        "pkfx": pkfx_text,
        "deps": deps,
        "missing": [d["basename"] for d in deps if d["source"] == "missing"],
        "game_available": available,
    }


async def get_release_vfx_asset(release: ModRelease, ref: str) -> tuple[bytes, str]:
    """Bytes of one asset a release's VFX references - bundled-first, else the live game
    tree. Authorized against the release's .pkfx dependency set (resolved by basename),
    so it is not an open game-file proxy. Returns ``(data, media_type)``."""
    from app.trove.mods_hub import vfx
    if release.release_format != "tmod":
        raise _not_found("This release has no VFX.")
    data = await store.get_blob(release.tmod_sha)
    if data is None:
        raise _not_found("Release artifact not found")
    bn = vfx.basename(ref).lower()
    _items, index = await asyncio.to_thread(_tmod_pkfx_and_index, data)
    lookup, read, _available = await _game_vfx_resolver()
    depset = _VFX_DEPSET_CACHE.get(release.tmod_sha)
    if depset is None:
        depset = await _build_vfx_depset(index, lookup, read)
        _vfx_remember_depset(release.tmod_sha, depset)
    if bn not in depset and bn not in index:
        raise _not_found("Asset not referenced by this release's VFX.")
    raw = index.get(bn)
    if raw is None:
        gp = lookup(bn)
        raw = await read(gp) if gp else None
    if raw is None:
        raise _not_found("Asset not found (not bundled and not in the game tree).")
    return raw, vfx.media_type_for(ref)


def _decode_blueprint_payload(tmod_bytes: bytes, want_path: str) -> dict:
    """Sync: find ``want_path`` in the .tmod and pack its voxels into the compact
    parallel-array payload the web viewer consumes (shared with /updates)."""
    from app.trove.render.voxel import (
        BlueprintEmpty,
        BlueprintError,
        BlueprintTooLarge,
        pack_blueprint,
    )

    parsed = tmod.read_tmod(tmod_bytes)
    want = want_path.strip().lower()
    target = next((f for f in parsed["files"]
                   if f["path"].lower() == want and want.endswith(".blueprint")), None)
    if target is None:
        raise _not_found("No such blueprint in this release")
    raw = base64.b64decode(target["content_base64"])
    try:
        return pack_blueprint(raw, target["path"])
    except BlueprintTooLarge as e:
        raise APIError(413, ErrorCode.bad_request, str(e))
    except BlueprintEmpty as e:
        raise APIError(422, ErrorCode.bad_request, str(e))
    except BlueprintError as e:
        raise APIError(422, ErrorCode.bad_request, f"Couldn't read that blueprint: {e}")


async def decode_release_blueprint(
    release: ModRelease, path: str, fmt: str = "json",
) -> bp_cache.Cached:
    """The voxel payload for one .blueprint inside a release, ready to serve.

    Reading the .tmod and decoding it is expensive and the answer never changes,
    so it happens once per (artifact, path) and is cached from then on - nothing
    below the ``get_or_build`` call runs on a hit."""
    if release.release_format != "tmod":
        raise _not_found("This release has no blueprint models.")

    async def build() -> dict:
        data = await store.get_blob(release.tmod_sha)
        if data is None:
            raise _not_found("Release artifact not found")
        return await asyncio.to_thread(_decode_blueprint_payload, data, path)

    return await bp_cache.get_or_build(
        bp_cache.key_for_tmod(release.tmod_sha, path), build, fmt)


async def update_release(
    release: ModRelease, project: ModProject, actor: SiteUser, *,
    title=None, title_i18n=None, changelog=None, changelog_i18n=None, status=None,
) -> dict:
    _require_owner(project, actor)
    if title is not None:
        release.title = title.strip()
    if changelog is not None:
        release.changelog = changelog
    _set_i18n(release, {
        "title_i18n": (title_i18n, "title", 160),
        "changelog_i18n": (changelog_i18n, "changelog", 20_000),
    })
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
        project.last_release_at = release.published_at or utcnow()
        await project.save()
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


async def get_image(sha: str, width: int | None = None) -> tuple[bytes, str] | None:
    """The stored image, or a WebP downscaled to ``width`` (see store.THUMB_WIDTHS).

    A width the image can't usefully serve - already narrower, animated, or one
    Pillow declines to open - falls back to the original rather than erroring, so
    a caller asking for a thumbnail always gets a usable picture.
    """
    asset = await ModImageAsset.find_one(ModImageAsset.sha == sha)
    if asset is None:
        return None
    if width is not None:
        variant = asset.variants.get(str(width))
        if variant is not None:
            cached = await store.get_blob(variant)
            if cached is not None:
                return cached, "image/webp"
    data = await store.get_blob(sha)
    if data is None:
        return None
    if width is None:
        return data, asset.content_type
    thumb = await asyncio.to_thread(store.render_thumbnail, data, width)
    if thumb is None:
        return data, asset.content_type
    thumb_sha, _ = await store.put_blob(thumb)
    # Set the one key rather than saving the document: two requests can race for
    # the same variant, and they'd otherwise write back each other's stale copy
    # of the whole map.
    await asset.set({f"variants.{width}": thumb_sha})
    return thumb, "image/webp"


async def set_banner(project: ModProject, actor: SiteUser, sha: str) -> ModProject:
    _require_owner(project, actor)
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        raise _not_found("No such uploaded image")
    project.banner_sha = sha
    project.updated_at = utcnow()
    await project.save()
    return project


async def clear_banner(project: ModProject, actor: SiteUser) -> ModProject:
    """Drop the project's banner image (falls back to the placeholder). The blob
    itself is left for GC, matching how previews are removed."""
    _require_owner(project, actor)
    if project.banner_sha is not None:
        project.banner_sha = None
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
    # Mods the user owns OR collaborates on.
    docs = await ModProject.find({"$or": [
        {"owner_id": actor.id}, {"collaborators.user_id": actor.id},
    ]}).sort("-updated_at").to_list()
    # Opportunistically resync the URL handle to the owner's current username, so a
    # Discord rename propagates to their mod links the next time they open My Mods.
    for p in docs:
        if p.owner_id == actor.id and p.owner_handle != actor.username:
            p.owner_handle = actor.username
            await p.save()
    return [{**project_card(p), "is_collaborator": p.owner_id != actor.id} for p in docs]


# --- moderation ------------------------------------------------------------

async def _get_by_id(project_id: str) -> ModProject | None:
    """Fetch a project by its ObjectId string (master actions address by id, since
    slugs are only unique per owner)."""
    oid = to_oid(project_id)
    return await ModProject.get(oid) if oid else None


async def take_down(project_id: str, reason: str) -> ModProject:
    project = await _get_by_id(project_id)
    if project is None:
        raise _not_found()
    project.taken_down = True
    project.takedown_reason = reason.strip() or "Removed by a moderator."
    project.updated_at = utcnow()
    await project.save()
    # DSA Art. 17: resolve the reports and give the owner a statement of reasons.
    from app.trove import moderation
    await moderation.resolve_reports_for("mod", project.id)
    owner = await SiteUser.get(project.owner_id) if project.owner_id else None
    await moderation.notify_takedown(
        owner.notify_email if owner else None, "mod", project.title,
        project.takedown_reason, f"/mods/{project.owner_handle}/{project.slug}",
    )
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


async def _project_sizes(projects: list[ModProject]) -> dict[str, int]:
    """On-disk content footprint (bytes) per project: summed release-artifact sizes
    + banner/preview image sizes. (The git source repo lives on the filesystem and
    isn't summed here - artifacts + images are the indexed, dominant footprint.)
    Two grouped queries total, not one-per-project."""
    if not projects:
        return {}
    ids = [p.id for p in projects]
    rel_rows = await ModRelease.aggregate([
        {"$match": {"project_id": {"$in": ids}}},
        {"$group": {"_id": "$project_id", "s": {"$sum": "$tmod_size"}}},
    ]).to_list()
    rel = {str(r["_id"]): r["s"] for r in rel_rows}
    shas: set[str] = set()
    for p in projects:
        if p.banner_sha:
            shas.add(p.banner_sha)
        shas.update(p.preview_shas or [])
    img: dict[str, int] = {}
    if shas:
        for a in await ModImageAsset.find({"sha": {"$in": list(shas)}}).to_list():
            img[a.sha] = a.byte_size
    sizes: dict[str, int] = {}
    for p in projects:
        total = rel.get(str(p.id), 0)
        if p.banner_sha:
            total += img.get(p.banner_sha, 0)
        for sh in (p.preview_shas or []):
            total += img.get(sh, 0)
        sizes[str(p.id)] = total
    return sizes


# Sort keys offered to the admin projects list. Size is computed (not a stored
# field), so it sorts in Python; the rest are direct fields sorted in Mongo.
_PROJECT_SORTS: dict[str, object] = {
    "updated": "-updated_at",
    "created": "-created_at",
    "popularity": [("popularity_score", -1), ("download_count", -1)],
    "downloads": "-download_count",
    "stars": "-star_count",
    "size": "size",   # sentinel - handled separately
}


async def master_list_projects(
    *, q: str | None = None, owner: str | None = None, visibility: str | None = None,
    sort: str = "updated", limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """ALL projects (drafts + taken-down included) for master oversight - no
    visibility gate. Used by the dev-portal Mods-hub admin tab. Sortable by recency
    (updated/created), popularity, downloads, stars, or on-disk size."""
    query: dict = {}
    if q:
        query.update(_search_clause(q))
    if owner:
        query["owner_username"] = owner
    if visibility:
        query["visibility"] = visibility
    total = await ModProject.find(query).count()

    spec = _PROJECT_SORTS.get(sort, "-updated_at")
    if spec == "size":
        # Size isn't stored, so load the matched set, size it (two grouped queries),
        # sort + paginate in Python. Admin-only + usually narrowed by search/owner.
        all_docs = await ModProject.find(query).to_list()
        sizes = await _project_sizes(all_docs)
        all_docs.sort(key=lambda p: sizes.get(str(p.id), 0), reverse=True)
        docs = all_docs[offset:offset + limit]
    else:
        docs = await ModProject.find(query).sort(spec).skip(offset).limit(limit).to_list()
        sizes = await _project_sizes(docs)

    items = [{**project_card(p), "id": str(p.id), "taken_down": p.taken_down,
              "owner_id": str(p.owner_id) if p.owner_id else None,
              "size_bytes": sizes.get(str(p.id), 0),
              "popularity_score": round(p.popularity_score, 3),
              "downloads_7d": p.downloads_7d} for p in docs]
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
        query.update(_search_clause(q))
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


async def admin_assign_stray(
    project_ids: list[str], user_id: str, master_id: PydanticObjectId,
) -> dict:
    """Master action: hand one or more stray mods directly to a known modder by their
    SiteUser id, with no claim request - for proactively attributing mods as their
    authors sign up. Pending claims on each assigned mod are auto-rejected (the
    handover decides ownership). Returns what was assigned + any per-mod errors."""
    uid = to_oid(user_id)
    user = await SiteUser.get(uid) if uid else None
    if user is None:
        raise _not_found("No such user.")
    assigned: list[dict] = []
    errors: list[dict] = []
    for pid in project_ids:
        project = await _get_by_id(pid)
        if project is None or not project.is_stray:
            errors.append({"id": pid, "error": "Not a stray mod (already claimed or missing)."})
            continue
        await handover_stray(project, user)
        await ModClaimRequest.find(
            ModClaimRequest.project_id == project.id,
            ModClaimRequest.status == "pending",
        ).update(Set({ModClaimRequest.status: "rejected",
                      ModClaimRequest.resolved_by: master_id,
                      ModClaimRequest.resolved_at: utcnow()}))
        assigned.append({"id": str(project.id), "slug": project.slug})
    return {"assigned": assigned, "errors": errors,
            "owner_handle": user.username, "owner_username": user.display_name or user.username}


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
    oid = to_oid(claim_id)
    claim = await ModClaimRequest.get(oid) if oid else None
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
    oid = to_oid(token_id)
    doc = await ModGitToken.get(oid) if oid else None
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
    the same value the lookup-by-hash endpoint matches on, and exactly what
    ``download_url`` serves. A build is never rewritten after it's cut (an attached
    config is baked in once, at build time), so this hash is stable for the life of
    the release - which is what lets an app tell "installed" from "outdated" by
    comparing it against the hash of the file on disk."""
    return {
        "tag": r.tag,
        "branch": r.branch,
        "title": r.title,
        "title_i18n": r.title_i18n,
        "changelog": r.changelog,
        "changelog_i18n": r.changelog_i18n,
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
        "title_i18n": p.title_i18n,
        "summary": p.summary,
        "summary_i18n": p.summary_i18n,
        "description": p.description,
        "description_i18n": p.description_i18n,
        # readme_text = releases-only long-form README (English, always present when
        # there is one); the *_i18n maps are the creator's translations of the field
        # they name, keyed by language code. warnings = <br>-split blocks.
        "readme_text": p.readme_text,
        "readme_i18n": p.readme_i18n,
        "warnings": p.warnings,
        "warnings_i18n": p.warnings_i18n,
        "tags": p.tags,
        "categories": mod_categories.tags_from_flags(mod_categories.flags_from_tags(p.tags)),
        "flags": mod_categories.flags_from_tags(p.tags),
        "author": p.author or p.owner_username,
        # The creator marked this one as still in development: expect changes and
        # rough edges. It's a label only - a beta mod is served like any other.
        "is_beta": p.is_beta,
        # "Stray" = an unclaimed mod uploaded via contributions (not tied to a user
        # yet). The origin/source is intentionally not exposed in the public API.
        "is_stray": p.is_stray,
        # "Uploaded" = shared on the creator's behalf: uploader owns it, `author`
        # credits the named creator. Distinct from an authored mod (author empty).
        "uploaded_on_behalf": p.uploaded_on_behalf,
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
        # When a build last landed - what `sort=recent` orders by. `updated_at`
        # moves on any page edit, so it is NOT the "new version" signal.
        "last_release_at": _iso(p.last_release_at) if p.last_release_at else None,
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
        query["owner_username"] = _author_eq(author)
    if q:
        query.update(_search_clause(q))
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
    Draft / taken-down mods never match.

    A build that was repacked to carry an attached config answers to BOTH hashes -
    the release's own and the modder's pre-injection upload - so a copy installed
    from anywhere still resolves to the right mod and update."""
    seen: set[str] = set()
    uniq = [h for h in (x.strip().lower() for x in hashes if x and x.strip())
            if not (h in seen or seen.add(h))]
    results: dict[str, dict] = {}
    if uniq:
        wanted = set(uniq)
        releases = await ModRelease.find(
            _hash_match(tuple(uniq)), ModRelease.status == "published",
        ).sort("-published_at").to_list()
        proj_cache: dict = {}
        for r in releases:
            keys = [h for h in (r.tmod_sha, *(r.prior_tmod_shas or []))
                    if h in wanted and h not in results]
            if not keys:
                continue  # newest already chosen (sorted desc)
            if r.project_id not in proj_cache:
                proj_cache[r.project_id] = await ModProject.get(r.project_id)
            proj = proj_cache[r.project_id]
            if proj is None or proj.taken_down or proj.visibility == "draft":
                continue
            hit = {"mod": public_mod_dto(proj), "release": public_release_dto(r)}
            for key in keys:
                results[key] = hit
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


async def owner_avatar_url(project: ModProject) -> str | None:
    """The mod owner's profile picture, for the byline on the mod page. ``None`` for
    strays (no owner yet) - the UI keeps its placeholder icon."""
    if project.owner_id is None:
        return None
    user = await SiteUser.get(project.owner_id)
    if user is None:
        return None
    return _profile_avatar_url(user, await ModProfile.find_one(ModProfile.site_user_id == user.id))


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
        "tagline_i18n": p.tagline_i18n if p else {},
        "readme": p.readme if p else "",
        "readme_i18n": p.readme_i18n if p else {},
        "avatar_url": _profile_avatar_url(user, profile),
        "avatar_sha": p.avatar_sha if p else None,
        "banner_url": _public_img_url(p.banner_sha) if (p and p.banner_sha) else None,
        "banner_sha": p.banner_sha if p else None,
        "discord_url": p.discord_url if p else None,
        "website_url": p.website_url if p else None,
        "donation_urls": p.donation_urls if p else [],
        "is_owner": is_owner,
        "taken_down": bool(p and p.taken_down),
        # Reason is shown only to the owner (who sees their flagged profile).
        "takedown_reason": (p.takedown_reason if (p and p.taken_down and is_owner) else None),
        "joined_at": _iso(user.created_at),
        "page_url": f"{settings.app_url.rstrip('/')}/mods/{user.username}",
        "mod_count": len(ordered),
        "mod_order": [m.slug for m in ordered],
        "featured_slug": featured.get("slug") if featured else None,
        "featured": featured,
        "mods": [project_card(m) for m in ordered],
    }


async def take_down_profile(profile_id: str, reason: str) -> ModProfile:
    """Master takedown of a creator profile - hides it from the public and gives the
    owner a statement of reasons (on-page banner + opt-in email). DSA Art. 17."""
    oid = to_oid(profile_id)
    profile = await ModProfile.get(oid) if oid else None
    if profile is None:
        raise _not_found("Profile not found")
    profile.taken_down = True
    profile.takedown_reason = reason.strip() or "Removed by a moderator."
    profile.updated_at = utcnow()
    await profile.save()
    from app.trove import moderation
    await moderation.resolve_reports_for("profile", profile.id)
    owner = await SiteUser.get(profile.site_user_id)
    await moderation.notify_takedown(
        owner.notify_email if owner else None, "profile",
        profile.display_name or profile.handle, profile.takedown_reason,
        f"/mods/{profile.handle}",
    )
    return profile


async def restore_profile(profile_id: str) -> ModProfile:
    oid = to_oid(profile_id)
    profile = await ModProfile.get(oid) if oid else None
    if profile is None:
        raise _not_found("Profile not found")
    profile.taken_down = False
    profile.takedown_reason = None
    profile.updated_at = utcnow()
    await profile.save()
    return profile


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
    # A taken-down profile is hidden from the public; the owner still sees it flagged.
    if profile is not None and profile.taken_down and not is_owner:
        return None
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
    actor: SiteUser, *, display_name=None, tagline=None, tagline_i18n=None,
    readme=None, readme_i18n=None,
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
    _set_i18n(profile, {
        "tagline_i18n": (tagline_i18n, "tagline", 160),
        "readme_i18n": (readme_i18n, "readme", 40_000),
    })
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
