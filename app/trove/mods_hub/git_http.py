"""Authenticated git smart-HTTP server for the Mods Hub.

Exposes each project's repo at ``/git/mods/<slug>.git`` so a modder can
``git clone / pull / push`` locally. Auth is HTTP Basic with a git access token
as the password (site login is Discord-only, no password) - see
``service.authenticate_git`` + the ``/me/git-tokens`` endpoints.

  - clone / fetch / pull (``git-upload-pack``): allowed anonymously for public &
    unlisted projects; a draft requires its owner.
  - push (``git-receive-pack``): requires the authenticated owner.

The git wire protocol itself is handled by dulwich in ``gitstore`` (verified with
a real ``git`` client). These routes only do routing + auth + body plumbing.
"""

from __future__ import annotations

import base64

from fastapi import APIRouter, Query, Request, Response

from app.core.errors import APIError, ErrorCode
from app.site_auth.models import SiteUser
from app.trove.mods_hub import gitstore, service

git_router = APIRouter(tags=["mods-git"], include_in_schema=False)

_SERVICES = {"git-upload-pack", "git-receive-pack"}
_AUTH_CHALLENGE = {"WWW-Authenticate": 'Basic realm="Kiwi Mods Hub git"'}


def _basic_token(request: Request) -> str | None:
    """Extract the password (= git access token) from an HTTP Basic header.
    The username is ignored - the token alone identifies the user."""
    header = request.headers.get("Authorization", "")
    if not header.startswith("Basic "):
        return None
    try:
        _, _, pw = base64.b64decode(header[6:]).decode("utf-8", "replace").partition(":")
    except Exception:  # noqa: BLE001
        return None
    return pw or None


async def _authorize(handle: str, slug: str, request: Request, *, push: bool) -> tuple[object, SiteUser | None]:
    project = await service.get_project(handle, slug)
    token = _basic_token(request)
    user = await service.authenticate_git(token) if token else None

    if project is None:
        # Don't reveal which slugs exist: prompt for creds, then 404.
        if user is None:
            raise APIError(401, ErrorCode.not_authenticated, "Authentication required",
                           headers=_AUTH_CHALLENGE)
        raise APIError(404, ErrorCode.not_found, "Repository not found")

    if push:
        if user is None:
            raise APIError(401, ErrorCode.not_authenticated,
                           "A git access token is required to push", headers=_AUTH_CHALLENGE)
        if project.owner_id != user.id:
            raise APIError(403, ErrorCode.forbidden, "You can only push to your own mods")
    else:
        # Clone/pull needs the *source* visible: files-mode + public source (or
        # the owner). Private-source / releases-only repos are owner-only.
        if not service.source_visible(project, user):
            if user is None:
                raise APIError(401, ErrorCode.not_authenticated, "Authentication required",
                               headers=_AUTH_CHALLENGE)
            raise APIError(404, ErrorCode.not_found, "Repository not found")
    return project, user


def _nocache(content: bytes, content_type: str) -> Response:
    return Response(content=content, media_type=content_type,
                    headers={"Cache-Control": "no-cache, max-age=0, must-revalidate",
                             "Pragma": "no-cache"})


@git_router.get("/git/mods/{handle}/{slug}.git/info/refs")
async def info_refs(handle: str, slug: str, request: Request,
                    service_name: str = Query(alias="service")) -> Response:
    if service_name not in _SERVICES:
        raise APIError(403, ErrorCode.forbidden, "Unsupported service")
    project, _ = await _authorize(handle, slug, request, push=(service_name == "git-receive-pack"))
    body = await gitstore.advertise_refs(str(project.id), service_name.encode())
    return _nocache(body, f"application/x-{service_name}-advertisement")


@git_router.post("/git/mods/{handle}/{slug}.git/git-upload-pack")
async def upload_pack(handle: str, slug: str, request: Request) -> Response:
    project, _ = await _authorize(handle, slug, request, push=False)
    out = await gitstore.run_service(str(project.id), b"git-upload-pack", await request.body())
    return _nocache(out, "application/x-git-upload-pack-result")


@git_router.post("/git/mods/{handle}/{slug}.git/git-receive-pack")
async def receive_pack(handle: str, slug: str, request: Request) -> Response:
    project, _ = await _authorize(handle, slug, request, push=True)
    out = await gitstore.run_service(str(project.id), b"git-receive-pack", await request.body())
    await service.touch_after_push(project)   # bump updated_at; history is read live from git
    return _nocache(out, "application/x-git-receive-pack-result")
