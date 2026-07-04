"""Request bodies for the DM-subscription Dashboard CRUD."""

from __future__ import annotations

from pydantic import BaseModel, Field


class DmSubCreate(BaseModel):
    events: list[str] = Field(default_factory=list)
    filters: dict | None = None
    label: str = ""


class DmSubUpdate(BaseModel):
    events: list[str] | None = None
    filters: dict | None = None
    label: str | None = None
    active: bool | None = None
