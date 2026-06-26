"""Modpacks business logic. No FastAPI types in here - the router adapts.

Reads return JSON-ready dicts (datetimes as ISO strings) shared by both the
``/v1/modpacks/hub/*`` API and the website's ``/site/modpacks/*`` proxies. Writes
take a ``SiteUser`` actor and enforce ownership.

A modpack stores only *references* to Mods Hub mods. Each reference (entry) names a
mod, the mod *variant* (Mods Hub branch) to pull from, and - optionally - a pinned
version. Artifacts are resolved + built on the fly at download time so unlocked
entries always track the latest published build. Images reuse the Mods Hub CAS +
``ModImageAsset`` (served at ``/site/mods/image/<sha>``).
"""

from __future__ import annotations

import io
import json
import logging
import re
import zipfile
from datetime import datetime
from urllib.parse import quote

from beanie import PydanticObjectId
from beanie.operators import Inc
from pymongo.errors import DuplicateKeyError

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.site_auth.models import SiteUser
from app.trove import tmod
from app.trove.mods_hub import service as mods_service
from app.trove.mods_hub import store as mods_store
from app.trove.mods_hub.models import Collaborator, ModImageAsset, ModProject, ModRelease
from app.trove.modpacks.models import (
    ModpackEntry,
    ModpackProject,
    ModpackStar,
    ModpackVariant,
    Visibility,
)

logger = logging.getLogger("kiwi.modpacks")

_SORTS = {
    "recent": "-updated_at",
    "downloads": "-download_count",
    "stars": "-star_count",
    "new": "-created_at",
    "title": "title",
}

_DEFAULT_VARIANT = "default"


# --- helpers ---------------------------------------------------------------

def _iso(dt: datetime | None) -> str | None:
    return dt.isoformat() if dt else None


def _slugify(text: str) -> str:
    s = re.sub(r"[^a-z0-9]+", "-", (text or "").lower()).strip("-")
    return s[:60] or "pack"


def _clean_tags(tags: list[str]) -> list[str]:
    out: list[str] = []
    for t in tags or []:
        t = re.sub(r"\s+", " ", (t or "").strip().lower())[:30]
        if t and t not in out:
            out.append(t)
    return out[:12]


_URL_RE = re.compile(r"^https?://\S+$", re.IGNORECASE)


def _clean_url(u: str | None, *, field: str = "link") -> str | None:
    u = (u or "").strip()
    if not u:
        return None
    if len(u) > 300 or not _URL_RE.match(u):
        raise APIError(400, ErrorCode.bad_request,
                       f"Invalid {field} URL - it must start with http:// or https://.")
    return u


def _safe_filename(name: str) -> str:
    cleaned = re.sub(r'[<>:"/\\|?*\x00-\x1f]', "", str(name or "")).strip().strip(".")
    return cleaned[:120] or "modpack"


async def _unique_slug(owner_id: PydanticObjectId, title: str) -> str:
    base = _slugify(title)
    candidate = base
    n = 2
    while await ModpackProject.find_one(
        ModpackProject.owner_id == owner_id, ModpackProject.slug == candidate,
    ) is not None:
        candidate = f"{base}-{n}"
        n += 1
    return candidate


def _collab_ids(pack: ModpackProject) -> set:
    return {c.user_id for c in (pack.collaborators or [])}


def can_edit(pack: ModpackProject, actor: SiteUser | None) -> bool:
    """Edit rights = the primary owner OR a collaborator (co-owner)."""
    if actor is None:
        return False
    return actor.id == pack.owner_id or actor.id in _collab_ids(pack)


def is_primary_owner(pack: ModpackProject, actor: SiteUser | None) -> bool:
    return actor is not None and actor.id == pack.owner_id


def _require_owner(pack: ModpackProject, actor: SiteUser) -> None:
    """Edit-level gate: the primary owner or any collaborator."""
    if not can_edit(pack, actor):
        raise APIError(403, ErrorCode.forbidden, "You don't have edit access to this modpack.")


def _require_primary_owner(pack: ModpackProject, actor: SiteUser) -> None:
    """Stricter gate for owner-only actions (delete, managing collaborators)."""
    if not is_primary_owner(pack, actor):
        raise APIError(403, ErrorCode.forbidden, "Only the modpack's owner can do this.")


def _not_found(what: str = "Modpack not found") -> APIError:
    return APIError(404, ErrorCode.not_found, what)


def _search_clause(q: str) -> dict:
    """Case-insensitive SUBSTRING search across the card-visible fields (replaces the
    whole-word ``$text`` so partial + any-case terms match)."""
    rx = {"$regex": re.escape(q.strip()), "$options": "i"}
    return {"$or": [{"title": rx}, {"summary": rx}, {"tags": rx}, {"owner_username": rx}]}


def _author_eq(author: str) -> dict:
    return {"$regex": f"^{re.escape(author.strip())}$", "$options": "i"}


def _find_variant(pack: ModpackProject, name: str | None) -> ModpackVariant:
    """The named variant, or the pack's default when ``name`` is falsy/unknown."""
    if name:
        for v in pack.variants:
            if v.name == name:
                return v
    for v in pack.variants:
        if v.name == pack.default_variant:
            return v
    if pack.variants:
        return pack.variants[0]
    raise _not_found("This modpack has no variants")


# --- entry resolution ------------------------------------------------------

async def _resolve_entry(entry: ModpackEntry) -> tuple[dict, ModRelease | None]:
    """Resolve one entry against the live Mods Hub: refresh its denormalized
    display fields and pick the build it points at (the locked release, or the
    latest published ``.tmod`` on its branch). Returns ``(view, release)`` where
    ``release`` is None when the mod/build is unavailable. ``view`` is always
    JSON-ready and carries an ``available`` flag + ``reason`` for the UI."""
    # Custom uploaded .tmod (not a hub mod): resolves directly to its stored bytes.
    if entry.custom_sha:
        cview = {
            "custom": True, "custom_sha": entry.custom_sha,
            "custom_filename": entry.custom_filename, "handle": "", "slug": "",
            "title": entry.title or "Uploaded mod", "author": entry.author or "",
            "branch": "", "version_locked": False, "locked_tag": None, "version": None,
            "available": True, "reason": None,
        }
        if not await mods_store.has_blob(entry.custom_sha):
            cview["available"] = False
            cview["reason"] = "missing"
        return cview, None

    view = {
        "custom": False,
        "handle": entry.handle, "slug": entry.slug, "title": entry.title,
        "author": "", "branch": entry.branch, "version_locked": entry.version_locked,
        "locked_tag": entry.locked_tag, "version": None,
        "available": False, "reason": None,
    }
    mod = await ModProject.get(entry.project_id) if entry.project_id else None
    if mod is None:
        view["reason"] = "removed"
        return view, None
    # Keep denormalized fields current (mod may have been renamed / re-handled).
    view["handle"], view["slug"], view["title"] = mod.owner_handle, mod.slug, mod.title
    view["author"] = mod.owner_username   # the mod's author, linkable to /mods/<handle>
    if mod.taken_down or mod.visibility == "draft":
        view["reason"] = "unavailable"
        return view, None

    if entry.version_locked and entry.locked_tag:
        rel = await ModRelease.find_one(
            ModRelease.project_id == mod.id, ModRelease.branch == entry.branch,
            ModRelease.tag == entry.locked_tag, ModRelease.status == "published",
        )
    else:
        rel = await ModRelease.find(
            ModRelease.project_id == mod.id, ModRelease.branch == entry.branch,
            ModRelease.status == "published", ModRelease.release_format == "tmod",
        ).sort("-published_at").first_or_none()

    if rel is None:
        view["reason"] = "no build"
        return view, None
    view["version"] = rel.tag
    if rel.release_format != "tmod":
        view["reason"] = "not a .tmod"   # e.g. a locked tag that points at a .zip
        return view, None
    view["available"] = True
    return view, rel


async def _variant_view(variant: ModpackVariant) -> dict:
    resolved = [await _resolve_entry(e) for e in variant.entries]
    entries = [view for view, _ in resolved]
    return {
        "name": variant.name,
        "label": variant.label or variant.name,
        "entries": entries,
        "mod_count": len(entries),
        "available_count": sum(1 for e in entries if e["available"]),
    }


# --- DTO builders ----------------------------------------------------------

def pack_card(p: ModpackProject) -> dict:
    total = sum(len(v.entries) for v in p.variants)
    default_v = next((v for v in p.variants if v.name == p.default_variant),
                     p.variants[0] if p.variants else None)
    return {
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "tags": p.tags,
        "owner_username": p.owner_username,
        "visibility": p.visibility,
        "banner_sha": p.banner_sha,
        # First preview so a card with no banner can fall back to it (cards only).
        "preview_sha": p.preview_shas[0] if p.preview_shas else None,
        "download_count": p.download_count,
        "star_count": p.star_count,
        "variant_count": len(p.variants),
        # Mod count of the default variant - the headline number on a card.
        "mod_count": len(default_v.entries) if default_v else 0,
        "total_entries": total,
        "updated_at": _iso(p.updated_at),
        "created_at": _iso(p.created_at),
    }


async def pack_detail(pack: ModpackProject, viewer: SiteUser | None) -> dict:
    is_owner = can_edit(pack, viewer)            # owner OR collaborator -> editor UI
    primary = is_primary_owner(pack, viewer)
    # Variants render in the stored order; every entry is resolved to its current
    # version + availability so the page lists "all mods + variants + versions".
    variants = [await _variant_view(v) for v in pack.variants]
    return {
        **pack_card(pack),
        "description": pack.description,
        "warnings": pack.warnings,
        "preview_shas": pack.preview_shas,
        "discord_url": pack.discord_url,
        "website_url": pack.website_url,
        "donation_urls": pack.donation_urls,
        "default_variant": pack.default_variant,
        "variants": variants,
        "taken_down": pack.taken_down,
        "takedown_reason": pack.takedown_reason if is_owner else None,
        "is_owner": is_owner,
        "is_primary_owner": primary,
        "collaborators": [{"id": str(c.user_id), "username": c.username}
                          for c in (pack.collaborators or [])],
        "starred": await has_starred(viewer, pack),
    }


# --- visibility + lookup ---------------------------------------------------

def can_view(pack: ModpackProject, viewer: SiteUser | None) -> bool:
    if can_edit(pack, viewer):                   # owner or collaborator
        return True
    if pack.taken_down:
        return False
    return pack.visibility in ("public", "unlisted")


async def get_pack(handle: str, slug: str) -> ModpackProject | None:
    user = await SiteUser.find_one(SiteUser.username == (handle or "").strip().lower())
    if user is None:
        return None
    return await ModpackProject.find_one(
        ModpackProject.owner_id == user.id, ModpackProject.slug == slug,
    )


async def get_for_view(handle: str, slug: str, viewer: SiteUser | None) -> ModpackProject:
    pack = await get_pack(handle, slug)
    if pack is None or not can_view(pack, viewer):
        raise _not_found()
    return pack


async def list_public(
    *, q: str | None = None, tag: str | None = None, author: str | None = None,
    sort: str = "recent", limit: int = 30, offset: int = 0,
) -> tuple[list[dict], int]:
    query: dict = {"visibility": "public", "taken_down": False}
    if tag:
        query["tags"] = tag.strip().lower()
    if author:
        query["owner_username"] = _author_eq(author)
    if q:
        query.update(_search_clause(q))
    sort_key = _SORTS.get(sort, "-updated_at")
    total = await ModpackProject.find(query).count()
    docs = await ModpackProject.find(query).sort(sort_key).skip(offset).limit(limit).to_list()
    return [pack_card(p) for p in docs], total


async def list_owned(actor: SiteUser) -> list[dict]:
    # Modpacks the user owns OR collaborates on.
    docs = await ModpackProject.find({"$or": [
        {"owner_id": actor.id}, {"collaborators.user_id": actor.id},
    ]}).sort("-updated_at").to_list()
    for p in docs:                       # resync URL handle to current username (owner only)
        if p.owner_id == actor.id and p.owner_handle != actor.username:
            p.owner_handle = actor.username
            await p.save()
    return [{**pack_card(p), "is_collaborator": p.owner_id != actor.id} for p in docs]


# --- create / update -------------------------------------------------------

async def create_modpack(
    actor: SiteUser, *, title: str, summary: str, description: str,
    tags: list[str], visibility: Visibility,
) -> ModpackProject:
    slug = await _unique_slug(actor.id, title)
    pack = ModpackProject(
        slug=slug, title=title.strip(), summary=summary.strip(),
        description=description, tags=_clean_tags(tags), visibility=visibility,
        owner_id=actor.id, owner_username=actor.display_name or actor.username,
        owner_handle=actor.username,
        variants=[ModpackVariant(name=_DEFAULT_VARIANT, label="Default")],
        default_variant=_DEFAULT_VARIANT,
    )
    await pack.insert()
    return pack


async def update_modpack(
    pack: ModpackProject, actor: SiteUser, *, title=None, summary=None,
    description=None, warnings=None, tags=None, visibility=None,
    discord_url=None, website_url=None, donation_urls=None,
    default_variant=None, variant_order=None,
) -> ModpackProject:
    _require_owner(pack, actor)
    if title is not None:
        pack.title = title.strip()
    if summary is not None:
        pack.summary = summary.strip()
    if description is not None:
        pack.description = description
    if warnings is not None:
        pack.warnings = warnings
    if tags is not None:
        pack.tags = _clean_tags(tags)
    if visibility is not None:
        pack.visibility = visibility
    if discord_url is not None:
        pack.discord_url = _clean_url(discord_url, field="Discord")
    if website_url is not None:
        pack.website_url = _clean_url(website_url, field="website")
    if donation_urls is not None:
        cleaned = [u for u in (_clean_url(x, field="support") for x in donation_urls) if u]
        pack.donation_urls = cleaned[:5]
    if default_variant is not None:
        if not any(v.name == default_variant for v in pack.variants):
            raise APIError(400, ErrorCode.bad_request, "No such variant to make default.")
        pack.default_variant = default_variant
    if variant_order is not None:
        # Reorder the variants list per the given names; unlisted fall to the end.
        order = {name: i for i, name in enumerate(variant_order)}
        pack.variants.sort(key=lambda v: order.get(v.name, len(order)))
    pack.owner_handle = actor.username
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def delete_modpack(pack: ModpackProject, actor: SiteUser) -> None:
    _require_primary_owner(pack, actor)          # collaborators can't delete the pack
    await ModpackStar.find(ModpackStar.modpack_id == pack.id).delete()
    await pack.delete()


# --- master moderation -----------------------------------------------------

async def _get_pack_by_id(pack_id: str) -> ModpackProject | None:
    try:
        return await ModpackProject.get(PydanticObjectId(pack_id))
    except Exception:
        return None


async def master_list_modpacks(
    *, q: str | None = None, owner: str | None = None, visibility: str | None = None,
    limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """ALL modpacks (drafts + taken-down included) for master oversight."""
    query: dict = {}
    if q:
        query.update(_search_clause(q))
    if owner:
        query["owner_username"] = owner
    if visibility:
        query["visibility"] = visibility
    total = await ModpackProject.find(query).count()
    docs = await ModpackProject.find(query).sort("-updated_at").skip(offset).limit(limit).to_list()
    items = [{**pack_card(p), "id": str(p.id), "taken_down": p.taken_down,
              "owner_id": str(p.owner_id) if p.owner_id else None} for p in docs]
    return items, total


async def take_down(pack_id: str, reason: str) -> ModpackProject:
    pack = await _get_pack_by_id(pack_id)
    if pack is None:
        raise _not_found()
    pack.taken_down = True
    pack.takedown_reason = (reason or "").strip() or None
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def restore(pack_id: str) -> ModpackProject:
    pack = await _get_pack_by_id(pack_id)
    if pack is None:
        raise _not_found()
    pack.taken_down = False
    pack.takedown_reason = None
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def master_delete(pack_id: str) -> None:
    pack = await _get_pack_by_id(pack_id)
    if pack is None:
        raise _not_found()
    await ModpackStar.find(ModpackStar.modpack_id == pack.id).delete()
    await pack.delete()


# --- collaborators (co-owners) ---------------------------------------------

async def add_collaborator(pack: ModpackProject, actor: SiteUser, username: str) -> dict:
    """Add a co-owner by username (primary owner only)."""
    _require_primary_owner(pack, actor)
    uname = (username or "").strip().lstrip("@").lower()
    if not uname:
        raise APIError(400, ErrorCode.bad_request, "Enter a username to collaborate with.")
    user = await SiteUser.find_one(SiteUser.username == uname)
    if user is None:
        raise APIError(404, ErrorCode.not_found, f"No site user '@{uname}'. They must sign in once first.")
    if user.id == pack.owner_id:
        raise APIError(400, ErrorCode.bad_request, "That's the owner.")
    if user.id in _collab_ids(pack):
        return await pack_detail(pack, actor)
    if len(pack.collaborators) >= 20:
        raise APIError(400, ErrorCode.bad_request, "Too many collaborators (max 20).")
    pack.collaborators.append(Collaborator(user_id=user.id, username=user.username))
    pack.updated_at = utcnow()
    await pack.save()
    return await pack_detail(pack, actor)


async def remove_collaborator(pack: ModpackProject, actor: SiteUser, user_id: str) -> dict:
    _require_primary_owner(pack, actor)
    try:
        uid = PydanticObjectId(user_id)
    except Exception:
        uid = None
    pack.collaborators = [c for c in pack.collaborators if c.user_id != uid]
    pack.updated_at = utcnow()
    await pack.save()
    return await pack_detail(pack, actor)


# --- variants --------------------------------------------------------------

async def add_variant(
    pack: ModpackProject, actor: SiteUser, *, name: str, copy_from: str | None = None,
) -> ModpackProject:
    _require_owner(pack, actor)
    slug = _slugify(name)
    if any(v.name == slug for v in pack.variants):
        raise APIError(409, ErrorCode.conflict, f"A variant named '{slug}' already exists.")
    if len(pack.variants) >= 30:
        raise APIError(400, ErrorCode.bad_request, "Too many variants (max 30).")
    entries: list[ModpackEntry] = []
    if copy_from:
        src = next((v for v in pack.variants if v.name == copy_from), None)
        if src is not None:
            entries = [e.model_copy(deep=True) for e in src.entries]
    pack.variants.append(ModpackVariant(name=slug, label=name.strip(), entries=entries))
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def update_variant(
    pack: ModpackProject, actor: SiteUser, name: str, *, label=None,
) -> ModpackProject:
    _require_owner(pack, actor)
    variant = next((v for v in pack.variants if v.name == name), None)
    if variant is None:
        raise _not_found("Variant not found")
    if label is not None:
        variant.label = label.strip()
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def delete_variant(pack: ModpackProject, actor: SiteUser, name: str) -> ModpackProject:
    _require_owner(pack, actor)
    if not any(v.name == name for v in pack.variants):
        raise _not_found("Variant not found")
    if len(pack.variants) <= 1:
        raise APIError(400, ErrorCode.bad_request, "A modpack needs at least one variant.")
    pack.variants = [v for v in pack.variants if v.name != name]
    if pack.default_variant == name:        # default removed -> first remaining
        pack.default_variant = pack.variants[0].name
    pack.updated_at = utcnow()
    await pack.save()
    return pack


# --- entries (replace a variant's whole list) ------------------------------

def _fingerprint_bytes(data: bytes) -> tuple[str, set[str]] | None:
    """``(title_lowercased, set_of_replaced_file_paths)`` for a ``.tmod``'s bytes.
    The mod's own preview image (the file named by the ``previewPath`` header
    property) is excluded - it's a thumbnail, not a real game-file replacement.
    None if the bytes don't parse as a ``.tmod``."""
    try:
        parsed = tmod.read_tmod(data, metadata_only=True)
    except Exception:
        return None
    props = parsed.get("properties") or {}
    title = (props.get("title") or "").strip().lower()
    preview = (props.get("previewPath") or "").strip().lower()
    files = {(f.get("path") or "").lower() for f in parsed.get("files", []) if f.get("path")}
    files.discard(preview)
    return title, files


async def _fingerprint_sha(sha: str) -> tuple[str, set[str]] | None:
    data = await mods_store.get_blob(sha)
    return _fingerprint_bytes(data) if data is not None else None


def _entry_sha(entry: ModpackEntry, rel: ModRelease | None) -> str | None:
    """The CAS sha of the entry's packed ``.tmod`` (custom upload or resolved release)."""
    return entry.custom_sha or (rel.tmod_sha if rel is not None else None)


def _pair_conflicts(label_a, fp_a, label_b, fp_b) -> list[str]:
    out = []
    if fp_a[0] and fp_a[0] == fp_b[0]:
        out.append(f"“{label_a}” and “{label_b}” share the same mod title.")
    shared = sorted(fp_a[1] & fp_b[1])
    if shared:
        shown = ", ".join(shared[:3]) + (" …" if len(shared) > 3 else "")
        out.append(f"“{label_a}” and “{label_b}” both replace: {shown}")
    return out


async def _detect_conflicts(entries: list[ModpackEntry]) -> list[str]:
    """Find conflicting mods among the (resolvable .tmod) entries: two mods conflict
    if they share the same header ``title`` OR replace any of the same files (preview
    images ignored). Handles both hub-mod references and custom uploads."""
    fps = []   # (label, fingerprint)
    for e in entries:
        view, rel = await _resolve_entry(e)
        if not view["available"]:
            continue                       # unresolved/zip/no-build can't be packed -> can't conflict
        sha = _entry_sha(e, rel)
        if not sha:
            continue
        fp = await _fingerprint_sha(sha)
        if fp is None:
            continue
        fps.append((view["title"] or e.slug or "Uploaded mod", fp))

    conflicts: list[str] = []
    for i in range(len(fps)):
        for j in range(i + 1, len(fps)):
            conflicts += _pair_conflicts(fps[i][0], fps[i][1], fps[j][0], fps[j][1])
    return conflicts


async def _conflicts_against(new_label: str, new_fp: tuple[str, set[str]],
                             existing: list[ModpackEntry]) -> list[str]:
    """Conflicts of ONE new mod (already fingerprinted) against existing entries -
    used to vet an upload before adding it."""
    out: list[str] = []
    for e in existing:
        view, rel = await _resolve_entry(e)
        if not view["available"]:
            continue
        sha = _entry_sha(e, rel)
        if not sha:
            continue
        fp = await _fingerprint_sha(sha)
        if fp is None:
            continue
        out += _pair_conflicts(new_label, new_fp, view["title"] or "a mod", fp)
    return out


async def set_entries(
    pack: ModpackProject, actor: SiteUser, variant_name: str, entries_input: list,
) -> ModpackProject:
    """Replace the variant's ordered mod list. Each input is validated to a real,
    viewable mod (public/unlisted, not taken down); the stable ``project_id`` +
    current title are resolved + denormalized. Duplicate (mod, branch) pairs are
    collapsed (they'd pack to the same file). Submitting is REJECTED if any two mods
    conflict (same header title, or they replace the same files). Branch/version are
    NOT hard-validated - an entry can outlive a since-removed build (shows 'unavailable')."""
    _require_owner(pack, actor)
    variant = next((v for v in pack.variants if v.name == variant_name), None)
    if variant is None:
        raise _not_found("Variant not found")

    resolved: list[ModpackEntry] = []
    seen: set = set()
    for item in entries_input:
        if item.custom_sha:
            # Custom uploaded .tmod round-tripped from a prior upload: keep it as-is
            # (the blob must still exist; dedupe by sha).
            if not await mods_store.has_blob(item.custom_sha):
                raise APIError(400, ErrorCode.bad_request, "An uploaded mod is missing from storage.")
            if item.custom_sha in seen:
                continue
            seen.add(item.custom_sha)
            resolved.append(ModpackEntry(
                project_id=None, custom_sha=item.custom_sha,
                custom_filename=item.custom_filename, title=item.title or "Uploaded mod",
                author=item.author or "",
            ))
            continue
        mod = await get_mod(item.handle, item.slug)
        if mod is None or mod.taken_down or mod.visibility == "draft":
            raise APIError(400, ErrorCode.bad_request,
                           f"Mod '{item.handle}/{item.slug}' isn't available to add.")
        branch = (item.branch or "main").strip() or "main"
        key = (mod.id, branch)
        if key in seen:
            continue
        seen.add(key)
        resolved.append(ModpackEntry(
            project_id=mod.id, handle=mod.owner_handle, slug=mod.slug, title=mod.title,
            branch=branch, version_locked=bool(item.version_locked),
            locked_tag=(item.locked_tag or None) if item.version_locked else None,
        ))

    conflicts = await _detect_conflicts(resolved)
    if conflicts:
        raise APIError(409, ErrorCode.conflict,
                       "These mods conflict and can't be in the same variant: " + " ".join(conflicts))

    variant.entries = resolved
    pack.updated_at = utcnow()
    await pack.save()
    return pack


async def get_mod(handle: str, slug: str) -> ModProject | None:
    """Resolve a Mods Hub mod for inclusion (thin wrapper over the hub service)."""
    return await mods_service.get_project(handle, slug)


_MAX_UPLOAD_BYTES = 80 * 1024 * 1024


async def upload_entry(
    pack: ModpackProject, actor: SiteUser, variant_name: str, filename: str, data: bytes,
) -> dict:
    """Add a custom uploaded ``.tmod`` to a variant. Validates the file, **rejects it
    (409) if it conflicts** with mods already in the variant (same title / shared
    files - nothing is committed on conflict), then: if its content hash matches a
    mod we ALREADY host, add a **reference** to that mod (pinned to the matched
    build) instead of storing a duplicate; otherwise store it as a **custom** entry.
    Returns the updated pack detail (+ `matched_existing`)."""
    _require_owner(pack, actor)
    variant = next((v for v in pack.variants if v.name == variant_name), None)
    if variant is None:
        raise _not_found("Variant not found")
    if not data:
        raise APIError(400, ErrorCode.bad_request, "The uploaded file is empty.")
    if len(data) > _MAX_UPLOAD_BYTES:
        raise APIError(413, ErrorCode.bad_request, "That .tmod is too large to upload.")

    fp = _fingerprint_bytes(data)
    if fp is None:
        raise APIError(400, ErrorCode.bad_request, "That isn't a valid .tmod file.")
    props = tmod.read_tmod(data, metadata_only=True).get("properties") or {}
    title = (props.get("title") or "").strip()
    if not title:
        raise APIError(400, ErrorCode.bad_request, "The .tmod has no title in its header.")
    author = (props.get("author") or "").strip()

    # Conflict check BEFORE committing anything.
    conflicts = await _conflicts_against(title, fp, variant.entries)
    if conflicts:
        raise APIError(409, ErrorCode.conflict,
                       "This mod conflicts with mods already in the variant: " + " ".join(conflicts))

    # Store (CAS is content-addressed + idempotent → no duplicate bytes) and dedup by
    # hash: if we already host this exact file as a mod release, reference that mod.
    sha, _ = await mods_store.put_blob(data)
    rel = await ModRelease.find_one(ModRelease.tmod_sha == sha)
    if rel is not None:
        mod = await ModProject.get(rel.project_id)
        if mod is not None and not mod.taken_down and mod.visibility != "draft":
            if not any(e.project_id == mod.id and e.branch == rel.branch for e in variant.entries):
                variant.entries.append(ModpackEntry(
                    project_id=mod.id, handle=mod.owner_handle, slug=mod.slug, title=mod.title,
                    branch=rel.branch, version_locked=True, locked_tag=rel.tag,
                ))
            pack.updated_at = utcnow()
            await pack.save()
            return {"matched_existing": True, "handle": mod.owner_handle, "slug": mod.slug,
                    **(await pack_detail(pack, actor))}

    # Not hosted (or matched a hidden mod) → keep it as a custom uploaded entry.
    if not any(e.custom_sha == sha for e in variant.entries):
        variant.entries.append(ModpackEntry(
            project_id=None, custom_sha=sha, custom_filename=f"{_safe_filename(title)}.tmod",
            title=title, author=author,
        ))
    pack.updated_at = utcnow()
    await pack.save()
    return {"matched_existing": False, **(await pack_detail(pack, actor))}


# --- images ----------------------------------------------------------------

async def set_banner(pack: ModpackProject, actor: SiteUser, sha: str) -> ModpackProject:
    _require_owner(pack, actor)
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        raise _not_found("No such uploaded image")
    pack.banner_sha = sha
    pack.updated_at = utcnow()
    await pack.save()
    return pack


# --- likes (stars) ---------------------------------------------------------

async def has_starred(viewer: SiteUser | None, pack: ModpackProject) -> bool:
    if viewer is None:
        return False
    return await ModpackStar.find_one(
        ModpackStar.modpack_id == pack.id, ModpackStar.site_user_id == viewer.id,
    ) is not None


async def star_pack(actor: SiteUser, pack: ModpackProject) -> dict:
    """Like a modpack (idempotent). The count is bumped with an atomic ``$inc`` so
    concurrent likes from different users can't lose an update."""
    if await ModpackStar.find_one(
        ModpackStar.modpack_id == pack.id, ModpackStar.site_user_id == actor.id,
    ) is None:
        try:
            await ModpackStar(modpack_id=pack.id, site_user_id=actor.id).insert()
        except DuplicateKeyError:
            pass   # raced another request - already liked, don't double-count
        else:
            await ModpackProject.find_one(ModpackProject.id == pack.id).update(
                Inc({ModpackProject.star_count: 1}))
            pack.star_count += 1
    return {"starred": True, "star_count": pack.star_count}


async def unstar_pack(actor: SiteUser, pack: ModpackProject) -> dict:
    star = await ModpackStar.find_one(
        ModpackStar.modpack_id == pack.id, ModpackStar.site_user_id == actor.id)
    if star is not None:
        await star.delete()
        await ModpackProject.find_one(ModpackProject.id == pack.id).update(
            Inc({ModpackProject.star_count: -1}))
        pack.star_count = max(0, pack.star_count - 1)
    return {"starred": False, "star_count": pack.star_count}


# --- "which modpacks include this mod" (backlink for the mod page) ---------

async def packs_including_mod(mod: ModProject) -> list[ModpackProject]:
    """Public, non-taken-down modpacks that reference ``mod`` in any variant."""
    return await ModpackProject.find({
        "visibility": "public", "taken_down": False,
        "variants.entries.project_id": mod.id,
    }).sort("-download_count").limit(60).to_list()


# --- download (build artifact on the fly) ----------------------------------

async def _collect_builds(variant: ModpackVariant) -> tuple[list[tuple[str, bytes, dict]], list[dict]]:
    """Resolve a variant to ``(downloadable, manifest)`` where ``downloadable`` is a
    list of ``(filename, tmod_bytes, view)`` for the entries that resolved to a real
    ``.tmod`` build, and ``manifest`` is the full per-mod record (incl. unavailable
    ones, flagged) for the artifact manifest."""
    downloadable: list[tuple[str, bytes, dict]] = []
    manifest: list[dict] = []
    for entry in variant.entries:
        view, rel = await _resolve_entry(entry)
        manifest.append({
            "handle": view["handle"], "slug": view["slug"], "title": view["title"],
            "branch": view["branch"], "version": view["version"],
            "locked": view["version_locked"], "available": view["available"],
        })
        if not view["available"]:
            continue
        sha = _entry_sha(entry, rel)
        if not sha:
            continue
        data = await mods_store.get_blob(sha)
        if data is None:
            manifest[-1]["available"] = False
            continue
        # CRITICAL: a packed mod's filename MUST be the .tmod's internal `title`
        # property (``<title>.tmod``) - Trove matches a mod in-game by that name and
        # REJECTS a mismatched filename. For a hub ref that's `release_download_filename`;
        # for a custom upload it's the stored `custom_filename` (also `<title>.tmod`).
        name = (entry.custom_filename or f"{_safe_filename(view['title'])}.tmod") if entry.custom_sha \
            else mods_service.release_download_filename(rel)
        manifest[-1]["filename"] = name
        downloadable.append((name, data, view))
    return downloadable, manifest


async def build_artifact(
    pack: ModpackProject, variant_name: str | None, fmt: str,
) -> tuple[bytes, str, str]:
    """Build the modpack artifact for a variant. ``fmt`` is ``tpack`` (API; a
    ``.tmod``-style container packing each mod's ``.tmod``) or ``zip`` (website; the
    ``.tmod`` files + a ``modpack.json`` manifest). Returns ``(bytes, filename,
    media_type)``. Raises 400 if nothing in the variant resolves to a build."""
    variant = _find_variant(pack, variant_name)
    downloadable, manifest = await _collect_builds(variant)
    if not downloadable:
        raise APIError(400, ErrorCode.bad_request,
                       "This modpack has no downloadable mods right now "
                       "(every entry is missing a published build).")

    safe = _safe_filename(pack.title)
    suffix = "" if variant.name == _DEFAULT_VARIANT else f"-{variant.name}"
    manifest_doc = {
        "modpack": {
            "title": pack.title, "handle": pack.owner_handle, "slug": pack.slug,
            "author": pack.owner_username, "variant": variant.name,
            "warnings": pack.warnings or "",
        },
        "mods": manifest,
    }

    if fmt == "zip":
        buf = io.BytesIO()
        with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as zf:
            zf.writestr("modpack.json", json.dumps(manifest_doc, indent=2))
            # Each .tmod keeps its exact ``<title>.tmod`` name (Trove validates it);
            # the user drops these straight into their mods folder. `name` already
            # IS that filename - do not rename.
            for name, data, _ in downloadable:
                zf.writestr(f"mods/{name}", data)
        return buf.getvalue(), f"{safe}{suffix}.zip", "application/zip"

    # default: .tpack container (same format as a .tmod)
    properties = {
        "title": pack.title,
        "author": pack.owner_username,
        "packVersion": "1",
        "variant": variant.name,
        "notes": pack.warnings or "",
        "manifest": json.dumps(manifest_doc),
    }
    inner = [(name, data) for name, data, _ in downloadable]
    blob = tmod.build_tpack(1, properties, inner)
    return blob, f"{safe}{suffix}.tpack", "application/octet-stream"


async def record_download(pack: ModpackProject) -> None:
    pack.download_count += 1
    await pack.save()


# --- documented app-facing catalog API (/v1/modpacks/*) --------------------
# Same shape as the Mods Hub's public catalog: absolute image / page / download
# URLs so an external app (Better Trove Tools) can consume it directly. Reads are
# tokenless `mods:read`; only public, non-taken-down packs are exposed.

def _public_img_url(sha: str | None) -> str | None:
    # Modpack images live in the shared Mods Hub CAS, served by the hub image route.
    if not sha:
        return None
    return f"{settings.api_url.rstrip('/')}/v1/mods/hub/image/{sha}"


def _pack_page_url(p: ModpackProject) -> str:
    return f"{settings.app_url.rstrip('/')}/modpacks/{p.owner_handle}/{p.slug}"


def public_pack_card(p: ModpackProject) -> dict:
    default_v = next((v for v in p.variants if v.name == p.default_variant),
                     p.variants[0] if p.variants else None)
    return {
        "slug": p.slug,
        "handle": p.owner_handle,
        "title": p.title,
        "summary": p.summary,
        "tags": p.tags,
        "author": p.owner_username,
        "banner_url": _public_img_url(p.banner_sha),
        "preview_urls": [u for u in (_public_img_url(s) for s in p.preview_shas) if u],
        "download_count": p.download_count,
        "star_count": p.star_count,
        "variant_count": len(p.variants),
        "default_variant": p.default_variant,
        # Mod count of the default variant - the headline number.
        "mod_count": len(default_v.entries) if default_v else 0,
        "page_url": _pack_page_url(p),
        "created_at": _iso(p.created_at),
        "updated_at": _iso(p.updated_at),
    }


async def _public_variant(p: ModpackProject, variant: ModpackVariant) -> dict:
    """One variant, app-facing: the mods it bundles (with the version each resolves
    to + an absolute page link) and absolute download URLs for both formats."""
    mods = []
    for entry in variant.entries:
        view, _ = await _resolve_entry(entry)
        mods.append({
            "handle": view["handle"], "slug": view["slug"], "title": view["title"],
            "author": view["author"],
            "variant": view["branch"], "version": view["version"],
            "version_locked": view["version_locked"], "available": view["available"],
            "page_url": f"{settings.app_url.rstrip('/')}/mods/"
                        f"{view['handle']}/{view['slug']}",
            "author_url": f"{settings.app_url.rstrip('/')}/mods/{view['handle']}",
        })
    base = (f"{settings.api_url.rstrip('/')}/v1/modpacks/{p.owner_handle}/{p.slug}"
            f"/download?variant={quote(variant.name)}")
    return {
        "name": variant.name,
        "label": variant.label or variant.name,
        "mod_count": len(mods),
        "available_count": sum(1 for m in mods if m["available"]),
        "download_url": f"{base}&format=tpack",   # API default: a .tpack
        "zip_url": f"{base}&format=zip",
        "mods": mods,
    }


async def public_pack_dto(p: ModpackProject) -> dict:
    """Full app-facing metadata for one modpack: every variant + the mods (and
    resolved versions) each bundles, with absolute download URLs."""
    variants = [await _public_variant(p, v) for v in p.variants]
    return {
        **public_pack_card(p),
        "description": p.description,
        "warnings": p.warnings,
        "discord_url": p.discord_url,
        "website_url": p.website_url,
        "donation_urls": p.donation_urls,
        "variants": variants,
    }


async def public_list(
    *, q: str | None = None, tag: str | None = None, author: str | None = None,
    sort: str = "recent", limit: int = 30, offset: int = 0,
) -> tuple[list[dict], int]:
    """Browse public modpacks (cards, no mod lists) with app-facing DTOs."""
    query: dict = {"visibility": "public", "taken_down": False}
    if tag:
        query["tags"] = tag.strip().lower()
    if author:
        query["owner_username"] = _author_eq(author)
    if q:
        query.update(_search_clause(q))
    sort_key = _SORTS.get(sort, "-updated_at")
    total = await ModpackProject.find(query).count()
    docs = await ModpackProject.find(query).sort(sort_key).skip(offset).limit(limit).to_list()
    return [public_pack_card(p) for p in docs], total


async def public_detail(handle: str, slug: str) -> dict:
    """Full app-facing metadata for one public/unlisted modpack (404 otherwise)."""
    pack = await get_for_view(handle, slug, None)   # anon view: public/unlisted, not taken down
    return await public_pack_dto(pack)


async def public_packs_for_mod(handle: str, slug: str) -> list[dict]:
    """App-facing cards (absolute URLs) of public modpacks that include the given
    mod - the backlink shown on a mod's page. Empty list if the mod doesn't exist."""
    mod = await mods_service.get_project(handle, slug)
    if mod is None:
        return []
    return [public_pack_card(p) for p in await packs_including_mod(mod)]


async def site_packs_for_mod(handle: str, slug: str) -> list[dict]:
    """Same as ``public_packs_for_mod`` but with site-relative cards (``banner_sha``)
    for the same-origin mod page."""
    mod = await mods_service.get_project(handle, slug)
    if mod is None:
        return []
    return [pack_card(p) for p in await packs_including_mod(mod)]
