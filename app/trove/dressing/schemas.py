"""Response shapes for the dressing room."""

from __future__ import annotations

from pydantic import BaseModel, Field


class DressSocket(BaseModel):
    ap: str = Field(description="Attach point on the rig the style is drawn at.")
    slot: int = Field(description="The game's equipment slot number.")
    family: str = Field(description="Display family: Hat, Face, Melee, Bow, Gun, …")


class DressClassOut(BaseModel):
    key: str = Field(description="Stable class id (its prefab stem, e.g. `knight`).")
    name: str
    skeleton: str = Field(description="Rig the class is animated on.")
    weapons: list[str] = Field(description="Weapon families this class can hold.")
    sockets: list[DressSocket]
    costumes: int = Field(description="How many costumes are available for it.")


class DressClassList(BaseModel):
    items: list[DressClassOut]


class DressOptionOut(BaseModel):
    key: str = Field(description="Stable option id (its prefab stem).")
    name: str
    slot: str = Field(description="costume | hat | face | weapon")
    family: str = Field(default="", description="Weapon family, for weapon styles.")
    blueprint: str = Field(default="", description="Model blueprint basename (styles).")
    prefab: str = Field(default="", description="Source prefab path.")
    credit: str = Field(default="", description="Community author, for a "
                        "player-submitted hair style.")


class DressOptionPage(BaseModel):
    items: list[DressOptionOut]
    total: int
    offset: int
    limit: int


class DressOutfit(BaseModel):
    """The canonical selection after validation - what the share link resolves to."""

    cls: str = Field(alias="class")
    costume: str
    hat: str | None = None
    face: str | None = None
    weapon: str | None = None
    head: str | None = None
    hair: str | None = None
    eyes: str | None = None
    race: str | None = None
    dropped: list[str] = Field(
        default_factory=list,
        description="Slots the request asked for that this class has no socket for.")

    model_config = {"populate_by_name": True}


class DressRaceOut(BaseModel):
    key: str
    name: str
    heads: int = Field(description="How many head pieces this race has.")
    eyes: int = Field(description="How many eye pieces this race has.")


class DressRaceList(BaseModel):
    items: list[DressRaceOut]
