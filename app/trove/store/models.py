"""Mongo models for the store scope.

The catalog is tiny (a few hundred products) and changes at most daily, so
Mongo/Beanie is the right store - no Postgres involvement. Three collections:

- ``StoreProductDoc``  - one document per product code, upserted each ingest.
  Carries an embedded ``price_history`` (appended only when the price
  signature actually changes), so sales / price bumps are queryable over time
  without a separate collection.
- ``StoreCategoryDoc`` - one document per store tab, replaced each ingest
  (label, icon, display-ordered product codes).
- ``StoreStateDoc``    - singleton with the latest ingest anchor; "active"
  product = ``last_seen == state.last_anchor`` (present in the latest dump).
"""

from beanie import Document
from pydantic import BaseModel, Field
from pymongo import ASCENDING, DESCENDING, IndexModel


class StorePrice(BaseModel):
    """One purchase option (a product can have up to two, e.g. TWC + TWP)."""

    currency: str            # "TWC" (credits) / "TWP" (cubits) / other
    cost: float
    can_purchase: bool       # engine-side purchasability at capture time
    monthly: int = 0         # patron: per-month price; 0 otherwise
    sale: str = ""           # sale-sticker loc key suffix ("" = no sale)


class StoreTexture(BaseModel):
    """One preview texture layer (tile art composition)."""

    texture: str
    x: int = 0
    y: int = 0
    text: str = ""           # overlay text ("" = none)
    overlay: bool = False


class StorePricePoint(BaseModel):
    """Price signature at ``ts`` - appended when it differs from the last one."""

    ts: int                                  # unix seconds (ingest anchor)
    prices: list[StorePrice] = Field(default_factory=list)
    price_string: str = ""                   # real-money SKUs


class StoreProductDoc(Document):
    code: str                # the engine's product code (stable key)
    kind: str                # product|starter|patron|interactable|trial|class
    name: str = ""
    image: str = ""
    info: str = ""
    informational: bool = False
    tradable: bool = False

    prices: list[StorePrice] = Field(default_factory=list)
    price_string: str = ""
    price_string_currency: str = ""
    price_string_sale: str = ""
    promo: str = ""
    deal_expires_at: int | None = None       # unix seconds (anchor + countdown)

    interact_label: str = ""
    interact_enabled: bool = False
    trial_limits: str = ""

    class_level: int | None = None
    class_power_rank: int | None = None
    class_shield_frame: int | None = None
    class_sub_name: str = ""
    class_icon: str = ""

    textures: list[StoreTexture] = Field(default_factory=list)
    loot_title: str = ""
    loot_body: str = ""                      # lootbox probability text

    categories: list[int] = Field(default_factory=list)

    first_seen: int = 0                      # unix seconds (first ingest)
    last_seen: int = 0                       # unix seconds (latest ingest seen in)
    price_history: list[StorePricePoint] = Field(default_factory=list)
    # Availability intervals ``[[start_anchor, end_anchor], ...]`` in unix
    # seconds - one entry per continuous run the product was present in the
    # store. A product that leaves and later returns has multiple intervals,
    # so an honest "when was this available" timeline is queryable (first_seen/
    # last_seen alone can't tell a gap from a continuous run). Appended/extended
    # each ingest by ``service._record_availability``.
    availability: list[list[int]] = Field(default_factory=list)

    class Settings:
        name = "store_products"
        indexes = [
            IndexModel([("code", ASCENDING)], unique=True),
            IndexModel([("last_seen", DESCENDING)]),
            IndexModel([("categories", ASCENDING)]),
        ]


class StoreCategoryDoc(Document):
    index: int               # the engine's category index (stable key)
    label: str = ""          # raw loc key (e.g. "$Store_Tab_Featured")
    icon: str = ""
    codes: list[str] = Field(default_factory=list)   # display order
    last_seen: int = 0

    class Settings:
        name = "store_categories"
        indexes = [
            IndexModel([("index", ASCENDING)], unique=True),
        ]


class StoreStateDoc(Document):
    """Singleton: the latest ingest anchor + dump-level metadata."""

    key: str = "state"
    last_anchor: int | None = None
    title: str = ""
    product_count: int = 0
    category_count: int = 0

    class Settings:
        name = "store_state"
        indexes = [
            IndexModel([("key", ASCENDING)], unique=True),
        ]
