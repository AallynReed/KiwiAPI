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


class CodexLinkOut(BaseModel):
    rel: str                          # crafts | ingredient | craftable_at | unlocks | upgrade_cost | member_of
    path: str                         # the FAR end of the edge
    type: str | None = None           # the far end's codex type (None if it isn't an indexed prefab)
    name: str = ""
    category: str = ""
    blueprint: str | None = None
    qty: float | None = None          # output/ingredient amount, where the relation carries one
    data: dict = {}                   # relation extras (bench lane + crafting tab, member group)


class CodexLinkList(BaseModel):
    branch: str
    path: str                         # the entry the links were asked about
    direction: str                    # "out" (this -> others) or "in" (others -> this)
    items: list[CodexLinkOut]
    count: int


class CodexStatHolder(BaseModel):
    path: str
    type: str
    name: str
    category: str = ""
    blueprint: str | None = None
    stat: str                         # $Stat_… key
    stat_name: str = ""
    value: float | None = None        # normalized for display
    is_percent: bool = False
    slot: str | None = None           # $EquipmentSlot_… when the bonus is slot-conditional


class CodexStatPage(BaseModel):
    branch: str
    stat: str
    items: list[CodexStatHolder]      # strongest first; one row per entry
    count: int
    total: int


class CodexRequirementOut(BaseModel):
    rank: int
    rank_name: str                    # bronze … trovium
    collection: str = ""              # this rank's own collection path
    badge_id: str = ""
    completion_kind: str = ""         # metric | dragonsouls | STBossKilled | …
    requirement_key: str = ""
    label: str = ""                   # human-readable requirement
    amount: int | None = None
    difficulty: int = 0
    status: str = ""                  # decoded | blocked
    context: dict = {}


class CodexRequirementList(BaseModel):
    branch: str
    collection: str                   # collections/badge/<id>
    items: list[CodexRequirementOut]  # bronze first
    count: int


class CodexUpgradeNode(BaseModel):
    system_kind: str                  # geode_module | geode_companion | class_prestige
    system_key: str
    node_key: str
    rank: int | None = None
    costs: list[dict] = []            # [{item, quantity}, …]
    requires: list[str] = []          # other node keys named as prerequisites
    effects: dict = {}                # {name, description, abilities: [ref, …]}


class CodexUpgradeSystemInfo(BaseModel):
    system_kind: str
    system_key: str
    nodes: int


class CodexUpgradeSystemList(BaseModel):
    branch: str
    items: list[CodexUpgradeSystemInfo]
    count: int


class CodexUpgradeNodeList(BaseModel):
    branch: str
    system_key: str
    items: list[CodexUpgradeNode]     # rank order
    count: int


class CodexCategoryInfo(BaseModel):
    category: str
    count: int


class CodexCategoryList(BaseModel):
    branch: str
    type: str | None = None           # the type these categories belong to (None = all)
    items: list[CodexCategoryInfo]    # distinct categories, A→Z
    count: int
