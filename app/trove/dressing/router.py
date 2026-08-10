"""Public read API for the dressing room (``/v1/dressing/*``).

Tokenless, like the rest of the game-reference data: the catalogue is Trove's own
appearance list and the model is derived from files anyone can already read out of the
updates archive. Nothing here writes, and an outfit is never stored - the selection
travels in the query string and is resolved fresh on every request.
"""

from __future__ import annotations

from fastapi import APIRouter, Depends, Query, Request, Response

from app.core.dependencies import AccessContext, public_scope
from app.core.errors import COMMON_ERROR_RESPONSES, APIError, ErrorCode
from app.trove.dressing import catalogue as cat
from app.trove.dressing import service
from app.trove.dressing.schemas import (
    DressClassList,
    DressClassOut,
    DressOptionOut,
    DressOptionPage,
    DressOutfit,
    DressRaceList,
    DressRaceOut,
)
from app.trove.render import bp_cache

dressing_router = APIRouter(
    prefix="/v1/dressing", tags=["dressing room"], responses=COMMON_ERROR_RESPONSES,
)

_PUB = Depends(public_scope("codexes:read"))

_MAX_KEY = 220   # a style stem is short; a blueprint path is not


def _key(value: str | None) -> str | None:
    """A selection value: a style's prefab stem, or a blueprint reference. Bound the
    length before it reaches a lookup so an over-long query string is rejected rather
    than parsed; the shape of a blueprint name is checked in the service."""
    if value is None:
        return None
    value = value.strip().lower()
    if len(value) > _MAX_KEY:
        raise APIError(400, ErrorCode.bad_request, "Selection value is too long.")
    return value or None


def _empty() -> APIError:
    return APIError(503, ErrorCode.not_found,
                    "The dressing room catalogue isn't built yet - the game archive "
                    "hasn't been indexed on this instance.")


@dressing_router.get("/races", response_model=DressRaceList)
async def list_races(ctx: AccessContext = _PUB) -> DressRaceList:
    """The character-creation races. Each supplies the head and eyes the game draws
    whether or not anything covers them."""
    catalogue = await cat.get()
    if not catalogue:
        raise _empty()
    return DressRaceList(items=[
        DressRaceOut(key=r.key, name=r.name,
                     heads=len(r.pieces.get("head") or []),
                     eyes=len(r.pieces.get("eyes") or []))
        for r in catalogue.races.values()
    ])


@dressing_router.get("/classes", response_model=DressClassList)
async def list_classes(ctx: AccessContext = _PUB) -> DressClassList:
    """Every dressable class: its rig, its equipment sockets and the weapon families
    those sockets accept - read from the class's own prefab, so the compatibility rules
    are the game's."""
    catalogue = await cat.get()
    if not catalogue:
        raise _empty()
    return DressClassList(items=[
        DressClassOut(
            key=c.key, name=c.name, skeleton=c.skeleton, weapons=c.weapons,
            sockets=[{"ap": s["ap"], "slot": s["slot"], "family": s["family"]}
                     for s in c.sockets],
            costumes=len(catalogue.costumes.get(c.key) or []),
        )
        for c in sorted(catalogue.classes.values(), key=lambda c: c.name.lower())
    ])


@dressing_router.get("/options", response_model=DressOptionPage)
async def list_options(
    slot: str = Query(..., pattern="^(costume|hat|face|weapon|head|hair|eyes)$"),
    class_key: str | None = Query(default=None, alias="class",
                                  description="Required for `costume`; filters `weapon` "
                                              "to the families this class can hold."),
    race: str | None = Query(default=None, description="Filters `head`/`eyes` to that "
                             "race's own pieces."),
    q: str | None = Query(default=None, max_length=80, description="Name search."),
    offset: int = Query(default=0, ge=0),
    limit: int = Query(default=100, ge=1, le=500),
    ctx: AccessContext = _PUB,
) -> DressOptionPage:
    """The options for one slot. Costumes are per class (a costume belongs to the class
    whose skeleton it binds); weapon styles are filtered to the families the class has a
    socket for, so an incompatible style is never offered in the first place."""
    catalogue = await cat.get()
    if not catalogue:
        raise _empty()

    cls = catalogue.classes.get((class_key or "").lower()) if class_key else None
    if slot == "costume":
        if not cls:
            raise APIError(400, ErrorCode.bad_request, "`class` is required for costumes.")
        items = catalogue.costumes.get(cls.key) or []
    else:
        items = catalogue.styles.get(slot) or []
        if slot == "weapon" and cls:
            allowed = {s["slot"] for s in cls.sockets}
            items = [o for o in items if o.slot_id in allowed]
        # Heads and eyes belong to a race; hair is shared by all of them.
        chosen = catalogue.races.get((race or "").lower())
        if chosen and slot in ("head", "eyes"):
            mine = {b.lower() for b in chosen.pieces.get(slot, [])}
            items = [o for o in items if o.key in mine]

    if q:
        needle = q.strip().lower()
        items = [o for o in items if needle in o.name.lower() or needle in o.key]
    page = items[offset:offset + limit]
    return DressOptionPage(
        items=[DressOptionOut(key=o.key, name=o.name, slot=o.slot, family=o.family,
                              blueprint=o.blueprint, prefab=o.prefab,
                              credit=o.credit) for o in page],
        total=len(items), offset=offset, limit=limit,
    )


async def resolve_query(
    class_key: str, costume: str | None, hat: str | None,
    face: str | None, weapon: str | None, head: str | None = None,
    hair: str | None = None, weapon_family: str | None = None,
    eyes: str | None = None, race: str | None = None,
    hair_color: str | None = None, eye_color: str | None = None,
    skin_color: str | None = None,
) -> service.Outfit:
    """Shared by this router and the site proxy: validate a selection or 404."""
    outfit = await service.resolve(
        _key(class_key) or "", _key(costume),
        {"hat": _key(hat), "face": _key(face), "weapon": _key(weapon),
         "head": _key(head), "hair": _key(hair), "eyes": _key(eyes)},
        weapon_family=weapon_family, race=_key(race),
        colors={"hair_color": hair_color, "eye_color": eye_color, "skin_color": skin_color},
    )
    if outfit is None:
        raise APIError(404, ErrorCode.not_found, "Unknown class, or no costumes for it.")
    return outfit


@dressing_router.get("/outfit", response_model=DressOutfit)
async def get_outfit(
    class_key: str = Query(..., alias="class"),
    costume: str | None = Query(default=None),
    hat: str | None = Query(default=None),
    face: str | None = Query(default=None),
    weapon: str | None = Query(default=None),
    head: str | None = Query(default=None, description="Character-creation head. Omitted, "
                             "the race's default is drawn - the game shows a head with or "
                             "without a hat. `none` leaves it bare."),
    hair: str | None = Query(default=None, description="Character-creation hair style."),
    eyes: str | None = Query(default=None, description="Character-creation eyes (they ride "
                             "the `face` point, so a face style covers them)."),
    race: str | None = Query(default=None, description="Character-creation race - supplies "
                             "the default head and eyes."),
    hair_color: str | None = Query(default=None, description="`#rrggbb`. Tints the hair's "
                                   "mask voxels; see the model docs on how faithful this is."),
    eye_color: str | None = Query(default=None, description="`#rrggbb`."),
    skin_color: str | None = Query(default=None, description="`#rrggbb`. Tints a head's skin."),
    weapon_family: str | None = Query(default=None, description="Which weapon socket a raw "
                                      "blueprint fills (Melee/Bow/Gun/Staff/Spear/Fist). "
                                      "Only the Boomeranger is ambiguous without it."),
    ctx: AccessContext = _PUB,
) -> DressOutfit:
    """Normalise a selection without building the model: what a share link actually
    resolves to, plus any slot this class has no socket for."""
    outfit = await resolve_query(class_key, costume, hat, face, weapon, head, hair,
                                 weapon_family, eyes, race,
                                 hair_color, eye_color, skin_color)
    return DressOutfit(**outfit.as_dict(), dropped=outfit.dropped)


@dressing_router.get(
    "/model",
    responses={200: {"content": {"application/json": {}},
                     "description": "Assembled character: parts, rest pose + animations."}},
)
async def get_model(
    request: Request,
    class_key: str = Query(..., alias="class"),
    costume: str | None = Query(default=None),
    hat: str | None = Query(default=None),
    face: str | None = Query(default=None),
    weapon: str | None = Query(default=None),
    head: str | None = Query(default=None, description="Character-creation head. Omitted, "
                             "the race's default is drawn - the game shows a head with or "
                             "without a hat. `none` leaves it bare."),
    hair: str | None = Query(default=None, description="Character-creation hair style."),
    eyes: str | None = Query(default=None, description="Character-creation eyes (they ride "
                             "the `face` point, so a face style covers them)."),
    race: str | None = Query(default=None, description="Character-creation race - supplies "
                             "the default head and eyes."),
    hair_color: str | None = Query(default=None, description="`#rrggbb`. Tints the hair's "
                                   "mask voxels; see the model docs on how faithful this is."),
    eye_color: str | None = Query(default=None, description="`#rrggbb`."),
    skin_color: str | None = Query(default=None, description="`#rrggbb`. Tints a head's skin."),
    weapon_family: str | None = Query(default=None, description="Which weapon socket a raw "
                                      "blueprint fills (Melee/Bow/Gun/Staff/Spear/Fist). "
                                      "Only the Boomeranger is ambiguous without it."),
    fmt: str = Query(default="json", pattern="^(json|bin)$",
                     description="`json` (default) or `bin` - the compact KVX1 container."),
    ctx: AccessContext = _PUB,
) -> Response:
    """The dressed character as the web-viewer model payload - the same shape the Mods
    Hub's assembled creature uses, so the same viewer draws it.

    Built once per outfit and cached, then served gzipped with an ``ETag``."""
    outfit = await resolve_query(class_key, costume, hat, face, weapon, head, hair,
                                 weapon_family, eyes, race,
                                 hair_color, eye_color, skin_color)
    built = await service.model(outfit, fmt)
    if built is None:
        raise APIError(404, ErrorCode.not_found,
                       "That outfit has nothing to draw on this instance.")
    return bp_cache.respond(request, built)
