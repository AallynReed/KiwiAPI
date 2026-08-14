"""Price the static tome payout table against live marketplace medians.

Four price sources per reward, declared in the data file rather than inferred:

  market  the reward's own name is traded - median price-each
  market  with ``item``/``per``: the reward is not traded but a container of it
          is (16 quilt *sections* come from Samplebooks worth 2 sections each,
          5 Diamond Dragonite from a Pouch of 10), so unit = median / per
  flux    the currency itself, worth 1 by definition
  none    untradeable - it will NEVER have a market price, which is a different
          statement from "no listings right now" and is surfaced as such

A tome's total is ``None`` unless EVERY reward resolves, matching the crafting
calculator: one unpriced reward must never be silently dropped from a sum, or a
half-counted payout reads as a cheap tome. ``known_value`` keeps the partial for
display, clearly labelled.
"""

from __future__ import annotations

import json
from pathlib import Path

from app.trove import server_time as trove_server_time
from app.trove.market import service as market_service

_DATA_FILE = Path(__file__).resolve().parent.parent / "gamedata" / "tomes.json"

_TABLE: dict | None = None


def _table() -> dict:
    global _TABLE
    if _TABLE is None:
        _TABLE = json.loads(_DATA_FILE.read_text(encoding="utf-8"))
    return _TABLE


def _lookup_name(reward: dict) -> str | None:
    """The marketplace item name whose median prices this reward, or ``None``
    when the reward is untradeable or is flux itself."""
    price = reward.get("price") or {}
    if price.get("source") != "market":
        return None
    return price.get("item") or reward["item"]


def market_names() -> list[str]:
    """Every marketplace name the table needs, deduped - one bulk median query
    covers the whole page."""
    names = {
        n for tome in _table()["tomes"]
        for r in tome["rewards"]
        if (n := _lookup_name(r))
    }
    return sorted(names)


def _price_reward(reward: dict, medians: dict) -> dict:
    price = reward.get("price") or {}
    source = price.get("source")
    out = {
        "item": reward["item"],
        "amount": reward["amount"],
        "source": source,
        "via": None,
        "unit_price": None,
        "value": None,
        "listings": 0,
    }

    if source == "flux":
        out["unit_price"] = 1.0
    elif source == "market":
        name = _lookup_name(reward)
        per = price.get("per") or 1
        if name != reward["item"]:
            out["via"] = name
            out["per"] = per
        row = medians.get(name)
        if row:
            out["unit_price"] = row["median_each"] / per
            out["listings"] = row["count"]

    if out["unit_price"] is not None:
        out["value"] = round(out["unit_price"] * reward["amount"], 2)
    return out


def _price_tome(tome: dict, medians: dict) -> dict:
    rewards = [_price_reward(r, medians) for r in tome["rewards"]]
    untradeable = [r["item"] for r in rewards if r["source"] == "none"]
    # Tradeable but nothing listed right now - transient, unlike untradeable.
    # Named by what we actually price against: a quilt *section* never trades,
    # its Samplebook does, so "no Samplebook listings" is the true statement.
    unlisted = [r["via"] or r["item"] for r in rewards
                if r["source"] == "market" and r["unit_price"] is None]

    known = sum(r["value"] for r in rewards if r["value"] is not None)
    complete = bool(rewards) and not untradeable and not unlisted

    return {
        "name": tome["name"],
        "type": tome["type"],
        "note": tome.get("note"),
        "rewards": rewards,
        "value": round(known, 2) if complete else None,
        "known_value": round(known, 2) if rewards else None,
        "untradeable": untradeable,
        "unlisted": unlisted,
        # Why there is no number, so the page never has to guess from absence.
        "status": ("priced" if complete
                   else "no_payout_data" if not rewards
                   else "untradeable" if untradeable
                   else "unlisted"),
    }


async def valued_tomes() -> dict:
    """Every tome with its payout priced at current medians.

    ``best_regular`` is the yardstick the legendary list is read against: a
    legendary worth less than the best repeatable tome is a poor use of a weekly
    slot, since the regular one can simply be farmed again.
    """
    medians = await market_service.medians_for_names(market_names())
    tomes = [_price_tome(t, medians) for t in _table()["tomes"]]

    regular_values = [t["value"] for t in tomes
                      if t["type"] == "regular" and t["value"] is not None]

    return {
        "tomes": tomes,
        "best_regular": max(regular_values) if regular_values else None,
        "priced_items": len(medians),
        "weekly_reset_at": trove_server_time.server_time()["weekly_reset_at"],
    }
