"""Supporters request/response schemas (public + admin)."""
from datetime import datetime

from pydantic import BaseModel


class SupporterList(BaseModel):
    """Public view - names only, in display (insertion) order."""
    supporters: list[str]
    count: int


class SupporterAdminView(BaseModel):
    """One supporter row with admin metadata. ``added_by`` is null for names
    inserted by the boot-time seeder; otherwise the admin User id (str)."""
    name: str
    added_by: str | None = None
    created_at: datetime


class SupporterAdminList(BaseModel):
    items: list[SupporterAdminView]
    count: int


class SupporterAddRequest(BaseModel):
    name: str


class SupporterBulkReplaceRequest(BaseModel):
    names: list[str]


class SupporterBulkReplaceResponse(BaseModel):
    removed: int   # rows deleted before insert
    added: int     # rows inserted (after de-dup + trim)
