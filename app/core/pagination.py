"""Cursor (keyset) pagination over Mongo ObjectId.

The opaque cursor is the last ObjectId seen; newest-first is ``_id`` descending
and the next page is ``_id < cursor``. Preferred over offset/``skip`` on large
collections, which walks-and-discards skipped rows and can repeat or drop rows
when data shifts between pages.
"""

from typing import Generic, TypeVar

from beanie import PydanticObjectId
from pydantic import BaseModel

from app.core.errors import APIError, ErrorCode

T = TypeVar("T")


class Page(BaseModel, Generic[T]):
    items: list[T]
    next_cursor: str | None = None  # pass back as ?cursor= to fetch the next page
    has_more: bool = False


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


async def paginate(query, *, sort: str, limit: int, offset: int) -> tuple[list, int]:
    """Offset-page a Beanie query, returning ``(docs, total)``. For admin/history
    listings that show a total count; use ``paginate_newest_first`` for public
    cursor pages. Callers project ``docs`` into their own response shape."""
    total = await query.count()
    docs = await query.sort(sort).skip(offset).limit(limit).to_list()
    return docs, total
