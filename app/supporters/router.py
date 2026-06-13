"""Public supporters endpoint - tokenless, under the misc:read scope.

Lives under the shared ``/v1/misc`` prefix (same group as trove-status etc.).
A token carrying ``misc:read`` still works and earns the wider rate budget, but
no token is required - the site and any dashboard can render the credits list.
"""
from fastapi import APIRouter, Depends, Response

from app.core.dependencies import AccessContext, public_scope
from app.supporters import service
from app.supporters.schemas import SupporterList

public_router = APIRouter(prefix="/v1/misc", tags=["misc"])
_MISC_PUBLIC = Depends(public_scope("misc:read"))


@public_router.get(
    "/supporters", response_model=SupporterList,
    summary="List project supporters",
)
async def list_supporters(
    response: Response, _ctx: AccessContext = _MISC_PUBLIC,
) -> SupporterList:
    """**Tokenless.** The people who support Better Trove Tools, in display
    order - the same list shown on the /support page. Cached 5 min."""
    response.headers["Cache-Control"] = "public, max-age=300"
    names = await service.list_public()
    return SupporterList(supporters=names, count=len(names))
