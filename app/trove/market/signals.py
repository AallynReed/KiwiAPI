"""Anomaly detection over the marketplace - unusual moves, supply shocks, and
the shapes a duplication exploit leaves behind.

WHAT THIS DOES AND DOES NOT CLAIM
---------------------------------
Nothing here proves an exploit. The data is public listings: name, stack, price,
when it was posted. There is no seller identity, no trade log, no inventory
history - so "who" and "how" are permanently out of reach. What this can do is
say *this does not look like normal trading*, show the evidence, and name the
pattern it matches. Every verdict is worded as a resemblance, never a finding.
That is a deliberate product decision, not hedging: a false accusation about a
named player's market activity is worse than a missed detection.

WHY ROBUST STATISTICS
---------------------
Mean and standard deviation are useless here, because the outlier we are hunting
is *in the sample* and drags both toward itself - a big enough anomaly hides
itself by inflating the very threshold meant to catch it. So every baseline uses
the median and MAD (median absolute deviation), which a minority of extreme
points cannot move. The 0.6745 factor rescales MAD to be comparable to a normal
standard deviation, so the score reads on the familiar "sigma" intuition.

THE TWO DUPE SIGNATURES
-----------------------
They are opposites, which is what makes them separable:

  item dupe      One item floods. Supply spikes, price collapses, stacks get
                 unusually large. Confined to that item - everything else is
                 calm.

  currency dupe  Flux itself is duplicated, so flux buys less. MANY unrelated
                 items rise at once, with no supply spike anywhere. One item
                 rising is a market event; two thirds of the market rising in
                 the same week is not sixty independent events, it is the
                 denominator moving.

The second is why the market-wide breadth number exists. It is the only signal
that can distinguish "this item got expensive" from "flux got cheap", and no
per-item test can ever see it.
"""

from __future__ import annotations

import statistics
from collections import defaultdict

from app.core.config import settings
from app.trove.market import pg_store

DAY = 86400

# How much history a baseline needs before it is allowed to call anything
# unusual. Below this an item has not shown us what normal looks like, so it is
# skipped rather than guessed at.
MIN_BASELINE_DAYS = 6
# Listings in the scored day, below which the median is too thin to trust.
MIN_SAMPLES = 3

# Robust-z thresholds. 3.5 is the conventional MAD-outlier cutoff; the elevated
# tier is where a human should look, the extreme tier is where something is
# almost certainly structural rather than trading noise.
Z_ELEVATED = 3.5
Z_EXTREME = 6.0

# MAD rescaled to a normal-comparable sigma.
_MAD_TO_SIGMA = 0.6745

# Assumed floor on natural dispersion, as a share of the baseline. A series can
# be genuinely flat - an item that gets exactly 10 listings a day, or trades at
# one fixed price - and then MAD is 0. Treating that as "no opinion" silenced
# the loudest signal we have: a rock-steady item suddenly getting 140 listings
# is the whole point. Treating it as infinite sensitivity is just as wrong, so
# the floor says "assume at least this much wobble was always possible". On any
# series with real dispersion the true MAD exceeds it and this never binds.
MAD_FLOOR_RATIO = 0.05

# Materiality gates. A score says "unusual for this item", which on a very
# steady series can be true of a move nobody would notice. A dimension only
# counts if it is BOTH statistically odd and actually big.
MIN_PRICE_MOVE = 0.25   # 25% off baseline
MIN_COUNT_MOVE = 1.00   # a doubling of listings / stack size

# Share of tracked items that must move the same way in one day before the move
# is attributed to flux rather than to the items. Two thirds is well past what
# uncorrelated item news produces.
BREADTH_SHARE = 0.66
# A per-item move only counts toward breadth if it clears this, so ordinary
# noise on dozens of items cannot add up to a false market-wide alarm.
BREADTH_MIN_MOVE = 0.15


def _score(value: float, history: list[float]) -> tuple[float | None, float | None, float | None]:
    """``(robust_z, baseline, relative_change)`` for ``value`` against ``history``.

    All three are ``None`` when there is too little history, or the baseline is
    zero (nothing to be a multiple of). See ``MAD_FLOOR_RATIO`` for why a flat
    history still produces a score.
    """
    if len(history) < MIN_BASELINE_DAYS:
        return (None, None, None)
    med = statistics.median(history)
    if med <= 0:
        return (None, None, None)
    mad = statistics.median([abs(x - med) for x in history])
    mad = max(mad, MAD_FLOOR_RATIO * med)
    return (_MAD_TO_SIGMA * (value - med) / mad, med, (value - med) / med)


def _flagged(z: float | None, rel: float | None, min_move: float) -> bool:
    """Statistically odd AND big enough to care about.

    The size gate is symmetric in RATIO, not in percent. These are multiplicative
    quantities, so the mirror of "doubled" is "halved" (-50%), not "-100%" - a
    plain ``abs(rel) >= 1.0`` is unreachable downward, since nothing falls by
    more than all of itself. Comparing percentages instead of ratios here made
    every collapse invisible and left the squeeze pattern unreachable.
    """
    if z is None or rel is None or abs(z) < Z_ELEVATED:
        return False
    if rel >= 0:
        return rel >= min_move
    return rel <= (1.0 / (1.0 + min_move)) - 1.0


def _series_by_item(rows: list[dict]) -> dict[str, list[dict]]:
    out: dict[str, list[dict]] = defaultdict(list)
    for r in rows:
        out[r["name"]].append(r)
    for v in out.values():
        v.sort(key=lambda r: r["bucket"])
    return out


def _severity(*scores: float | None) -> str:
    peak = max((abs(s) for s in scores if s is not None), default=0.0)
    if peak >= Z_EXTREME:
        return "extreme"
    if peak >= Z_ELEVATED:
        return "elevated"
    return "normal"


def _classify(price_up: bool, price_down: bool, supply_up: bool,
              supply_down: bool, stack_up: bool) -> tuple[str, str]:
    """(pattern, plain-English reading). Ordered most-specific first, so the
    compound dupe-shaped signature wins over its individual parts."""
    up, down = price_up, price_down

    if down and supply_up and stack_up:
        return ("supply_flood", "Price collapsed while supply and stack sizes "
                                "both spiked - the shape a duplicated item makes.")
    if down and supply_up:
        return ("oversupply", "A lot more of this appeared and the price fell "
                              "with it.")
    if up and supply_down:
        return ("squeeze", "Supply dried up and the price ran up behind it.")
    if stack_up and not down:
        return ("odd_stacks", "Stack sizes are far larger than this item "
                              "normally trades in.")
    if up:
        return ("spike", "Priced well above its own recent range.")
    if down:
        return ("slump", "Priced well below its own recent range.")
    if supply_up:
        return ("supply_surge", "Many more listings than this item usually gets.")
    return ("unusual", "Moving outside its normal range.")


def _scan_item(name: str, series: list[dict], market_move: float = 0.0) -> dict | None:
    """Score the most recent complete day against everything before it.

    Prices are scored AFTER dividing out ``market_move``, the whole basket's
    median move. Without that, a currency event drowns the list: when flux loses
    a third of its value every single item reads as a spike, and the one item
    that actually did something unusual is buried among a hundred that merely
    got re-denominated. Deflating first asks the question worth asking - did
    this move beyond what the whole market did? The raw move is still reported.

    Supply and stack counts are NOT deflated. They are counts of listings and
    items, not amounts of flux, so a currency move does not touch them.
    """
    if len(series) < MIN_BASELINE_DAYS + 1:
        return None
    latest, history = series[-1], series[:-1]
    if latest["listings"] < MIN_SAMPLES:
        return None

    factor = 1.0 + (market_move or 0.0)
    deflated = latest["p50"] / factor if factor > 0 else latest["p50"]
    price_z, base, excess = _score(deflated, [d["p50"] for d in history])
    _, _, change = _score(latest["p50"], [d["p50"] for d in history])
    supply_z, _, supply_rel = _score(
        latest["listings"], [d["listings"] for d in history])
    stack_z, _, stack_rel = _score(
        latest["stack_med"], [d["stack_med"] for d in history])

    price_hit = _flagged(price_z, excess, MIN_PRICE_MOVE)
    supply_hit = _flagged(supply_z, supply_rel, MIN_COUNT_MOVE)
    stack_hit = _flagged(stack_z, stack_rel, MIN_COUNT_MOVE)
    if not (price_hit or supply_hit or stack_hit):
        return None

    # Severity reads only the dimensions that actually cleared both gates, so a
    # huge score on a trivially small move cannot inflate it.
    severity = _severity(*[z for z, hit in
                           ((price_z, price_hit), (supply_z, supply_hit),
                            (stack_z, stack_hit)) if hit])

    pattern, reading = _classify(
        price_hit and price_z > 0, price_hit and price_z < 0,
        supply_hit and supply_z > 0, supply_hit and supply_z < 0,
        stack_hit and stack_z > 0)

    return {
        "name": name,
        "pattern": pattern,
        "reading": reading,
        "severity": severity,
        "day": latest["bucket"],
        "price": round(latest["p50"], 2),
        "baseline": round(base, 2),
        # change = what it actually did. excess = what it did beyond the market.
        # During a currency event these diverge sharply, and `excess` is the one
        # that says something about this item rather than about flux.
        "change": round(change, 4) if change is not None else None,
        "excess": round(excess, 4) if excess is not None else None,
        "listings": latest["listings"],
        "stack_med": round(latest["stack_med"], 1),
        "stack_max": latest["stack_max"],
        "price_z": round(price_z, 2) if price_z is not None else None,
        "supply_z": round(supply_z, 2) if supply_z is not None else None,
        "stack_z": round(stack_z, 2) if stack_z is not None else None,
    }


def _breadth(by_item: dict[str, list[dict]]) -> dict:
    """Market-wide move for the latest day: how much of the tracked basket moved,
    and which way. This is the currency-side test - see the module docstring."""
    moves: list[float] = []
    for series in by_item.values():
        if len(series) < MIN_BASELINE_DAYS + 1:
            continue
        latest, history = series[-1], series[:-1]
        if latest["listings"] < MIN_SAMPLES:
            continue
        _, _, rel = _score(latest["p50"], [d["p50"] for d in history])
        if rel is not None:
            moves.append(rel)

    if not moves:
        return {"items": 0, "median_move": None, "share_up": None,
                "share_down": None, "verdict": "no_data", "reading": ""}

    n = len(moves)
    up = sum(1 for m in moves if m >= BREADTH_MIN_MOVE) / n
    down = sum(1 for m in moves if m <= -BREADTH_MIN_MOVE) / n

    if up >= BREADTH_SHARE:
        verdict, reading = "flux_weaker", (
            "Most of the market got more expensive at once. When unrelated items "
            "all rise together it is usually flux losing value rather than the "
            "items gaining it - which is what a flux duplication looks like from "
            "the outside.")
    elif down >= BREADTH_SHARE:
        verdict, reading = "flux_stronger", (
            "Most of the market got cheaper at once, which points at flux itself "
            "gaining value rather than every item independently falling.")
    else:
        verdict, reading = "normal", (
            "Items are moving independently, which is what ordinary trading "
            "looks like.")

    return {
        "items": n,
        "median_move": round(statistics.median(moves), 4),
        "share_up": round(up, 3),
        "share_down": round(down, 3),
        "verdict": verdict,
        "reading": reading,
    }


async def scan(days: int = 21) -> dict:
    """Full anomaly pass over the last ``days`` of listings.

    Empty (not an error) without Postgres, matching every other market read.
    """
    if not settings.postgres_enabled:
        return {"days": days, "generated_for": None, "market": None,
                "signals": [], "scanned_items": 0}

    from app.trove.market.service import _now
    floor = (_now() // DAY) * DAY - days * DAY
    rows = await pg_store.daily_series_all(created_at_floor=floor)
    by_item = _series_by_item(rows)

    # Breadth first: the per-item pass needs the market factor to divide out.
    market = _breadth(by_item)
    market_move = market["median_move"] or 0.0

    signals = [s for name, series in by_item.items()
               if (s := _scan_item(name, series, market_move)) is not None]
    # Loudest first, and a tie on severity goes to the bigger price dislocation.
    signals.sort(key=lambda s: (
        s["severity"] == "extreme",
        max(abs(s[k] or 0) for k in ("price_z", "supply_z", "stack_z")),
    ), reverse=True)

    latest_day = max((series[-1]["bucket"] for series in by_item.values()),
                     default=None)

    return {
        "days": days,
        "generated_for": latest_day,
        "market": market,
        "signals": signals,
        "scanned_items": len(by_item),
    }
