"""``/site/wiki/*`` - the wiki's data plane. Reads are anonymous; writes need a
site session (editors write, everyone else suggests). See ``service.py``."""
from typing import Literal

from fastapi import APIRouter, Depends, Query, Response
from pydantic import BaseModel, Field

from app.core.errors import APIError, ErrorCode
from app.site_auth.dependencies import get_current_site_user, get_optional_site_user
from app.site_auth.models import SiteUser
from app.wiki import service

router = APIRouter(prefix="/site/wiki", tags=["wiki"], include_in_schema=False)


def _fresh(response: Response) -> None:
    # Pages change the moment an editor saves; never serve a stale copy.
    response.headers["Cache-Control"] = "no-cache"


class SaveBody(BaseModel):
    slug: str = Field(max_length=200)
    title: str | None = Field(default=None, max_length=500)
    body: str = Field(default="", max_length=service.BODY_MAX + 1000)
    summary: str | None = Field(default=None, max_length=1000)
    base_rev: int = Field(ge=0)
    suggestion_id: str | None = Field(default=None, max_length=40)


class RevertBody(BaseModel):
    slug: str = Field(max_length=200)
    rev: int = Field(ge=1)
    base_rev: int = Field(ge=1)


class DeleteBody(BaseModel):
    slug: str = Field(max_length=200)
    reason: str | None = Field(default=None, max_length=1000)
    base_rev: int = Field(ge=1)


class RejectBody(BaseModel):
    note: str | None = Field(default=None, max_length=2000)


@router.get("/me")
async def me(response: Response, user: SiteUser | None = Depends(get_optional_site_user)) -> dict:
    response.headers["Cache-Control"] = "no-store"
    editor = service.is_editor(user)
    return {
        "signed_in": user is not None,
        "username": user.username if user else None,
        "is_editor": editor,
        "pending": await service.pending_count() if editor else 0,
    }


@router.get("/page")
async def page(response: Response, slug: str = Query(max_length=200)) -> dict:
    _fresh(response)
    found = await service.get_page(slug.strip().strip("/").lower())
    if found is None:
        raise APIError(404, ErrorCode.not_found, "No such page.")
    return found


@router.get("/pages")
async def pages(response: Response) -> dict:
    _fresh(response)
    items = await service.list_pages()
    return {"items": items, "count": len(items)}


@router.get("/history")
async def history(response: Response, slug: str = Query(max_length=200),
                  limit: int = Query(default=50, ge=1, le=200),
                  offset: int = Query(default=0, ge=0)) -> dict:
    _fresh(response)
    return await service.history(slug.strip().strip("/").lower(), limit=limit, offset=offset)


@router.get("/revision")
async def revision(response: Response, slug: str = Query(max_length=200),
                   rev: int = Query(ge=1)) -> dict:
    _fresh(response)
    found = await service.revision(slug.strip().strip("/").lower(), rev)
    if found is None:
        raise APIError(404, ErrorCode.not_found, "No such revision.")
    return found


@router.get("/recent")
async def recent(response: Response, limit: int = Query(default=50, ge=1, le=200),
                 per_page: bool = False) -> dict:
    _fresh(response)
    return {"items": await service.recent(limit, per_page=per_page)}


@router.get("/search")
async def search(response: Response, q: str = Query(default="", max_length=200),
                 limit: int = Query(default=20, ge=1, le=50)) -> dict:
    _fresh(response)
    return {"query": q, "items": await service.search(q, limit)}


@router.post("/page")
async def save(body: SaveBody, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.save(user, slug=body.slug, title=body.title, body=body.body,
                              summary=body.summary, base_rev=body.base_rev,
                              suggestion_id=body.suggestion_id)


@router.post("/page/revert")
async def revert(body: RevertBody, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.revert(user, slug=body.slug, rev=body.rev, base_rev=body.base_rev)


@router.post("/page/delete")
async def delete(body: DeleteBody, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.delete(user, slug=body.slug, reason=body.reason, base_rev=body.base_rev)


@router.post("/suggestions", status_code=201)
async def suggest(body: SaveBody, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.suggest(user, slug=body.slug, title=body.title, body=body.body,
                                 summary=body.summary, base_rev=body.base_rev)


@router.get("/suggestions")
async def suggestions(response: Response, user: SiteUser = Depends(get_current_site_user),
                      mine: bool = False,
                      status: Literal["pending", "accepted", "rejected", "withdrawn"] | None = None,
                      limit: int = Query(default=50, ge=1, le=200),
                      offset: int = Query(default=0, ge=0)) -> dict:
    response.headers["Cache-Control"] = "no-store"
    return await service.list_suggestions(user, mine=mine, status=status, limit=limit, offset=offset)


@router.get("/suggestions/{sid}")
async def suggestion(sid: str, response: Response,
                     user: SiteUser = Depends(get_current_site_user)) -> dict:
    response.headers["Cache-Control"] = "no-store"
    return await service.get_suggestion(user, sid)


@router.post("/suggestions/{sid}/accept")
async def accept(sid: str, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.accept(user, sid)


@router.post("/suggestions/{sid}/reject")
async def reject(sid: str, body: RejectBody, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.reject(user, sid, body.note)


@router.post("/suggestions/{sid}/withdraw")
async def withdraw(sid: str, user: SiteUser = Depends(get_current_site_user)) -> dict:
    return await service.withdraw(user, sid)
