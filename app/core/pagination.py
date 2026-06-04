"""Cursor (keyset) pagination over Mongo ObjectId.

Offset/`skip` pagination degrades on large collections (the server walks and
discards every skipped row) and can repeat or drop rows when data shifts between
pages. Keyset pagination instead carries an opaque cursor — here the last
ObjectId seen — and asks for "the next N after this id". ObjectIds are
monotonic-ish and unique, so newest-first ordering is ``_id`` descending and the
next page is ``_id < cursor``.
"""

from dataclasses import dataclass
from typing import Generic, TypeVar

from beanie import PydanticObjectId
from fastapi import Query
from pydantic import BaseModel

from app.core.errors import APIError, ErrorCode

T = TypeVar("T")

DEFAULT_LIMIT = 50
MAX_LIMIT = 200


class Page(BaseModel, Generic[T]):
    items: list[T]
    next_cursor: str | None = None  # pass back as ?cursor= to fetch the next page
    has_more: bool = False


@dataclass
class ListParams:
    """Standard cursor-pagination query params for any list endpoint."""

    cursor: str | None
    limit: int


def list_params(
    cursor: str | None = Query(default=None, description="next_cursor from a prior page"),
    limit: int = Query(default=DEFAULT_LIMIT, ge=1, le=MAX_LIMIT),
) -> ListParams:
    return ListParams(cursor=cursor, limit=limit)


def decode_cursor(cursor: str | None) -> PydanticObjectId | None:
    if cursor is None:
        return None
    try:
        return PydanticObjectId(cursor)
    except Exception:
        raise APIError(status_code=400, code=ErrorCode.bad_request, message="Invalid cursor")


async def paginate_newest_first(
    document_cls,
    base_filter: dict,
    cursor: str | None,
    limit: int,
) -> tuple[list, str | None, bool]:
    """Return (docs, next_cursor, has_more) newest-first by ``_id``.

    ``base_filter`` is a raw Mongo filter scoping the query (e.g. by user). Fetch
    one extra row to know whether a further page exists without a second query.
    """
    after = decode_cursor(cursor)
    query = dict(base_filter)
    if after is not None:
        query["_id"] = {"$lt": after}

    docs = await document_cls.find(query).sort("-_id").limit(limit + 1).to_list()
    has_more = len(docs) > limit
    docs = docs[:limit]
    next_cursor = str(docs[-1].id) if has_more and docs else None
    return docs, next_cursor, has_more
