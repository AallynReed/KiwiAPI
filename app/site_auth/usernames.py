"""Trove-username (the frozen website ``SiteUser.username``) change flow.

A user signs up with Discord; their ``username`` is seeded from the Discord handle
(see ``oauth._unique_username``) and then FROZEN - it only changes through this
admin-approved request flow, so renaming on Discord never shifts a user's mod
handles/URLs. The username is the handle for mods/modpacks/profiles, so a change is
reviewed by a master (approve = rename + re-home their mod/modpack handles; reject =
keep it, with a reason shown to the user).

Shared by the site router (user requests) + the admin router (review).
"""

from __future__ import annotations

import re

from beanie import PydanticObjectId

from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.site_auth.models import SiteUser, UsernameChangeRequest

# Allowed character set + length. Periods follow Discord's rules (see
# ``normalize_username``): valid anywhere EXCEPT they can't lead/trail the name or
# repeat (``..``), which the regex's character class alone can't express.
_USERNAME_RE = re.compile(r"^[a-z0-9_.]{3,24}$")

# Handles that would clash with site routes / reserved namespaces (e.g. the stray
# mods namespace `/mods/stray/...`) or look like system accounts.
RESERVED_USERNAMES = frozenset({
    "stray", "why", "admin", "administrator", "api", "me", "mods", "modpacks",
    "trovesaurus", "kiwi", "support", "login", "logout", "dashboard", "system",
    "moderator", "staff", "root", "null", "undefined",
})


def normalize_username(raw: str) -> str:
    """Validate + canonicalise a requested username. Raises ``APIError`` on bad
    format or a reserved word."""
    name = (raw or "").strip().lower()
    # Periods follow Discord's rules: allowed (including at the end), but not at the
    # start and never two in a row.
    if not _USERNAME_RE.match(name) or name.startswith(".") or ".." in name:
        raise APIError(400, ErrorCode.bad_request,
                       "Username must be 3-24 characters using lowercase letters, numbers, "
                       'underscores or periods - a period can\'t start it or repeat ("..").')
    if name in RESERVED_USERNAMES:
        raise APIError(409, ErrorCode.bad_request, "That username is reserved.")
    return name


def username_request_dto(req: UsernameChangeRequest) -> dict:
    return {
        "id": str(req.id),
        "site_user_id": str(req.site_user_id),
        "current_username": req.current_username,
        "requested_username": req.requested_username,
        "status": req.status,
        "reason": req.reason,
        "created_at": req.created_at.isoformat() if req.created_at else None,
        "resolved_at": req.resolved_at.isoformat() if req.resolved_at else None,
    }


async def request_change(user: SiteUser, raw: str) -> UsernameChangeRequest:
    """Create (or replace the user's existing pending) username-change request."""
    name = normalize_username(raw)
    if name == user.username:
        raise APIError(400, ErrorCode.bad_request, "That's already your username.")
    taken = await SiteUser.find_one(SiteUser.username == name)
    if taken is not None and taken.id != user.id:
        raise APIError(409, ErrorCode.bad_request, "That username is already taken.")
    other = await UsernameChangeRequest.find_one(
        UsernameChangeRequest.requested_username == name,
        UsernameChangeRequest.status == "pending",
    )
    if other is not None and other.site_user_id != user.id:
        raise APIError(409, ErrorCode.bad_request, "Someone else has a pending request for that username.")
    existing = await UsernameChangeRequest.find_one(
        UsernameChangeRequest.site_user_id == user.id,
        UsernameChangeRequest.status == "pending",
    )
    if existing is not None:
        existing.requested_username = name
        existing.current_username = user.username
        existing.created_at = utcnow()
        await existing.save()
        return existing
    req = UsernameChangeRequest(
        site_user_id=user.id, current_username=user.username, requested_username=name)
    await req.insert()
    return req


async def latest_request(user: SiteUser) -> UsernameChangeRequest | None:
    """The user's most recent request (pending shows 'awaiting'; rejected shows the
    reason so the dashboard can surface it)."""
    return await UsernameChangeRequest.find(
        UsernameChangeRequest.site_user_id == user.id).sort("-created_at").first_or_none()


async def cancel_pending(user: SiteUser) -> None:
    req = await UsernameChangeRequest.find_one(
        UsernameChangeRequest.site_user_id == user.id,
        UsernameChangeRequest.status == "pending")
    if req is not None:
        await req.delete()


# --- admin review ----------------------------------------------------------

async def list_requests(status: str | None = "pending", limit: int = 100) -> list[dict]:
    query: dict = {}
    if status:
        query["status"] = status
    docs = await UsernameChangeRequest.find(query).sort("-created_at").limit(limit).to_list()
    return [username_request_dto(r) for r in docs]


async def _get_request(request_id: str) -> UsernameChangeRequest:
    try:
        req = await UsernameChangeRequest.get(PydanticObjectId(request_id))
    except Exception:
        req = None
    if req is None:
        raise APIError(404, ErrorCode.not_found, "Username request not found")
    return req


async def _rehome_handles(user: SiteUser, name: str) -> None:
    """Re-home the user's mod + modpack handles to ``name`` (the URL-affecting
    field) so their addresses follow the renamed account."""
    from beanie.operators import Set

    from app.trove.modpacks.models import ModpackProject
    from app.trove.mods_hub.models import ModProject

    await ModProject.find(ModProject.owner_id == user.id).update(
        Set({ModProject.owner_handle: name}))
    await ModpackProject.find(ModpackProject.owner_id == user.id).update(
        Set({ModpackProject.owner_handle: name}))


async def approve_request(request_id: str, master_id: PydanticObjectId) -> dict:
    """Approve: rename the account's ``username`` and re-home their mod + modpack
    handles to it (so their URLs follow the approved name)."""
    req = await _get_request(request_id)
    if req.status != "pending":
        raise APIError(400, ErrorCode.bad_request, "This request is already resolved.")
    user = await SiteUser.find_one(SiteUser.id == req.site_user_id)
    if user is None:
        raise APIError(404, ErrorCode.not_found, "The requesting user no longer exists.")
    name = req.requested_username
    taken = await SiteUser.find_one(SiteUser.username == name)
    if taken is not None and taken.id != user.id:
        raise APIError(409, ErrorCode.conflict, "That username was taken in the meantime.")

    user.username = name
    user.updated_at = utcnow()
    await user.save()
    await _rehome_handles(user, name)

    req.status = "approved"
    req.resolved_by = master_id
    req.resolved_at = utcnow()
    await req.save()
    return {**username_request_dto(req), "new_username": name}


async def admin_set_username(user_id: str, raw: str, master_id: PydanticObjectId) -> dict:
    """Master override: set a site user's frozen Trove username directly (no request),
    re-homing their mod/modpack handles and resolving any pending request they had."""
    try:
        user = await SiteUser.get(PydanticObjectId(user_id))
    except Exception:
        user = None
    if user is None:
        raise APIError(404, ErrorCode.not_found, "No such user.")
    name = normalize_username(raw)
    if name == user.username:
        return {"username": name, "changed": False}
    taken = await SiteUser.find_one(SiteUser.username == name)
    if taken is not None and taken.id != user.id:
        raise APIError(409, ErrorCode.conflict, "That username is already taken.")

    user.username = name
    user.updated_at = utcnow()
    await user.save()
    await _rehome_handles(user, name)

    # Any pending self-service request is now moot - resolve it as approved.
    pending = await UsernameChangeRequest.find_one(
        UsernameChangeRequest.site_user_id == user.id,
        UsernameChangeRequest.status == "pending")
    if pending is not None:
        pending.status = "approved"
        pending.resolved_by = master_id
        pending.resolved_at = utcnow()
        await pending.save()
    return {"username": name, "changed": True}


async def reject_request(request_id: str, master_id: PydanticObjectId, reason: str) -> dict:
    req = await _get_request(request_id)
    if req.status != "pending":
        raise APIError(400, ErrorCode.bad_request, "This request is already resolved.")
    req.status = "rejected"
    req.reason = (reason or "").strip()[:2000]
    req.resolved_by = master_id
    req.resolved_at = utcnow()
    await req.save()
    return username_request_dto(req)
