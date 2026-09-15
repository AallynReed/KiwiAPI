"""The statistics page's data plane: ``/site/mod-stats``.

The page itself is rendered by the website container (``app/web/pages.py`` ->
``/zakros-ui-stats``).
"""
from fastapi import APIRouter, Response

from app.mod_stats import service

router = APIRouter(tags=["mod-stats"], include_in_schema=False)


@router.get("/site/mod-stats")
async def mod_stats(response: Response) -> dict:
    response.headers["Cache-Control"] = "public, max-age=600"
    return await service.public_stats()
