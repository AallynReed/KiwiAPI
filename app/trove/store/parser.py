"""Parser for the raw StoreLog.cfg dump the bot POSTs.

The mod (TroveScraper/StoreScraper/StoreBot.as) writes:

    boot = 1                (mod loaded - the uploader's stop-pressing signal)
    cat$<index> = <enc(label)>;<enc(icon)>;<code,code,...>
    prod$<code> = <18 ``;``-separated fields, see below>
    meta$balance = <credits>;<points>
    meta$title = <enc(title)>
    meta$instructions = <enc(text)>
    done = true

``boot`` (and any other unknown key) is ignored here.

Free-text fields go through ``StoreProduct.enc()`` - a hand-rolled
percent-encoder byte-identical to ``encodeURIComponent`` (Iggy's AS3 toplevel
doesn't ship the real one), escaping ``;`` ``:`` ``|`` ``,`` ``=`` and
newlines as UTF-8 ``%XX`` - so ``;`` is always a field separator, ``|`` a
list separator and ``:`` a sub-field separator. This module is the exact
mirror of ``StoreProduct.toValue()`` - change both together or neither.

Product value fields (0-based):

     0 enc(code)          product code (authoritative; the cfg key repeats it)
     1 kind               product|starter|patron|interactable|trial|class
     2 enc(name)
     3 enc(image)
     4 owned              0/1 (scraper-account specific - recorded, not served)
     5 tradable           0/1 (hasTradableGrants)
     6 informational      0/1
     7 enc(info)          tooltip/description text
     8 prices             `CUR:cost:can01:monthly:enc(sale)` joined by `|`
     9 priceString        `enc(str):CUR:enc(sale)` (real-money SKUs) or ''
    10 enc(promo)
    11 dealSeconds        countdown remaining at capture, or ''
    12 interact           `enc(label):enabled01:enc(trialLimits)` or ''
    13 classData          `level:pr:shield:enc(sub):enc(icon)` or ''
    14 textures           `enc(tex):x:y:enc(text):overlay01` joined by `|`
    15 enc(lootTitle)
    16 enc(lootBody)      lootbox probability text ('' when not a lootbox)
    17 cats               category indices joined by `,`

Pure (no I/O, no DB) so it can be unit-tested cheaply. Bad rows are dropped -
never fail the whole dump on one weird line (same policy as the leaderboards
parser).
"""

from __future__ import annotations

import logging
from typing import NamedTuple
from urllib.parse import unquote

logger = logging.getLogger("kiwi.trove.store.parser")

PRODUCT_FIELD_COUNT = 18

KINDS = frozenset({"product", "starter", "patron", "interactable", "trial", "class"})


class ParsedPrice(NamedTuple):
    currency: str        # "TWC" / "TWP" / anything else the engine sends
    cost: float
    can_purchase: bool
    monthly: int         # patron: per-month price; 0 otherwise
    sale: str            # sale-sticker loc key suffix ('' = no sale)


class ParsedTexture(NamedTuple):
    texture: str
    x: int
    y: int
    text: str            # overlay text ('' = none)
    overlay: bool


class ParsedProduct(NamedTuple):
    code: str
    kind: str
    name: str
    image: str
    owned: bool
    tradable: bool
    informational: bool
    info: str
    prices: list[ParsedPrice]
    price_string: str            # pre-formatted real-money price ('' = none)
    price_string_currency: str
    price_string_sale: str
    promo: str
    deal_seconds: float | None   # limited-time deal countdown at capture
    interact_label: str
    interact_enabled: bool
    trial_limits: str
    class_level: int | None
    class_power_rank: int | None
    class_shield_frame: int | None
    class_sub_name: str
    class_icon: str
    textures: list[ParsedTexture]
    loot_title: str
    loot_body: str
    categories: list[int]        # category indices this code appeared under


class ParsedCategory(NamedTuple):
    index: int
    label: str           # raw loc key (e.g. "$Store_Tab_Featured")
    icon: str
    codes: list[str]     # product codes in display order


class ParsedStoreDump(NamedTuple):
    categories: list[ParsedCategory]
    products: list[ParsedProduct]
    balance: tuple[float, float] | None   # scraper account (credits, points)
    title: str
    instructions: str
    done: bool           # the bot's "scrape finished" marker was present


def _dec(s: str) -> str:
    """Reverse of AS3 encodeURIComponent. ``unquote`` (NOT unquote_plus -
    ``+`` is a literal plus in this scheme)."""
    return unquote(s)


def _parse_prices(raw: str) -> list[ParsedPrice]:
    # Exact duplicates are dropped (order otherwise preserved): a product
    # shown under several tabs got its prices re-pushed per tab by the engine,
    # and dumps from mod v0.1 recorded every push. The real ProductTile keys
    # purchase panels by currency (a re-push REPLACES), so an exact repeat is
    # never meaningful. The mod resets per-tile accumulators now; this keeps
    # old dumps parseable to the same clean result.
    out: list[ParsedPrice] = []
    for chunk in raw.split("|"):
        if not chunk:
            continue
        parts = chunk.split(":")
        if len(parts) != 5:
            continue
        cur, cost_s, can_s, monthly_s, sale = parts
        try:
            cost = float(cost_s)
            monthly = int(monthly_s)
        except ValueError:
            continue
        price = ParsedPrice(
            currency=cur, cost=cost, can_purchase=can_s == "1",
            monthly=monthly, sale=_dec(sale),
        )
        if price not in out:
            out.append(price)
    return out


def _parse_textures(raw: str) -> list[ParsedTexture]:
    # Same exact-duplicate drop as _parse_prices (texture layers were
    # re-pushed per tab too).
    out: list[ParsedTexture] = []
    for chunk in raw.split("|"):
        if not chunk:
            continue
        parts = chunk.split(":")
        if len(parts) != 5:
            continue
        tex, x_s, y_s, text, overlay_s = parts
        try:
            x, y = int(x_s), int(y_s)
        except ValueError:
            continue
        texture = ParsedTexture(
            texture=_dec(tex), x=x, y=y, text=_dec(text), overlay=overlay_s == "1",
        )
        if texture not in out:
            out.append(texture)
    return out


def _parse_product(value: str) -> ParsedProduct | None:
    fields = value.split(";")
    if len(fields) != PRODUCT_FIELD_COUNT:
        return None
    code = _dec(fields[0])
    kind = fields[1]
    if not code or kind not in KINDS:
        return None

    # priceString: `enc(str):CUR:enc(sale)` or ''
    ps, ps_cur, ps_sale = "", "", ""
    if fields[9]:
        ps_parts = fields[9].split(":")
        if len(ps_parts) == 3:
            ps, ps_cur, ps_sale = _dec(ps_parts[0]), ps_parts[1], _dec(ps_parts[2])

    deal_seconds: float | None = None
    if fields[11]:
        try:
            deal_seconds = float(fields[11])
        except ValueError:
            pass

    # interact: `enc(label):enabled01:enc(trialLimits)` or ''
    i_label, i_enabled, i_trial = "", False, ""
    if fields[12]:
        i_parts = fields[12].split(":")
        if len(i_parts) == 3:
            i_label, i_enabled, i_trial = _dec(i_parts[0]), i_parts[1] == "1", _dec(i_parts[2])

    # classData: `level:pr:shield:enc(sub):enc(icon)` or ''
    c_level = c_pr = c_shield = None
    c_sub = c_icon = ""
    if fields[13]:
        c_parts = fields[13].split(":")
        if len(c_parts) == 5:
            try:
                c_level, c_pr, c_shield = int(c_parts[0]), int(c_parts[1]), int(c_parts[2])
                c_sub, c_icon = _dec(c_parts[3]), _dec(c_parts[4])
            except ValueError:
                c_level = c_pr = c_shield = None

    categories: list[int] = []
    if fields[17]:
        for tok in fields[17].split(","):
            try:
                categories.append(int(tok))
            except ValueError:
                continue

    return ParsedProduct(
        code=code,
        kind=kind,
        name=_dec(fields[2]),
        image=_dec(fields[3]),
        owned=fields[4] == "1",
        tradable=fields[5] == "1",
        informational=fields[6] == "1",
        info=_dec(fields[7]),
        prices=_parse_prices(fields[8]),
        price_string=ps,
        price_string_currency=ps_cur,
        price_string_sale=ps_sale,
        promo=_dec(fields[10]),
        deal_seconds=deal_seconds,
        interact_label=i_label,
        interact_enabled=i_enabled,
        trial_limits=i_trial,
        class_level=c_level,
        class_power_rank=c_pr,
        class_shield_frame=c_shield,
        class_sub_name=c_sub,
        class_icon=c_icon,
        textures=_parse_textures(fields[14]),
        loot_title=_dec(fields[15]),
        loot_body=_dec(fields[16]),
        categories=categories,
    )


def _parse_category(index_s: str, value: str) -> ParsedCategory | None:
    try:
        index = int(index_s)
    except ValueError:
        return None
    parts = value.split(";")
    if len(parts) != 3:
        return None
    label, icon, codes_raw = parts
    codes = [c for c in codes_raw.split(",") if c]
    return ParsedCategory(index=index, label=_dec(label), icon=_dec(icon), codes=codes)


def parse_dump(text: str) -> ParsedStoreDump:
    """Parse a full StoreLog.cfg dump.

    Section headers (``[kiwistore.swf]``), comments and unrelated key=value
    lines are ignored. A ``prod$`` line whose value doesn't have exactly
    ``PRODUCT_FIELD_COUNT`` fields is dropped (and logged) rather than failing
    the dump. Duplicate keys: first wins (matches the leaderboards policy).
    """
    categories: dict[int, ParsedCategory] = {}
    products: dict[str, ParsedProduct] = {}
    balance: tuple[float, float] | None = None
    title = ""
    instructions = ""
    done = False

    for line in text.splitlines():
        line = line.strip()
        if not line or line.startswith(("[", "#", ";")):
            continue
        key, sep, value = line.partition("=")
        if not sep:
            continue
        key = key.strip()
        value = value.strip()

        if key == "done":
            done = value.lower() == "true"
        elif key.startswith("cat$"):
            cat = _parse_category(key[4:], value)
            if cat is None:
                logger.warning("store: dropped malformed category line key=%r", key)
            elif cat.index not in categories:
                categories[cat.index] = cat
        elif key.startswith("prod$"):
            prod = _parse_product(value)
            if prod is None:
                logger.warning("store: dropped malformed product line key=%r", key)
            elif prod.code not in products:
                products[prod.code] = prod
        elif key == "meta$balance":
            parts = value.split(";")
            if len(parts) == 2:
                try:
                    balance = (float(parts[0]), float(parts[1]))
                except ValueError:
                    pass
        elif key == "meta$title":
            title = _dec(value)
        elif key == "meta$instructions":
            instructions = _dec(value)

    return ParsedStoreDump(
        categories=[categories[i] for i in sorted(categories)],
        products=list(products.values()),
        balance=balance,
        title=title,
        instructions=instructions,
        done=done,
    )
