"""Creator tokens + creator↔developer connections (Mods Hub API access).

The Mods Hub is owned by **Dashboard** accounts (Discord login, ``SiteUser``),
but mods should also be manageable from the **API**, whose accounts are dev-portal
``User`` rows. This module is the bridge.

    creator token   one per Dashboard account, minted lazily, shown once.
                    A *connect code*, NOT a per-call credential.
    connection      ``ModCreatorLink`` - created when a developer pastes a creator
                    token into their portal account. Holds the permissions, stays
                    editable by the creator, and is revocable from either side.

So a request is authorized by TWO independent gates: the API token must carry the
``mods:write`` scope, and the calling account must hold a live connection whose
project scope covers the mod being touched. Rotating the creator token revokes
every connection at once.

The service layer (``mods_hub.service``) already takes ``actor: SiteUser`` on every
write and enforces ownership itself, so an authorized API call resolves to the
creator's ``SiteUser`` and then walks exactly the same code path as the website -
there is no second, parallel implementation of mod editing to keep in sync.
"""
from __future__ import annotations

import secrets

from beanie import PydanticObjectId

from app.auth.models import User
from app.core.errors import APIError, ErrorCode
from app.core.security import hash_token
from app.core.utils import iso, to_oid, utcnow
from app.site_auth.models import SiteUser
from app.trove.mods_hub.models import ModCreatorLink, ModProject

# Distinct from the API-token prefix (``kiwi_``) on purpose: pasting the wrong one
# is the obvious mistake here, and the shape alone tells them apart.
CREATOR_TOKEN_PREFIX = "kiwi_creator_"

MAX_LINKS_PER_CREATOR = 25
MAX_LINKS_PER_DEVELOPER = 50


# ── the creator's token ─────────────────────────────────────────────────────

def _mint() -> tuple[str, str, str]:
    """``(plaintext, sha256, display_prefix)`` for a fresh creator token."""
    raw = CREATOR_TOKEN_PREFIX + secrets.token_urlsafe(24)
    return raw, hash_token(raw), raw[: len(CREATOR_TOKEN_PREFIX) + 8]


def token_dto(user: SiteUser) -> dict:
    """The token's public face - never the plaintext, which exists only in the
    response that minted it."""
    return {
        "has_token": bool(user.creator_token_hash),
        "prefix": user.creator_token_prefix,
        "issued_at": iso(user.creator_token_at),
    }


async def ensure_token(user: SiteUser) -> tuple[dict, str | None]:
    """The creator's token, minting one on first use.

    Returns ``(dto, plaintext)`` where plaintext is set ONLY when this call minted
    it - an existing token can't be re-read (we store just the hash), so the panel
    tells the user to rotate if they lost it."""
    if user.creator_token_hash:
        return token_dto(user), None
    raw, hashed, prefix = _mint()
    user.creator_token_hash = hashed
    user.creator_token_prefix = prefix
    user.creator_token_at = utcnow()
    user.updated_at = utcnow()
    await user.save()
    return token_dto(user), raw


async def rotate_token(user: SiteUser) -> tuple[dict, str]:
    """Mint a replacement token and cut every existing connection.

    This is the "revoke everyone" button: the old token stops working AND the
    connections it created are revoked, so a leaked token can't be un-leaked into
    still-live access."""
    raw, hashed, prefix = _mint()
    user.creator_token_hash = hashed
    user.creator_token_prefix = prefix
    user.creator_token_at = utcnow()
    user.updated_at = utcnow()
    await user.save()
    await ModCreatorLink.find(
        ModCreatorLink.site_user_id == user.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).update({"$set": {"revoked": True, "revoked_at": utcnow(),
                       "revoked_by_rotation": True}})
    return token_dto(user), raw


# ── connections ─────────────────────────────────────────────────────────────

async def _project_titles(ids: list[PydanticObjectId]) -> dict[str, dict]:
    """Minimal cards for the mods a narrowed connection names, so both UIs can
    show titles instead of raw ids."""
    if not ids:
        return {}
    docs = await ModProject.find({"_id": {"$in": ids}}).to_list()
    return {str(p.id): {"id": str(p.id), "slug": p.slug, "title": p.title,
                        "handle": p.owner_handle} for p in docs}


async def _link_dto(link: ModCreatorLink, *, projects: dict[str, dict] | None = None) -> dict:
    projects = projects if projects is not None else await _project_titles(link.project_ids)
    return {
        "id": str(link.id),
        "label": link.label,
        "all_projects": link.all_projects,
        "project_ids": [str(i) for i in link.project_ids],
        "projects": [projects[k] for k in (str(i) for i in link.project_ids) if k in projects],
        "created_at": iso(link.created_at),
        "last_used_at": iso(link.last_used_at),
        "request_count": link.request_count,
    }


async def connect(api_user: User, raw_token: str, label: str = "") -> dict:
    """Connect the calling API account to the creator who owns ``raw_token``.

    Re-connecting an existing pair reactivates that connection instead of creating
    a second one, and (deliberately) leaves its project scope alone: a creator who
    narrowed a connection shouldn't have it silently widened by a reconnect."""
    token = (raw_token or "").strip()
    if not token:
        raise APIError(400, ErrorCode.bad_request, "Paste the creator token.")
    creator = await SiteUser.find_one(SiteUser.creator_token_hash == hash_token(token))
    if creator is None or not creator.is_active or creator.is_deleted:
        # One message for "wrong token" and "dead account" - never confirm that a
        # token exists but its owner is gone.
        raise APIError(404, ErrorCode.not_found,
                       "That creator token isn't valid. Ask the creator for a "
                       "current one from their Dashboard.")
    label = (label or "").strip()[:60]

    existing = await ModCreatorLink.find_one(
        ModCreatorLink.site_user_id == creator.id,
        ModCreatorLink.api_user_id == api_user.id,
    )
    if existing is not None:
        if existing.revoked:
            existing.revoked = False
            existing.revoked_at = None
            existing.revoked_by_rotation = False
        if label:
            existing.label = label
        await existing.save()
        return {**await _link_dto(existing), **_creator_face(creator)}

    live_for_creator = await ModCreatorLink.find(
        ModCreatorLink.site_user_id == creator.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).count()
    if live_for_creator >= MAX_LINKS_PER_CREATOR:
        raise APIError(409, ErrorCode.conflict,
                       "This creator has connected the maximum number of API "
                       "accounts. They can free one up from their Dashboard.")
    live_for_dev = await ModCreatorLink.find(
        ModCreatorLink.api_user_id == api_user.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).count()
    if live_for_dev >= MAX_LINKS_PER_DEVELOPER:
        raise APIError(409, ErrorCode.conflict,
                       "You've reached the maximum number of connected creators.")

    link = ModCreatorLink(
        site_user_id=creator.id, api_user_id=api_user.id, label=label,
        all_projects=True,          # the creator narrows it afterwards if they want
    )
    await link.insert()
    return {**await _link_dto(link), **_creator_face(creator)}


def _creator_face(creator: SiteUser) -> dict:
    """How a connected creator is shown to the developer: their public mod handle,
    which is also the value the API's ``creator`` selector takes."""
    return {"creator": {"handle": creator.username,
                        "display_name": creator.display_name or creator.username}}


async def list_for_developer(api_user: User) -> list[dict]:
    """The creators this API account can manage mods for (portal "Creators" tab)."""
    links = await ModCreatorLink.find(
        ModCreatorLink.api_user_id == api_user.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).sort("-created_at").to_list()
    out: list[dict] = []
    for link in links:
        creator = await SiteUser.get(link.site_user_id)
        if creator is None:
            continue
        out.append({**await _link_dto(link), **_creator_face(creator)})
    return out


async def list_for_creator(creator: SiteUser) -> list[dict]:
    """The API accounts connected to this creator (Dashboard "API access" panel).

    The developer's email is never surfaced - their connection ``label`` is what
    identifies it, and the creator can revoke any row regardless."""
    links = await ModCreatorLink.find(
        ModCreatorLink.site_user_id == creator.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).sort("-created_at").to_list()
    owned = await ModProject.find(ModProject.owner_id == creator.id).to_list()
    projects = {str(p.id): {"id": str(p.id), "slug": p.slug, "title": p.title,
                            "handle": p.owner_handle} for p in owned}
    return [await _link_dto(link, projects=projects) for link in links]


async def owned_cards(creator: SiteUser) -> list[dict]:
    """The creator's mods, minimal - the picker the Dashboard uses to narrow a
    connection to specific mods."""
    docs = await ModProject.find(ModProject.owner_id == creator.id).sort("title").to_list()
    return [{"id": str(p.id), "slug": p.slug, "title": p.title} for p in docs]


async def _own_link(creator: SiteUser, link_id: str) -> ModCreatorLink:
    oid = to_oid(link_id)
    link = await ModCreatorLink.get(oid) if oid else None
    if link is None or link.site_user_id != creator.id or link.revoked:
        raise APIError(404, ErrorCode.not_found, "No such connection.")
    return link


async def set_scope(
    creator: SiteUser, link_id: str, *, all_projects: bool, project_ids: list[str],
) -> dict:
    """Narrow a connection to named mods, or widen it back to "all, including new".

    Ids that aren't this creator's mods are dropped rather than rejected: a mod
    deleted between page load and save shouldn't fail the whole save."""
    link = await _own_link(creator, link_id)
    if all_projects:
        link.all_projects = True
        link.project_ids = []
    else:
        wanted = [oid for oid in (to_oid(i) for i in project_ids) if oid is not None]
        owned = await ModProject.find(
            ModProject.owner_id == creator.id, {"_id": {"$in": wanted}},
        ).to_list()
        if not owned:
            raise APIError(400, ErrorCode.bad_request,
                           "Pick at least one mod, or grant access to all of them.")
        link.all_projects = False
        link.project_ids = [p.id for p in owned]
    await link.save()
    return await _link_dto(link)


async def revoke_by_creator(creator: SiteUser, link_id: str) -> None:
    link = await _own_link(creator, link_id)
    link.revoked = True
    link.revoked_at = utcnow()
    await link.save()


async def disconnect_by_developer(api_user: User, link_id: str) -> None:
    """The developer dropping a creator from their own list."""
    oid = to_oid(link_id)
    link = await ModCreatorLink.get(oid) if oid else None
    if link is None or link.api_user_id != api_user.id or link.revoked:
        raise APIError(404, ErrorCode.not_found, "No such connection.")
    link.revoked = True
    link.revoked_at = utcnow()
    await link.save()


# ── authorization (used by the write-route dependency) ──────────────────────

async def live_links(api_user: User) -> list[ModCreatorLink]:
    return await ModCreatorLink.find(
        ModCreatorLink.api_user_id == api_user.id,
        ModCreatorLink.revoked == False,  # noqa: E712
    ).to_list()


def covers(link: ModCreatorLink, project: ModProject) -> bool:
    """Does this connection reach that mod? ``all_projects`` covers mods created
    after the connection was made, which is the point of the flag."""
    if project.owner_id != link.site_user_id:
        return False
    return link.all_projects or project.id in link.project_ids


async def touch(link: ModCreatorLink) -> None:
    """Record activity for the connections list. Best-effort - a bookkeeping write
    must never fail the request it is bookkeeping."""
    try:
        await link.update({"$set": {"last_used_at": utcnow()},
                           "$inc": {"request_count": 1}})
    except Exception:  # noqa: BLE001 - see docstring
        pass
