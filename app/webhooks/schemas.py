"""Request bodies for the Dashboard webhook endpoints."""

from __future__ import annotations

from pydantic import BaseModel, Field

from app.embed_templates import EmbedTemplate


class WebhookCreate(BaseModel):
    url: str = Field(..., max_length=400)
    events: list[str] = Field(..., min_length=1)
    label: str = Field("", max_length=80)
    templates: dict[str, EmbedTemplate] | None = None


class WebhookUpdate(BaseModel):
    url: str | None = Field(None, max_length=400)
    events: list[str] | None = None
    label: str | None = Field(None, max_length=80)
    active: bool | None = None
    templates: dict[str, EmbedTemplate] | None = None
