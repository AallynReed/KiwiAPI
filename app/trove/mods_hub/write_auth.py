"""Who may call a Mods Hub write endpoint.

One dependency serves both callers, so there is a single implementation of every
mod-editing route:

  * **Website** - a Dashboard session, presented as the ``HttpOnly`` cookie the
    browser sends by itself, or as a ``kind=site`` JWT in the Authorization header
    (the desktop app). The caller is the creator, and the route's own ownership
    checks apply.
  * **API** - an API token carrying ``mods:write``, belonging to a dev-portal
    account that holds a live ``ModCreatorLink`` to the creator being acted for
    (see ``creators.py``). The dependency resolves that connection and returns the
    creator's ``SiteUser``, so the route body runs exactly as it does for the
    website.

The two credential shapes are told apart the same way ``require_master_ingest``
does it - a JWT has two ``.`` separators, an API token has none - so only one
validator ever runs. Anything that isn't an API token (a JWT, or no header at all)
goes to ``get_current_site_user``, which is what reads the session cookie.

**API callers are default-denied.** ``_API_ROUTES`` is an explicit allowlist of
(route template → methods); anything absent - including any route added later - is
website-only until someone deliberately lists it. That's what keeps deleting mods
and releases, minting git tokens, editing the creator's profile, and reassigning
collaborators on the website, under the creator's own login.
"""
from __future__ import annotations

from dataclasses import dataclass

from fastapi import Depends, Request, Response
from fastapi.security import HTTPAuthorizationCredentials, HTTPBearer

# The API-token validator + limiter live in core.dependencies; reuse them rather
# than re-implementing token checks (they also do the IP allowlist, rate limits
# and usage accounting every other API route gets).
from app.core.dependencies import _enforce_token_limits, _resolve_token
from app.core.errors import APIError, ErrorCode
from app.core.scopes import mask_grants
from app.core.utils import to_oid
from app.site_auth.dependencies import get_current_site_user
from app.site_auth.models import SiteUser
from app.tokens.models import ApiToken
from app.trove.mods_hub import creators
from app.trove.mods_hub.models import ModCreatorLink, ModProject, ModRelease

MODS_WRITE_SCOPE = "mods:write"

# Header alternative to ``?creator=`` for picking which connected creator to act
# as, on the routes that carry no ``{handle}`` of their own.
CREATOR_HEADER = "X-Kiwi-Creator"

_scheme = HTTPBearer(
    scheme_name="ModsHubWrite",
    description="Dashboard session JWT, or an API token with the mods:write scope",
    auto_error=False,
)

# Route template -> methods an API caller may use. Default-deny (see module docs).
_API_ROUTES: dict[str, frozenset[str]] = {
    "/v1/mods/hub/me/projects": frozenset({"GET"}),
    "/v1/mods/hub/projects": frozenset({"POST"}),
    # No DELETE: removing a mod stays a website action.
    "/v1/mods/hub/projects/{handle}/{slug}": frozenset({"PATCH"}),
    "/v1/mods/hub/projects/{handle}/{slug}/fork": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/banner": frozenset({"POST", "DELETE"}),
    "/v1/mods/hub/projects/{handle}/{slug}/previews": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/previews/{sha}": frozenset({"DELETE"}),
    "/v1/mods/hub/projects/{handle}/{slug}/branches": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/branches/{name}": frozenset({"DELETE"}),
    "/v1/mods/hub/projects/{handle}/{slug}/commits": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/releases": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/releases/upload": frozenset({"POST"}),
    "/v1/mods/hub/projects/{handle}/{slug}/fix-placement": frozenset({"POST"}),
    # Publish / unpublish / retitle an existing release.
    "/v1/mods/hub/releases/{release_id}": frozenset({"PATCH"}),
    # Pack a config into an existing build (repacks the artifact - same trust level
    # as cutting a release, which is already allowed above).
    "/v1/mods/hub/releases/{release_id}/config": frozenset({"POST"}),
}

# Routes that bring a NEW mod into existence. A connection narrowed to named mods
# must not be able to mint more of them, so these require an all-projects link.
_CREATE_ROUTES = frozenset({
    "/v1/mods/hub/projects",
    "/v1/mods/hub/projects/{handle}/{slug}/fork",
})


@dataclass
class ModWriteAuth:
    """Resolved write caller. ``link``/``token`` are set only on the API path."""

    user: SiteUser                      # the creator whose mods are being managed
    link: ModCreatorLink | None = None
    token: ApiToken | None = None

    @property
    def via_api(self) -> bool:
        return self.link is not None


def _route_path(request: Request) -> str:
    return getattr(request.scope.get("route"), "path", "") or ""


def _forbidden(message: str) -> APIError:
    return APIError(403, ErrorCode.forbidden, message)


async def _target_project(request: Request) -> ModProject | None:
    """The mod this request acts on, from whatever the route gives us: a
    ``{handle}/{slug}`` pair, or a ``{release_id}`` we resolve through its release.
    ``None`` for routes that don't name one (create / list)."""
    params = request.path_params
    handle, slug = params.get("handle"), params.get("slug")
    if handle and slug:
        from app.trove.mods_hub import service

        return await service.get_project(handle, slug)
    release_id = params.get("release_id")
    if release_id:
        oid = to_oid(release_id)
        release = await ModRelease.get(oid) if oid else None
        if release is not None:
            return await ModProject.get(release.project_id)
    return None


async def _pick_creator(
    request: Request, links: list[ModCreatorLink], project: ModProject | None,
) -> ModCreatorLink:
    """Which connected creator is this call acting as?

    The mod itself decides when the route names one. Otherwise the caller selects
    with ``?creator=<handle>`` or the ``X-Kiwi-Creator`` header - unless they have
    exactly one connection, in which case there's nothing to disambiguate."""
    if project is not None:
        link = next((x for x in links if x.site_user_id == project.owner_id), None)
        if link is None:
            # Same message whether the mod is unknown to us or simply not covered:
            # a token shouldn't be usable to probe which mods exist.
            raise _forbidden("This token isn't connected to that mod's creator.")
        return link

    wanted = (request.query_params.get("creator")
              or request.headers.get(CREATOR_HEADER) or "").strip().lower()
    if not wanted:
        if len(links) == 1:
            return links[0]
        raise APIError(400, ErrorCode.bad_request,
                       "You're connected to several creators - name one with "
                       f"?creator=<handle> or the {CREATOR_HEADER} header.")
    creator = await SiteUser.find_one(SiteUser.username == wanted)
    link = next((x for x in links if creator is not None
                 and x.site_user_id == creator.id), None)
    if link is None:
        raise _forbidden(f"You aren't connected to the creator '{wanted}'.")
    return link


async def _authorize_api(
    request: Request, response: Response, creds: HTTPAuthorizationCredentials,
) -> ModWriteAuth:
    user, token = await _resolve_token(creds)
    if not mask_grants(token.scopes, MODS_WRITE_SCOPE):
        raise APIError(403, ErrorCode.insufficient_scope,
                       f"This token is missing the required scope: {MODS_WRITE_SCOPE}")
    await _enforce_token_limits(request, response, user, token)

    route = _route_path(request)
    allowed = _API_ROUTES.get(route)
    if allowed is None or request.method not in allowed:
        raise _forbidden(
            "This action is only available to the creator on the website. A "
            "connected API account can create mods, cut and publish releases, and "
            "edit mod metadata, images and visibility.")

    links = await creators.live_links(user)
    if not links:
        raise _forbidden(
            "This account isn't connected to any creator. Ask a creator for their "
            "creator token and add them from the dev portal.")

    project = await _target_project(request)
    link = await _pick_creator(request, links, project)
    if project is not None and not creators.covers(link, project):
        raise _forbidden("That mod isn't covered by your connection to this creator.")
    if route in _CREATE_ROUTES and not link.all_projects:
        raise _forbidden(
            "This creator limited your connection to specific mods, so it can't "
            "create new ones.")

    creator = await SiteUser.get(link.site_user_id)
    if creator is None or not creator.is_active or creator.is_deleted:
        raise _forbidden("That creator's account is no longer active.")
    await creators.touch(link)
    return ModWriteAuth(user=creator, link=link, token=token)


async def get_mod_write_auth(
    request: Request,
    response: Response,
    creds: HTTPAuthorizationCredentials | None = Depends(_scheme),
) -> ModWriteAuth:
    """Resolve a Mods Hub write caller - Dashboard session or connected API account."""
    if creds is not None and creds.credentials.count(".") != 2:
        return await _authorize_api(request, response, creds)
    # Everything else is the website's own session. No header at all is the normal
    # case now that the session is an HttpOnly cookie, so this must not short out
    # on `creds is None` - the cookie is read inside get_current_site_user, which
    # raises the 401 when there is genuinely nothing to authenticate.
    return ModWriteAuth(user=await get_current_site_user(request, creds))


async def get_mod_write_user(
    auth: ModWriteAuth = Depends(get_mod_write_auth),
) -> SiteUser:
    """The creator to act as. Routes depend on this and stay unaware of which
    credential got them here - every permission question is settled above."""
    return auth.user
