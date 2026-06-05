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
    blueprint: str | None = None
    data: dict = {}                   # type-specific rich fields (populated incrementally)
    indexed_at: datetime


class CodexEntryPage(BaseModel):
    branch: str
    type: str
    items: list[CodexEntryOut]
    count: int                        # returned this page
    total: int                        # all matching entries (for paging)
