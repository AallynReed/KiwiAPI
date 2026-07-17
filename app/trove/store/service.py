"""Write- and read-side helpers for the store scope.

Insert flow:
1. Parse the cfg dump (pure parser) into categories + products.
2. Upsert each product by ``code``: every field is refreshed from the dump,
   ``first_seen`` is kept from the first sighting, ``last_seen`` gets the
   ingest anchor, and a ``price_history`` point is appended ONLY when the
   price signature changed (so sales / price bumps are queryable without
   bloating the doc on every daily re-scrape).
3. Categories are replaced wholesale (label / icon / display-ordered codes).
4. The singleton state doc records the anchor; a product is "active" iff
   ``last_seen == state.last_anchor`` - delisted products simply stop being
   re-seen and fall out of the default read view (never deleted).

``owned`` from the dump is the scraper account's ownership - deliberately NOT
persisted (it's not catalog data).
"""

from __future__ import annotations

import logging

from app.core.utils import utcnow
from app.trove.store.models import (
    StoreCategoryDoc,
    StorePrice,
    StorePricePoint,
    StoreProductDoc,
    StoreStateDoc,
    StoreTexture,
)
from app.trove.store.parser import ParsedProduct, parse_dump

logger = logging.getLogger(__name__)


async def _get_state() -> StoreStateDoc:
    state = await StoreStateDoc.find_one(StoreStateDoc.key == "state")
    if state is None:
        state = StoreStateDoc()
    return state


def _price_signature(prices: list[StorePrice], price_string: str) -> tuple:
    """Order- AND multiplicity-independent identity of a product's purchase
    options (a set: mod v0.1 stored duplicate entries for multi-tab products,
    so a plain sorted list would flag a phantom "price change" on the first
    ingest after the dedupe fix)."""
    return (
        tuple(sorted({(p.currency, p.cost, p.monthly, p.sale) for p in prices})),
        price_string,
    )


def _apply_parsed(doc: StoreProductDoc, p: ParsedProduct, anchor: int) -> bool:
    """Refresh ``doc`` from the parsed product. Returns True when the price
    signature changed (a history point was appended)."""
    new_prices = [
        StorePrice(currency=pr.currency, cost=pr.cost, can_purchase=pr.can_purchase,
                   monthly=pr.monthly, sale=pr.sale)
        for pr in p.prices
    ]
    changed = _price_signature(new_prices, p.price_string) != _price_signature(
        doc.prices, doc.price_string,
    )

    doc.kind = p.kind
    doc.name = p.name
    doc.image = p.image
    doc.info = p.info
    doc.informational = p.informational
    doc.tradable = p.tradable
    doc.prices = new_prices
    doc.price_string = p.price_string
    doc.price_string_currency = p.price_string_currency
    doc.price_string_sale = p.price_string_sale
    doc.promo = p.promo
    doc.deal_expires_at = (
        anchor + int(p.deal_seconds) if p.deal_seconds is not None else None
    )
    doc.interact_label = p.interact_label
    doc.interact_enabled = p.interact_enabled
    doc.trial_limits = p.trial_limits
    doc.class_level = p.class_level
    doc.class_power_rank = p.class_power_rank
    doc.class_shield_frame = p.class_shield_frame
    doc.class_sub_name = p.class_sub_name
    doc.class_icon = p.class_icon
    doc.textures = [
        StoreTexture(texture=t.texture, x=t.x, y=t.y, text=t.text, overlay=t.overlay)
        for t in p.textures
    ]
    # The loot pass is optional/timeout-prone: keep the last known loot text
    # rather than blanking it when a probe misses.
    if p.loot_body:
        doc.loot_title = p.loot_title
        doc.loot_body = p.loot_body
    doc.categories = p.categories
    doc.last_seen = anchor
    if changed:
        doc.price_history.append(StorePricePoint(
            ts=anchor, prices=new_prices, price_string=p.price_string,
        ))
    return changed


def _record_availability(doc: StoreProductDoc, anchor: int, prev_anchor: int | None) -> None:
    """Extend or open the product's availability interval for this ingest.

    Continuity is decided against ``prev_anchor`` (the PREVIOUS ingest's
    anchor) - NOT against wall-clock - so it's correct at any cadence (daily,
    hourly, irregular back-fills): a product whose last interval ended exactly
    at the previous ingest is still continuously present, so we extend it;
    anything else (first sighting, or a return after one or more ingests where
    it was absent) opens a fresh interval. Must run BEFORE ``_apply_parsed``
    overwrites ``last_seen`` so the legacy back-fill reads the prior value.
    """
    av = doc.availability
    # Back-fill a doc created before availability tracking existed: seed one
    # interval from its recorded first->last sighting so its history isn't lost.
    if not av and doc.first_seen and doc.last_seen and doc.last_seen != anchor:
        av = doc.availability = [[doc.first_seen, doc.last_seen]]
    if av and prev_anchor is not None and av[-1][1] == prev_anchor:
        av[-1][1] = anchor                 # continuous with the previous ingest
    elif av and av[-1][1] == anchor:
        pass                               # idempotent re-ingest of same anchor
    else:
        av.append([anchor, anchor])        # first sighting or returned after a gap


async def insert_dump(text: str, timestamp: int | None = None) -> dict:
    """Ingest one StoreLog.cfg dump. Raises ``ValueError`` when the dump
    contains no recognisable store data (bad upload, wrong file)."""
    parsed = parse_dump(text)
    if not parsed.products and not parsed.categories:
        raise ValueError("No store data found in the upload - is this a StoreLog.cfg?")
    anchor = timestamp if timestamp is not None else int(utcnow().timestamp())
    # The PREVIOUS ingest's anchor, read before we overwrite it below - drives
    # per-product availability continuity.
    prev_anchor = (await _get_state()).last_anchor

    price_changes = 0
    created = 0
    for p in parsed.products:
        doc = await StoreProductDoc.find_one(StoreProductDoc.code == p.code)
        if doc is None:
            doc = StoreProductDoc(code=p.code, kind=p.kind, first_seen=anchor)
            created += 1
        # Record availability BEFORE _apply_parsed overwrites last_seen.
        _record_availability(doc, anchor, prev_anchor)
        if _apply_parsed(doc, p, anchor):
            price_changes += 1
        await doc.save()

    for c in parsed.categories:
        cat = await StoreCategoryDoc.find_one(StoreCategoryDoc.index == c.index)
        if cat is None:
            cat = StoreCategoryDoc(index=c.index)
        cat.label = c.label
        cat.icon = c.icon
        cat.codes = c.codes
        cat.last_seen = anchor
        await cat.save()

    state = await _get_state()
    state.last_anchor = anchor
    state.title = parsed.title or state.title
    state.product_count = len(parsed.products)
    state.category_count = len(parsed.categories)
    await state.save()

    summary = {
        "products": len(parsed.products),
        "categories": len(parsed.categories),
        "created": created,
        "price_changes": price_changes,
        "anchor": anchor,
        "done_marker": parsed.done,
    }
    logger.info("store ingest: %s", summary)
    return summary


_DAY = 86400


def _interval_days(start: int, end: int) -> int:
    """Whole days an availability interval covers. A single-snapshot interval
    ``[a, a]`` counts as 1 day seen; each extra day of continuous presence adds
    one (daily ingests → ``[a, a+6d]`` = 7 days)."""
    return max(1, round((end - start) / _DAY) + 1)


def _records(doc: StoreProductDoc, anchor: int | None) -> dict:
    """Derived at-a-glance stats for a product's detail panel. Cheap pure math
    over ``availability`` + ``price_history`` (+ current prices)."""
    av = [iv for iv in doc.availability if len(iv) == 2]
    total_days = sum(_interval_days(s, e) for s, e in av)
    longest = max((_interval_days(s, e) for s, e in av), default=0)
    active = anchor is not None and doc.last_seen == anchor

    # Cheapest / priciest each currency has EVER been (history points carry the
    # full price set at each change; fold the current prices in too).
    lows: dict[str, float] = {}
    highs: dict[str, float] = {}
    price_sets = [pt.prices for pt in doc.price_history] + [doc.prices]
    for pset in price_sets:
        for pr in pset:
            lows[pr.currency] = min(lows.get(pr.currency, pr.cost), pr.cost)
            highs[pr.currency] = max(highs.get(pr.currency, pr.cost), pr.cost)

    return {
        "times_available": len(av),
        "returns": max(0, len(av) - 1),      # times it came back after leaving
        "total_days_seen": total_days,
        "longest_run_days": longest,
        "first_seen": doc.first_seen,
        "last_seen": doc.last_seen,
        "currently_active": active,
        # Whole days since last present (only meaningful while gone). Uses now()
        # so it's live, not frozen at the last ingest.
        "gap_days": (None if active or not doc.last_seen
                     else max(0, (int(utcnow().timestamp()) - doc.last_seen) // _DAY)),
        "price_low": lows or None,
        "price_high": highs or None,
        "price_changes": len(doc.price_history),
    }


def _doc_out(doc: StoreProductDoc, anchor: int | None, *, history: bool = False) -> dict:
    out = {
        "code": doc.code,
        "kind": doc.kind,
        "name": doc.name,
        "image": doc.image,
        "info": doc.info,
        "informational": doc.informational,
        "tradable": doc.tradable,
        "prices": [p.model_dump() for p in doc.prices],
        "price_string": doc.price_string or None,
        "price_string_currency": doc.price_string_currency or None,
        "price_string_sale": doc.price_string_sale or None,
        "promo": doc.promo or None,
        "deal_expires_at": doc.deal_expires_at,
        "interact_label": doc.interact_label or None,
        "interact_enabled": doc.interact_enabled,
        "trial_limits": doc.trial_limits or None,
        "class_level": doc.class_level,
        "class_power_rank": doc.class_power_rank,
        "class_sub_name": doc.class_sub_name or None,
        "class_icon": doc.class_icon or None,
        "textures": [t.model_dump() for t in doc.textures],
        "loot_title": doc.loot_title or None,
        "loot_body": doc.loot_body or None,
        "categories": doc.categories,
        "first_seen": doc.first_seen,
        "last_seen": doc.last_seen,
        "active": anchor is not None and doc.last_seen == anchor,
    }
    if history:
        out["price_history"] = [pt.model_dump() for pt in doc.price_history]
        out["availability"] = [list(iv) for iv in doc.availability]
        out["records"] = _records(doc, anchor)
    return out


async def list_products(
    *,
    category: int | None = None,
    kind: str | None = None,
    currency: str | None = None,
    q: str | None = None,
    active_only: bool = True,
    on_sale: bool = False,
    limit: int = 100,
    offset: int = 0,
) -> tuple[list[dict], int, int | None]:
    """Filtered catalog page. Returns (items, total, anchor)."""
    state = await _get_state()
    anchor = state.last_anchor

    query: dict = {}
    if active_only and anchor is not None:
        query["last_seen"] = anchor
    if category is not None:
        query["categories"] = category
    if kind is not None:
        query["kind"] = kind
    if currency is not None:
        query["prices.currency"] = currency
    if on_sale:
        query["$or"] = [
            {"prices": {"$elemMatch": {"sale": {"$ne": ""}}}},
            {"price_string_sale": {"$ne": ""}},
        ]
    if q:
        query["name"] = {"$regex": q, "$options": "i"}

    find = StoreProductDoc.find(query)
    total = await find.count()
    docs = await find.sort("+code").skip(offset).limit(limit).to_list()
    return [_doc_out(d, anchor) for d in docs], total, anchor


async def get_product(code: str) -> dict | None:
    state = await _get_state()
    doc = await StoreProductDoc.find_one(StoreProductDoc.code == code)
    if doc is None:
        return None
    return _doc_out(doc, state.last_anchor, history=True)


async def list_categories() -> tuple[list[dict], int | None]:
    state = await _get_state()
    anchor = state.last_anchor
    cats = await StoreCategoryDoc.find().sort("+index").to_list()
    return [
        {
            "index": c.index,
            "label": c.label,
            "icon": c.icon or None,
            "codes": c.codes,
            "count": len(c.codes),
            "active": anchor is not None and c.last_seen == anchor,
        }
        for c in cats
    ], anchor


async def timeline(*, kind: str | None = None, limit: int = 1000) -> dict:
    """Compact availability bands for every product - the global History tab's
    data source. One round-trip instead of N per-product calls: each item
    carries just what a Gantt row needs (code/name/kind/image + intervals +
    active), and ``span`` gives the axis bounds (oldest first_seen → now)."""
    state = await _get_state()
    anchor = state.last_anchor
    query: dict = {}
    if kind is not None:
        query["kind"] = kind
    # Most-recently-present first, so live/just-gone packs head the chart.
    docs = await StoreProductDoc.find(query).sort("-last_seen").limit(limit).to_list()

    items = [
        {
            "code": d.code,
            "name": d.name,
            "kind": d.kind,
            "image": d.image or None,
            "availability": [list(iv) for iv in d.availability],
            "first_seen": d.first_seen,
            "last_seen": d.last_seen,
            "active": anchor is not None and d.last_seen == anchor,
        }
        for d in docs
    ]
    now = int(utcnow().timestamp())
    span_start = min((d.first_seen for d in docs if d.first_seen), default=anchor or now)
    span_end = max([now, *(d.last_seen for d in docs if d.last_seen)]) if docs else now
    return {
        "anchor": anchor,
        "span": {"start": span_start, "end": span_end},
        "items": items,
        "count": len(items),
    }


async def resolve_texture_sha(path: str, branch: str) -> str | None:
    """content_sha256 for a game texture path in the updates CAS, tolerant of
    case / separator drift between the store's recorded path and the manifest
    (mirrors ``render.source._from_store``): try exact, then lowercased, then a
    case-insensitive regex. ``None`` if the branch has no such file."""
    import re

    from app.trove.updates import read as updates_read
    from app.trove.updates.models import UpdateState

    norm = path.replace("\\", "/").strip().lstrip("/")
    for candidate in (norm, norm.lower()):
        meta = await updates_read.get_file_meta(branch, candidate)
        if meta and meta["content_sha256"]:
            return meta["content_sha256"]
    # Last resort: anchored case-insensitive exact-path match.
    escaped = re.escape(norm)
    doc = await UpdateState.find_one(
        {"branch": branch, "path": {"$regex": f"^{escaped}$", "$options": "i"}}
    )
    return doc.content_sha256 if doc and doc.content_sha256 else None


async def reset() -> int:
    """Wipe every product / category / the state doc. Master-panel escape
    hatch (bad ingest, format change). Returns docs removed."""
    removed = 0
    for model in (StoreProductDoc, StoreCategoryDoc, StoreStateDoc):
        res = await model.find().delete()
        removed += res.deleted_count if res else 0
    logger.warning("store reset: %d docs removed", removed)
    return removed
