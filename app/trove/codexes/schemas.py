"""Response models for the `codexes:read` endpoints."""

from datetime import datetime

from pydantic import BaseModel


class CodexTypeInfo(BaseModel):
    type: str                         # ally | mount | dragon | memento | recipe | item | fish | badge
    count: int                        # entries of this type in the branch


class CodexTypeList(BaseModel):
    branch: str
    items: list[CodexTypeInfo]
    count: int                        # number of types present


class CodexEntryOut(BaseModel):
    type: str
    path: str                         # source prefab logical path (stable id)
    name: str
    category: str = ""
    description: str = ""
    tradable: bool | None = None
    mastery: int | None = None        # normal collectible mastery (None for non-collectibles)
    mastery_geode: int | None = None  # geode-mode mastery (None unless geode-listed)
    power_rank: int | None = None     # collectible Power Rank (None if absent)
    blueprint: str | None = None
    data: dict = {}                   # rich fields: stats, abilities, geode_companion levels
    indexed_at: datetime


class CodexEntryPage(BaseModel):
    branch: str
    type: str
    items: list[CodexEntryOut]
    count: int                        # returned this page
    total: int                        # all matching entries (for paging)


class CodexSearchPage(BaseModel):
    branch: str
    type: str | None = None           # the type filter, if one was given (else cross-type)
    query: str | None = None          # the search term, if any
    items: list[CodexEntryOut]        # each carries its own `type`
    count: int                        # returned this page
    total: int                        # all matching entries (for paging)


class CodexCategoryInfo(BaseModel):
    category: str
    count: int


class CodexCategoryList(BaseModel):
    branch: str
    type: str | None = None           # the type these categories belong to (None = all)
    items: list[CodexCategoryInfo]    # distinct categories, A→Z
    count: int
