"""Request body for creating/updating an image design (validated by FastAPI, so the
service never has to validate a Beanie Document directly)."""

from __future__ import annotations

from pydantic import BaseModel, Field

from app.images.models import Background, Layer


class DesignBody(BaseModel):
    name: str = Field("", max_length=120)
    width: int = 600
    height: int = 240
    background: Background = Field(default_factory=Background)
    layers: list[Layer] = Field(default_factory=list)
    bind_type: str | None = None
