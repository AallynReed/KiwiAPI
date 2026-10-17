"""Internal page-view ingest - the website container's beacon lands here.

Since the website/API split, pages are served by the ``web`` container, which
deliberately holds no database connection. Its collector (``pageviews.collect``)
hashes the visitor, labels the view and posts batches to this endpoint over the
compose network.

The api host is proxied by a catch-all ``location /``, so this path is reachable
from the internet as well - hence the shared-secret header. With
``INTERNAL_KEY`` unset the endpoint 404s outright (it does not fall open).
"""
import logging
from typing import Literal

from fastapi import APIRouter, Header
from pydantic import BaseModel, Field
from pymongo import UpdateOne
from pymongo.errors import BulkWriteError

from app.core.errors import APIError, ErrorCode
from app.core.internal_api import internal_key_ok
from app.core.utils import utcnow
from app.pageviews.models import PageView

logger = logging.getLogger("kiwi.pageviews")

router = APIRouter(prefix="/internal", include_in_schema=False)

_Label = Field(default=None, max_length=80)


class BeaconEvent(BaseModel):
    kind: Literal["pv", "end"] = "pv"
    pid: str | None = Field(default=None, max_length=40)
    route: str = Field(default="", max_length=200)
    path: str = Field(default="", max_length=500)
    visitor_hash: str = Field(default="", max_length=64)
    engaged_s: int = Field(default=0, ge=0, le=86_400)
    channel: str | None = _Label
    source: str | None = _Label
    device: str | None = _Label
    browser: str | None = _Label
    os: str | None = _Label
    lang: str | None = _Label


class BeaconBatch(BaseModel):
    events: list[BeaconEvent] = Field(max_length=500)


def _collection():
    return PageView.get_pymongo_collection()


async def record_batch(events: list[BeaconEvent]) -> None:
    """Insert the views, then apply the time beacons to them. Views go first so a
    time beacon in the same batch as its view still finds it."""
    now = utcnow()
    views = [
        {**e.model_dump(exclude={"kind"}), "engaged_s": 0, "created_at": now, "last_at": now}
        for e in events
        if e.kind == "pv" and e.route and e.path and len(e.visitor_hash) >= 8
    ]
    if views:
        try:
            await _collection().insert_many(views, ordered=False)
        except BulkWriteError as exc:
            # A replayed view hits the unique pid index; anything else is a real error.
            if any(err.get("code") != 11000 for err in exc.details.get("writeErrors", [])):
                raise

    engaged: dict[str, int] = {}
    for e in events:
        if e.kind == "end" and e.pid:
            engaged[e.pid] = max(engaged.get(e.pid, 0), e.engaged_s)
    if engaged:
        await _collection().bulk_write([
            UpdateOne({"pid": pid}, {"$max": {"engaged_s": s}, "$set": {"last_at": now}})
            for pid, s in engaged.items()
        ], ordered=False)


@router.post("/pageviews", status_code=202)
async def ingest_pageviews(
    batch: BeaconBatch, x_internal_key: str = Header(default=""),
) -> dict:
    """Record a batch of page views and time beacons from the website container."""
    if not internal_key_ok(x_internal_key):
        raise APIError(status_code=404, code=ErrorCode.not_found, message="Not found.")
    try:
        await record_batch(batch.events)
    except Exception:
        logger.exception("page-view batch failed")
    return {"accepted": len(batch.events)}
