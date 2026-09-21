"""Internal page-view ingest - the website container's beacon lands here.

Since the website/API split, showcase pages are served by the ``web`` container,
which deliberately holds no database connection; its middleware can't write a
``PageView`` row the way the API's does. Instead it hashes the visitor itself and
posts batches to this endpoint over the compose network, so Site Analytics keeps
counting real page loads.

The api host is proxied by a catch-all ``location /``, so this path is reachable
from the internet as well - hence the shared-secret header. With
``INTERNAL_KEY`` unset the endpoint 404s outright (it does not fall open), which
is the pre-split behaviour: no rows, but also nothing spoofable.
"""
import hmac
import logging

from fastapi import APIRouter, Header
from pydantic import BaseModel, Field

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.pageviews.models import PageView
from app.pageviews.recorder import recorder

logger = logging.getLogger("kiwi.pageviews")

router = APIRouter(prefix="/internal", include_in_schema=False)


class BeaconEvent(BaseModel):
    route: str = Field(max_length=200)        # matched route template
    path: str = Field(max_length=500)         # concrete page URL
    visitor_hash: str = Field(min_length=8, max_length=64)


class BeaconBatch(BaseModel):
    events: list[BeaconEvent] = Field(max_length=500)


def _authorised(supplied: str) -> bool:
    key = settings.internal_key
    return bool(key) and hmac.compare_digest(supplied, key)


@router.post("/pageviews", status_code=202)
async def ingest_pageviews(
    batch: BeaconBatch, x_internal_key: str = Header(default=""),
) -> dict:
    """Record a batch of page views beaconed by the website container."""
    if not _authorised(x_internal_key):
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Not found.")
    for event in batch.events:
        recorder.record(PageView(route=event.route, path=event.path,
                                 visitor_hash=event.visitor_hash))
    return {"accepted": len(batch.events)}
