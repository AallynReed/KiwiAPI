"""Mongo model for user-designed images."""

from datetime import datetime
from typing import Literal

from beanie import Document, PydanticObjectId
from pydantic import BaseModel, Field
from pymongo import ASCENDING, DESCENDING, IndexModel

from app.core.utils import utcnow

MIN_DIM = 32
MAX_DIM = 1600
MAX_LAYERS = 40
MAX_DESIGNS_PER_USER = 60


class Background(BaseModel):
    type: Literal["solid", "gradient", "image"] = "solid"
    color1: str = "#0d1117"
    color2: str = "#16213e"
    angle: int = 90                       # gradient direction (90 = top→bottom, 0 = left→right)
    image_sha: str | None = None          # CAS blob for type="image"
    fit: Literal["cover", "contain", "stretch"] = "cover"


class Layer(BaseModel):
    """One drawable layer. A single shape covers all three kinds (discriminated by
    ``type``) to keep the model + editor simple."""

    type: Literal["text", "rect", "image"] = "text"
    x: float = 0
    y: float = 0
    opacity: float = 1.0                  # 0..1, applies to every kind

    # text
    text: str = ""
    font_size: int = 32
    color: str = "#ffffff"
    bold: bool = False
    align: Literal["left", "center", "right"] = "left"
    max_width: int | None = None          # wrap width in px; None = single line

    # rect + image
    w: float = 120
    h: float = 48
    radius: int = 0                       # corner radius (rect + image)
    image_sha: str | None = None          # CAS blob for type="image"


class ImageDesign(Document):
    """A canvas + ordered layers, rendered to PNG by ``app/images/render.py``."""

    owner_id: PydanticObjectId
    name: str = ""
    width: int = 600
    height: int = 240
    background: Background = Field(default_factory=Background)
    layers: list[Layer] = Field(default_factory=list)
    # Optional event/announcement "kind" whose live variables fill the {placeholders}
    # when this design is rendered (e.g. "challenge", "game_update"); None = static.
    bind_type: str | None = None

    created_at: datetime = Field(default_factory=utcnow)
    updated_at: datetime = Field(default_factory=utcnow)

    class Settings:
        name = "image_designs"
        indexes = [IndexModel([("owner_id", ASCENDING), ("created_at", DESCENDING)])]
