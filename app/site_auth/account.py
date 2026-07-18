"""Self-service GDPR data export + account deletion for public (SiteUser) accounts.

Mirrors the dev-portal ``app/auth/account.py`` but for the Discord-only site
accounts, which own the bulk of user content (mods, modpacks, profiles).

Deletion is ANONYMIZE-IN-PLACE, not a hard delete: the account row is kept but
every identifying field is irreversibly stripped and it's flagged ``is_deleted``
(so it can never be logged into or re-linked). Because the row survives, every
``owner_id``/``reporter_id``/``winner_user_id`` that pointed at it automatically
becomes non-personal - no risky owner-reassignment, no per-owner slug collisions,
and modpacks (whose ``owner_id`` is non-optional) keep a valid owner. The user's
mods and modpacks stay live under the anonymized handle; purely-personal records
(sessions, tokens, stars, profile, DM subs, webhooks, image designs, giveaway
entries, claim/username requests) are deleted; denormalized handles on retained
records (giveaway winner, owned content) are scrubbed. (No email is stored at all
- sign-in is Discord-only; content reports store no reporter identity.)

Shared CAS blobs are never touched (they're content-addressed + deduped across
users); git repos keep their commit history (the author ident is the handle, not
the real email) under the now-anonymized project.
"""
import secrets

from beanie.operators import Inc, Set
from fastapi import APIRouter, Depends, status
from fastapi.encoders import jsonable_encoder
from fastapi.responses import JSONResponse
from pydantic import BaseModel

from app.core.email_outbox import OutboxEmail
from app.core.errors import APIError, ErrorCode
from app.core.utils import utcnow
from app.dm_subs.models import DmSubscription
from app.giveaways.models import Giveaway, GiveawayEntry
from app.images.models import ImageDesign
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import (
    SiteSession,
    SiteUser,
    UsernameChangeRequest,
)
from app.site_auth.oauth import clear_discord_token
from app.trove.modpacks.models import ModpackProject, ModpackStar
from app.trove.mods_hub.models import (
    ModClaimRequest,
    ModGitToken,
    ModProfile,
    ModProject,
    ModStar,
)
from app.webhooks.models import SiteWebhook

router = APIRouter(prefix="/v1/site-auth", tags=["site-auth"])


class _DeleteAccountBody(BaseModel):
    confirm_username: str


async def _unique_anon_username() -> str:
    """A reserved, non-identifying ``deleted_<hex>`` handle, unique among accounts."""
    for _ in range(12):
        candidate = "deleted_" + secrets.token_hex(8)  # 8 + 16 = 24 chars (the max)
        if await SiteUser.find_one(SiteUser.username == candidate) is None:
            return candidate
    return "deleted_" + secrets.token_hex(8)


@router.get("/me/export", include_in_schema=False)
async def export_my_data(user: SiteUser = Depends(get_current_site_user)) -> JSONResponse:
    """Download all of the account's data as JSON (GDPR portability, Art. 20)."""
    uid = user.id
    owned_mods = await ModProject.find(ModProject.owner_id == uid).to_list()
    collab_mods = await ModProject.find({"collaborators.user_id": uid}).to_list()
    owned_packs = await ModpackProject.find(ModpackProject.owner_id == uid).to_list()
    profile = await ModProfile.find_one(ModProfile.site_user_id == uid)
    stars = await ModStar.find(ModStar.site_user_id == uid).to_list()
    pack_stars = await ModpackStar.find(ModpackStar.site_user_id == uid).to_list()
    entries = await GiveawayEntry.find(GiveawayEntry.site_user_id == uid).to_list()
    dm_subs = await DmSubscription.find(DmSubscription.owner_id == uid).to_list()
    webhooks = await SiteWebhook.find(SiteWebhook.owner_id == uid).to_list()
    images = await ImageDesign.find(ImageDesign.owner_id == uid).to_list()
    git_tokens = await ModGitToken.find(ModGitToken.site_user_id == uid).to_list()
    sessions = await SiteSession.find(SiteSession.site_user_id == uid).to_list()
    name_reqs = await UsernameChangeRequest.find(
        UsernameChangeRequest.site_user_id == uid
    ).to_list()

    data = {
        "exported_at": utcnow(),
        "account": {
            "id": str(uid), "username": user.username, "discord_handle": user.discord_handle,
            "display_name": user.display_name, "notify_email": user.notify_email,
            "discord_id": user.discord_id, "is_verified": user.is_verified,
            "claimed_trove_name": user.claimed_trove_display, "claim_verified": user.claim_verified,
            "created_at": user.created_at, "last_login_at": user.last_login_at,
        },
        "mods_owned": [{"slug": m.slug, "title": m.title, "visibility": m.visibility,
                        "created_at": m.created_at} for m in owned_mods],
        "mods_collaborating": [{"slug": m.slug, "title": m.title,
                                "owner_handle": m.owner_handle} for m in collab_mods],
        "modpacks_owned": [{"slug": p.slug, "title": p.title, "visibility": p.visibility,
                            "created_at": p.created_at} for p in owned_packs],
        "profile": None if profile is None else {
            "handle": profile.handle, "display_name": profile.display_name,
            "tagline": profile.tagline, "readme": profile.readme,
        },
        "starred_mods": [{"project_id": str(s.project_id), "at": s.created_at} for s in stars],
        "starred_modpacks": [{"modpack_id": str(s.modpack_id), "at": s.created_at}
                             for s in pack_stars],
        "giveaway_entries": [{"giveaway_id": str(e.giveaway_id), "at": e.entered_at}
                             for e in entries],
        "dm_subscriptions": [{"id": str(d.id), "label": getattr(d, "label", None)}
                             for d in dm_subs],
        "webhooks": [{"id": str(w.id), "label": getattr(w, "label", None)} for w in webhooks],
        "image_designs": [{"id": str(i.id), "created_at": i.created_at} for i in images],
        "git_tokens": [{"name": t.name, "prefix": t.prefix, "revoked": t.revoked,
                        "created_at": t.created_at} for t in git_tokens],
        "sessions": [{"device": s.device, "created_at": s.created_at,
                      "last_used_at": s.last_used_at, "expires_at": s.expires_at}
                     for s in sessions],
        "username_change_requests": [{"requested_username": r.requested_username,
                                      "status": r.status, "created_at": r.created_at}
                                     for r in name_reqs],
    }
    return JSONResponse(
        content=jsonable_encoder(data),
        headers={"Content-Disposition": 'attachment; filename="better-trove-tools-export.json"'},
    )


async def anonymize_account(user: SiteUser) -> None:
    """Irreversibly strip the account's personal data (see module docstring)."""
    uid = user.id

    # 1. Purge any queued mail to their opt-in notification address (if they set one).
    if user.notify_email:
        await OutboxEmail.find(OutboxEmail.to == user.notify_email).delete()

    # 2. Delete purely-personal records outright.
    await SiteSession.find(SiteSession.site_user_id == uid).delete()
    await ModGitToken.find(ModGitToken.site_user_id == uid).delete()
    await ModProfile.find(ModProfile.site_user_id == uid).delete()
    await ModClaimRequest.find(ModClaimRequest.claimant_id == uid).delete()
    await UsernameChangeRequest.find(UsernameChangeRequest.site_user_id == uid).delete()
    await DmSubscription.find(DmSubscription.owner_id == uid).delete()
    await SiteWebhook.find(SiteWebhook.owner_id == uid).delete()
    await ImageDesign.find(ImageDesign.owner_id == uid).delete()

    # 3. Stars + giveaway entries: delete, keeping denormalized counts in sync.
    for s in await ModStar.find(ModStar.site_user_id == uid).to_list():
        await ModProject.find(ModProject.id == s.project_id).update(Inc({ModProject.star_count: -1}))
    await ModStar.find(ModStar.site_user_id == uid).delete()
    for s in await ModpackStar.find(ModpackStar.site_user_id == uid).to_list():
        await ModpackProject.find(ModpackProject.id == s.modpack_id).update(
            Inc({ModpackProject.star_count: -1})
        )
    await ModpackStar.find(ModpackStar.site_user_id == uid).delete()
    for e in await GiveawayEntry.find(GiveawayEntry.site_user_id == uid).to_list():
        await Giveaway.find(Giveaway.id == e.giveaway_id).update(Inc({Giveaway.entry_count: -1}))
    await GiveawayEntry.find(GiveawayEntry.site_user_id == uid).delete()

    anon = await _unique_anon_username()

    # 4. Scrub the denormalized handle on records we keep for integrity. (The
    #    id references now point at the anonymized tombstone, which holds no PII.
    #    Content reports store no reporter, so there's nothing to scrub there.)
    await Giveaway.find(Giveaway.winner_user_id == uid).update(
        Set({Giveaway.winner_username: anon})
    )

    # 5. Anonymize the denormalized owner handle on their live content. (ModRelease
    # and ModImageAsset keep owner_id pointing at the now-anonymized tombstone, which
    # holds no PII, so they need no change.)
    await ModProject.find(ModProject.owner_id == uid).update(
        Set({ModProject.owner_username: anon, ModProject.owner_handle: anon})
    )
    await ModpackProject.find(ModpackProject.owner_id == uid).update(
        Set({ModpackProject.owner_username: anon, ModpackProject.owner_handle: anon})
    )

    # 6. Remove the user from OTHER people's projects/modpacks as a collaborator.
    for proj in await ModProject.find({"collaborators.user_id": uid}).to_list():
        if any(c.user_id == uid for c in proj.collaborators):
            proj.collaborators = [c for c in proj.collaborators if c.user_id != uid]
            await proj.save()
    for pack in await ModpackProject.find({"collaborators.user_id": uid}).to_list():
        if any(c.user_id == uid for c in pack.collaborators):
            pack.collaborators = [c for c in pack.collaborators if c.user_id != uid]
            await pack.save()

    # 7. Anonymize the account row itself and retire it (irreversible).
    user.username = anon
    user.discord_handle = ""
    user.notify_email = None
    user.display_name = None
    user.discord_id = None
    user.discord_avatar = None
    user.is_active = False
    user.is_verified = False
    user.is_deleted = True
    user.clear_claim()
    user.token_version += 1                      # invalidate every outstanding access token
    user.updated_at = utcnow()
    await user.save()

    await clear_discord_token(uid)               # drop the cached Discord token, if any


@router.post("/me/delete", status_code=status.HTTP_204_NO_CONTENT, include_in_schema=False)
async def delete_my_account(
    payload: _DeleteAccountBody, user: SiteUser = Depends(get_current_site_user)
) -> None:
    """Anonymize + retire the account and its personal data (GDPR erasure, Art. 17).

    Requires typing the account's own username to confirm. Owned mods/modpacks stay
    public under an anonymized handle; everything identifying is removed."""
    if payload.confirm_username.strip().lower() != user.username.lower():
        raise APIError(
            status_code=400,
            code=ErrorCode.bad_request,
            message="The confirmation username doesn't match your account.",
        )
    await anonymize_account(user)
