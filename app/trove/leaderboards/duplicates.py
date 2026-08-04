"""Duplicate-name detection for the leaderboards dataset.

The problem
-----------
Trove's leaderboard dump carries **no player id** - just ``rank;name;score`` -
and the store therefore keys identity by lower-cased name
(``player.name_lower``). That breaks in two distinct ways, both of which make one
profile show two people's numbers:

* **``same_name``** - the game's *own* dump lists the identical spelling **twice
  on the same board**, at two different ranks with two different scores. Both
  rows resolve to one ``player`` row, so every per-player read interleaves them:
  the history panel shows two tiles per board and the chart zig-zags between the
  two scores because both points share a timestamp. Observed live: 16 names in a
  single capture, the worst spanning 25 boards with a 357k↔6.7M spread.
* **``case``** - two spellings that differ only in case (``Robot`` / ``robot``)
  fold into one ``name_lower`` key, so two accounts are *merged* into a single
  player row. This one is invisible after ingest (the store keeps one casing), so
  it is captured at write time from the staging table - see
  ``pg_store.write_snapshot``.

What this module does NOT do
----------------------------
It never decides *which* row is "the real player". Nothing in the data supports
that: there is no id to join on. What it does is (a) reconstruct which row at
capture N continues which row at capture N-1, so each identity plots as its own
continuous line instead of one crossing sawtooth, and (b) record the affected
names so the tab can surface them. Hence "**Possible** duplicates", the series
labels are neutral (``#1``/``#2``, not "the real one"), and the verdict is
descriptive rather than a guess.

The splitter (``split_series``)
-------------------------------
Pure, deterministic, and the heart of the fix. Given one board's points for one
name, it walks anchors oldest-first and links each capture's rows to the previous
capture's by **score continuity**: each open series predicts its next value from
its own recent rate (``_velocity``), and the rows are assigned to series so that
the total miss against those predictions is smallest, with an outright drop
penalised (``_DROP_PENALTY``) because a lifetime score climbs. Rows with no
plausible predecessor open a new series; series with no successor simply end (a
gap). With k rows per anchor (k is 2 in every observed case) every assignment is
brute-forced, so the result is optimal rather than greedy.

Two simpler rules were tried and rejected:

* **Sort by score.** A frozen leftover row sits *above* the live one on some
  boards (LateCom's board 11: ghost 8.1M vs live 2.6M), so the live line
  eventually overtakes the ghost and a score-ordered split swaps the two lines
  mid-chart.
* **Minimise raw score movement.** Degenerate: when every row rose, total
  movement is ``sum(scores) - sum(lasts)`` whatever the pairing, so *every*
  assignment ties and the tie-break silently decides. Predicting each line
  forward by its own rate is what breaks that tie correctly.

Drivers (same shape as ``renames``)
-----------------------------------
* **Live** (``detect_latest``) - the warmer re-scans the newest capture after each
  ingest, so the record tracks the current state.
* **Backfill** (``backfill``) - a dev-portal action walks the whole archive to
  date each group's ``first_anchor`` (single-flight + Redis progress).

Both write ``player_duplicate`` rows (idempotent on ``name_lower``).
"""
from __future__ import annotations

import itertools
import json
import logging
import time
from dataclasses import dataclass, field

from app.trove.leaderboards import pg_store
from app.trove.leaderboards import service as lb_service

logger = logging.getLogger(__name__)

METHOD_VERSION = 1

# A score DROP costs this much more than an equal-sized rise when matching a row
# to its predecessor. Lifetime scores only ever climb, so a candidate pairing
# that implies a fall is far more likely to be the wrong pairing - but it is not
# forbidden outright, because a daily/weekly board legitimately drops to zero at
# the 11:00-UTC reset (and there the whole board falls together, so the relative
# ordering the matcher relies on still holds).
_DROP_PENALTY = 8.0

# Above this many rows/series on one board at one anchor, fall back to greedy
# matching - the permutation count stops being free. Real data tops out at 2.
_BRUTE_FORCE_MAX = 5

# How many recent points a series' expected next value is extrapolated from. Short
# enough to track a player who starts or stops grinding, long enough that a single
# flat hour doesn't read as "this line has stalled".
_VELOCITY_WINDOW = 4

# In-process guard so the 30-min warm cycle doesn't re-scan an anchor it already
# handled (the result is idempotent, so it would just be wasted work). Reset on a
# full leaderboards reset.
_last_scanned_anchor: int | None = None

# Verdicts. Descriptive, never a claim about which identity is "real".
VERDICT_ONE_LIVE = "one_live"       # exactly one series moved; the rest are frozen
VERDICT_MULTI_LIVE = "multi_live"   # 2+ series moved - two active identities
VERDICT_ALL_IDLE = "all_idle"       # nothing moved in the window - undecidable
VERDICT_CASE_ONLY = "case_only"     # spellings differ only by case (merged rows)


# ── config ───────────────────────────────────────────────────────────────────

@dataclass
class _Cfg:
    lookback_days: int
    min_boards: int
    frozen_min_captures: int
    max_names: int
    board_names: dict[int, str] = field(default_factory=dict)


async def _load_config(anchor: int | None) -> _Cfg:
    """Read the runtime-config knobs, resolving board display names from
    ``anchor`` (None → names left unresolved, rendered as ``Board #uuid``)."""
    from app.admin import runtime_config

    cfg = _Cfg(
        lookback_days=int(await runtime_config.get_setting("duplicates_lookback_days")),
        min_boards=int(await runtime_config.get_setting("duplicates_min_boards")),
        frozen_min_captures=int(
            await runtime_config.get_setting("duplicates_frozen_min_captures")
        ),
        max_names=int(await runtime_config.get_setting("duplicates_max_names")),
    )
    if anchor is not None:
        try:
            boards = await lb_service.list_boards_at(anchor)
            cfg.board_names = {
                b["uuid"]: (b.get("name") or b.get("name_id") or str(b["uuid"]))
                for b in boards
            }
        except Exception:  # noqa: BLE001 - names are cosmetic, never fatal
            logger.warning("duplicates: board names unresolved at %d", anchor,
                           exc_info=True)
    return cfg


# ── the splitter (pure) ──────────────────────────────────────────────────────

def _velocity(points: list[dict]) -> float:
    """A series' recent score change per capture, floored at 0.

    This is what makes the matcher work. Costing a pairing by raw score movement
    is DEGENERATE: when every row rose, the total rise is ``sum(scores) -
    sum(lasts)`` no matter how the rows are paired up, so every assignment ties
    and the tie-break decides - which is how a live line gets handed to a frozen
    one the moment it overtakes it. Predicting each series forward by its own
    recent rate breaks the tie on the right side: a frozen line expects 0 and a
    grinding line expects its usual gain.

    Floored at 0 because a fall is never the *expectation* - at a daily reset
    every line drops together, and predicting the drop forward would make the
    next (recovering) capture look wrong for all of them equally."""
    if len(points) < 2:
        return 0.0
    tail = points[-_VELOCITY_WINDOW:]
    span = len(tail) - 1
    if span <= 0:
        return 0.0
    return max(0.0, (float(tail[-1]["score"]) - float(tail[0]["score"])) / span)


def _pair_cost(score: float, last: float, predicted: float) -> float:
    """Cost of continuing the series that last read ``last`` (and is expected to
    read ``predicted``) with a row scoring ``score``: how far the row misses the
    prediction, plus a heavy penalty if it means the score went DOWN, which a
    lifetime board never does outside a reset."""
    cost = abs(score - predicted)
    if score < last:
        cost += _DROP_PENALTY * (last - score)
    return cost


def _assign(
    scores: list[float], lasts: list[float], predicted: list[float],
) -> list[int | None]:
    """Slot index for each row (``None`` = no plausible predecessor, open a new
    series). Optimal by brute force while both sides are small; greedy beyond
    ``_BRUTE_FORCE_MAX``, which real data never reaches (k is 2)."""
    n, m = len(scores), len(lasts)
    if n == 1 and m == 1:
        return [0]

    if max(n, m) <= _BRUTE_FORCE_MAX:
        best_cost = float("inf")
        out: list[int | None] = [None] * n
        if m <= n:
            # Every series claims a distinct row; leftover rows start new series.
            best: tuple[int, ...] | None = None
            for rows_for_series in itertools.permutations(range(n), m):
                cost = sum(
                    _pair_cost(scores[r], lasts[s], predicted[s])
                    for s, r in enumerate(rows_for_series)
                )
                if cost < best_cost:
                    best_cost, best = cost, rows_for_series
            if best is not None:
                for s, r in enumerate(best):
                    out[r] = s
            return out
        # More series than rows: every row claims a distinct series; the series
        # left over simply have no point at this anchor (a gap).
        best_rows: tuple[int, ...] | None = None
        for series_for_rows in itertools.permutations(range(m), n):
            cost = sum(
                _pair_cost(scores[i], lasts[s], predicted[s])
                for i, s in enumerate(series_for_rows)
            )
            if cost < best_cost:
                best_cost, best_rows = cost, series_for_rows
        return list(best_rows) if best_rows is not None else out

    # Greedy fallback: each row takes its cheapest unused series.
    used: set[int] = set()
    greedy: list[int | None] = []
    for score in scores:
        pick, pick_cost = None, float("inf")
        for i in range(m):
            if i in used:
                continue
            cost = _pair_cost(score, lasts[i], predicted[i])
            if cost < pick_cost:
                pick, pick_cost = i, cost
        if pick is not None:
            used.add(pick)
        greedy.append(pick)
    return greedy


def split_series(points: list[dict]) -> list[dict]:
    """Split ONE board's points for ONE name into continuous per-identity series.

    ``points`` is ``[{"created_at", "rank", "score"}, ...]`` in any order (the
    store returns a flat row set). Returns ``[{"slot": int, "points": [...]}]``
    with each series' points anchor-ascending and ``slot`` stable across the whole
    input - slot 0 is the series that starts earliest (ties broken by higher
    score), so labels don't shuffle between requests.

    The common case (one row per anchor) returns a single series unchanged, which
    is what every non-duplicated player hits."""
    if not points:
        return []
    by_anchor: dict[int, list[dict]] = {}
    for p in points:
        by_anchor.setdefault(int(p["created_at"]), []).append(p)
    anchors = sorted(by_anchor)

    # Fast path: never more than one row at any anchor → nothing to split.
    if all(len(v) == 1 for v in by_anchor.values()):
        return [{
            "slot": 0,
            "points": [by_anchor[a][0] for a in anchors],
        }]

    series: list[list[dict]] = []   # points per slot, anchor-ascending
    for anchor in anchors:
        # Rows sorted high→low so equal-cost pairings resolve the same way every
        # time (the assignment itself is what decides slots, not this ordering).
        rows = sorted(by_anchor[anchor],
                      key=lambda p: (-float(p["score"]), int(p["rank"])))
        scores = [float(r["score"]) for r in rows]
        if not series:
            series = [[row] for row in rows]
            continue
        lasts = [float(s[-1]["score"]) for s in series]
        predicted = [last + _velocity(s) for last, s in zip(lasts, series, strict=True)]
        for row, slot in zip(rows, _assign(scores, lasts, predicted), strict=True):
            if slot is None:            # no plausible predecessor → new identity
                series.append([row])
            else:
                series[slot].append(row)

    # Stable slot numbering: earliest first appearance wins, then higher score.
    order = sorted(
        range(len(series)),
        key=lambda i: (series[i][0]["created_at"], -float(series[i][0]["score"])),
    )
    return [
        {"slot": new_slot, "points": series[old]}
        for new_slot, old in enumerate(order)
    ]


def summarise_series(split: list[dict], *, frozen_min_captures: int) -> list[dict]:
    """Describe each series from ``split_series``: its score span, whether it
    MOVED in the window, and how many captures it covers.

    ``frozen`` means the score never changed across the window *and* the series
    was seen at least ``frozen_min_captures`` times - below that there isn't
    enough evidence to call it stalled (a series with 2 points that happens to be
    flat is just an idle hour)."""
    out = []
    for s in split:
        pts = s["points"]
        scores = [float(p["score"]) for p in pts]
        moved = len(scores) > 1 and max(scores) != min(scores)
        out.append({
            "slot": s["slot"],
            "captures": len(pts),
            "first_anchor": int(pts[0]["created_at"]),
            "last_anchor": int(pts[-1]["created_at"]),
            "first_score": scores[0],
            "last_score": scores[-1],
            "last_rank": int(pts[-1]["rank"]),
            "moved": moved,
            "frozen": (not moved) and len(pts) >= frozen_min_captures,
        })
    return out


def _verdict(board_reports: list[dict]) -> str:
    """Fold the per-board series reports into one descriptive verdict for a name.

    A series is counted as live if it moved on ANY board (a player grinding one
    board is still one live identity), so the verdict answers "how many distinct
    identities showed activity here"."""
    live_slots: set[int] = set()
    seen_slots: set[int] = set()
    for board in board_reports:
        for s in board["series"]:
            seen_slots.add(s["slot"])
            if s["moved"]:
                live_slots.add(s["slot"])
    if not live_slots:
        return VERDICT_ALL_IDLE
    if len(live_slots) >= 2:
        return VERDICT_MULTI_LIVE
    return VERDICT_ONE_LIVE if len(seen_slots) > len(live_slots) else VERDICT_ALL_IDLE


def _summary_text(name: str, boards: int, verdict: str, series_count: int) -> str:
    plural = "s" if boards != 1 else ""
    if verdict == VERDICT_CASE_ONLY:
        return (
            f"Two or more spellings of “{name}” differ only in capitalisation, so "
            f"Trove's leaderboards treat them as one name here and their scores are "
            f"merged into a single profile."
        )
    head = (
        f"Trove's own capture lists “{name}” {series_count} times on the same "
        f"board across {boards} board{plural}"
    )
    if verdict == VERDICT_MULTI_LIVE:
        return (head + ", and more than one of those score lines is still moving - "
                "they look like separate active identities sharing a name.")
    if verdict == VERDICT_ONE_LIVE:
        return (head + ", and only one of those score lines is still moving; the "
                "other has not changed all window, which is what a leftover "
                "leaderboard row looks like.")
    return (head + ". None of the score lines moved in this window, so there is "
            "no way to tell them apart from activity alone.")


# ── detection ────────────────────────────────────────────────────────────────

async def _build_record(
    name: str, groups: list[dict], cfg: _Cfg, anchor: int, now: int,
) -> dict:
    """Assemble one ``player_duplicate`` row for ``name``. ``groups`` is the
    per-board duplication found at ``anchor``; the window rows are re-split so the
    evidence can say which line is moving and which has stalled."""
    window_start = anchor - cfg.lookback_days * 86400
    try:
        rows = await pg_store.player_rows_window(name, window_start)
    except Exception:  # noqa: BLE001 - evidence is best-effort, the row still lands
        logger.warning("duplicates: window rows failed for %r", name, exc_info=True)
        rows = []

    by_board: dict[int, list[dict]] = {}
    for r in rows:
        by_board.setdefault(int(r["leaderboard"]), []).append(r)

    dup_uuids = {int(g["board_uuid"]) for g in groups}
    board_reports: list[dict] = []
    for uuid in sorted(dup_uuids):
        pts = by_board.get(uuid) or []
        split = split_series(pts) if pts else []
        board_reports.append({
            "uuid": uuid,
            "name": cfg.board_names.get(uuid, f"Board #{uuid}"),
            "occurrences": next(
                (int(g["occurrences"]) for g in groups
                 if int(g["board_uuid"]) == uuid), 2),
            "series": summarise_series(
                split, frozen_min_captures=cfg.frozen_min_captures),
        })

    series_count = max((int(g["occurrences"]) for g in groups), default=2)
    verdict = _verdict(board_reports)
    first_anchor = min(
        (s["first_anchor"] for b in board_reports for s in b["series"]),
        default=anchor,
    )
    return {
        "name": name,
        "kind": "same_name",
        "verdict": verdict,
        "boards": len(dup_uuids),
        "max_occurrences": series_count,
        "spellings": [],
        "first_anchor": first_anchor,
        "last_anchor": anchor,
        "method_version": METHOD_VERSION,
        "updated_at": now,
        "evidence": {
            "lookback_days": cfg.lookback_days,
            "boards": board_reports[:40],
            "summary": _summary_text(name, len(dup_uuids), verdict, series_count),
        },
    }


async def detect_latest(*, force: bool = False) -> dict:
    """Find every name duplicated within a single board at the newest capture and
    persist a record per name.

    Called by the leaderboards warmer after each ingest (non-fatal). No-op when
    the flag is off or there's no capture yet; an in-process guard skips an anchor
    already scanned by this process unless ``force``."""
    global _last_scanned_anchor
    from app.core import features as feature_flags

    if not await feature_flags.is_enabled(feature_flags.DUPLICATES_FLAG):
        return {"status": "disabled", "detected": 0}

    ts = await lb_service.list_timestamps(limit=1, include_archive=False)
    if not ts:
        return {"status": "no_captures", "detected": 0}
    anchor = ts[0]
    if not force and _last_scanned_anchor == anchor:
        return {"status": "already_scanned", "anchor": anchor, "detected": 0}

    cfg = await _load_config(anchor)
    groups = await pg_store.duplicate_groups_at(anchor)
    by_name: dict[str, list[dict]] = {}
    for g in groups:
        by_name.setdefault(g["name"], []).append(g)

    # Widest blast radius first, so a max_names cap keeps the worst offenders.
    ordered = sorted(by_name.items(), key=lambda kv: -len(kv[1]))
    ordered = [(n, g) for n, g in ordered if len(g) >= cfg.min_boards]

    now = int(time.time())
    records = [
        await _build_record(name, g, cfg, anchor, now)
        for name, g in ordered[:cfg.max_names]
    ]
    if records:
        await pg_store.upsert_duplicates(records)
    # Names that no longer duplicate at this anchor stop being "current" but keep
    # their row (the history is the point) - the tab flags them via last_anchor.
    _last_scanned_anchor = anchor
    logger.info("duplicates: live pass at %d found %d duplicated name(s)",
                anchor, len(records))
    return {"status": "ok", "anchor": anchor, "detected": len(records),
            "candidates": len(by_name)}


# ── backfill driver (dev portal) ─────────────────────────────────────────────

_STATUS_KEY = "lb:duplicates:status"
_RUNNING_KEY = "lb:duplicates:running"
_local_status: dict = {"running": False}


async def _set_status(status: dict) -> None:
    global _local_status
    _local_status = status
    from app.core.redis import get_redis
    r = get_redis()
    if r is not None:
        try:
            await r.set(_STATUS_KEY, json.dumps(status), ex=86400)
        except Exception:
            pass


async def get_status() -> dict:
    """Latest backfill progress (shared via Redis across workers)."""
    from app.core.redis import get_redis
    r = get_redis()
    if r is not None:
        try:
            raw = await r.get(_STATUS_KEY)
            if raw:
                return json.loads(raw)
        except Exception:
            pass
    return dict(_local_status)


async def backfill(*, clear_first: bool = False) -> None:
    """Walk EVERY capture in the archive, oldest-first, recording duplicated names.

    The live pass only ever sees the newest capture, so it can't tell you when a
    duplication started. This does: each anchor's groups are folded into the
    record, keeping the earliest ``first_anchor`` and the latest ``last_anchor``.
    Single-flight guarded with live Redis progress, mirroring the rename
    backfill."""
    from app.core.redis import get_redis

    r = get_redis()
    if r is not None:
        got = await r.set(_RUNNING_KEY, "1", nx=True, ex=900)
        if not got:
            logger.info("duplicates backfill already running - skipping duplicate")
            return

    anchors = await pg_store.all_anchors_asc()
    cfg = await _load_config(anchors[-1] if anchors else None)
    now = int(time.time())
    status = {
        "running": True, "total": len(anchors), "done": 0, "detected": 0,
        "started_at": now, "finished_at": None, "last_anchor": None,
        "clear_first": clear_first,
    }
    await _set_status(status)
    try:
        if clear_first:
            status["cleared"] = await pg_store.delete_all_duplicates()
            await _set_status(status)

        seen: set[str] = set()
        for anchor in anchors:
            try:
                groups = await pg_store.duplicate_groups_at(anchor)
            except Exception:
                logger.warning("duplicates backfill: anchor %d failed", anchor,
                               exc_info=True)
                status["done"] += 1
                continue
            by_name: dict[str, list[dict]] = {}
            for g in groups:
                by_name.setdefault(g["name"], []).append(g)
            batch = []
            for name, g in by_name.items():
                if len(g) < cfg.min_boards:
                    continue
                rec = await _build_record(name, g, cfg, anchor, now)
                # Oldest anchor wins for first_anchor; upsert_duplicates keeps the
                # minimum, so just record it and let the store fold.
                batch.append(rec)
                seen.add(name.lower())
            if batch:
                await pg_store.upsert_duplicates(batch)
            status["detected"] = len(seen)
            status["done"] += 1
            status["last_anchor"] = anchor
            if status["done"] % 25 == 0:
                await _set_status(status)
                if r is not None:
                    try:
                        await r.expire(_RUNNING_KEY, 900)  # heartbeat
                    except Exception:
                        pass
    finally:
        status["running"] = False
        status["phase"] = "done"
        status["finished_at"] = int(time.time())
        await _set_status(status)
        if r is not None:
            try:
                await r.delete(_RUNNING_KEY)
            except Exception:
                pass
        logger.info("duplicates backfill done: %d name(s) across %d capture(s)",
                    status["detected"], status["done"])


# ── serving ──────────────────────────────────────────────────────────────────

async def serve_list(*, limit: int = 50, offset: int = 0,
                     kind: str | None = None) -> dict:
    """Recorded duplicate-name groups, widest blast radius first, for the tab/API."""
    from app.core import features as feature_flags

    if not await feature_flags.is_enabled(feature_flags.DUPLICATES_FLAG):
        return {"enabled": False, "duplicates": [], "total": 0, "current": 0,
                "limit": limit, "offset": offset, "method_version": METHOD_VERSION}
    rows, total = await pg_store.list_duplicates(
        limit=limit, offset=offset, kind=kind)
    latest = await pg_store.latest_duplicate_anchor()
    current = sum(1 for r in rows if latest and r["last_anchor"] == latest)
    return {
        "enabled": True, "duplicates": rows, "total": total, "current": current,
        "latest_anchor": latest, "limit": limit, "offset": offset,
        "method_version": METHOD_VERSION,
    }


async def for_name(name: str) -> dict:
    """The duplicate record for one name (``found=False`` when clean) - drives the
    warning banner in the player panel and on ``/player/<name>``."""
    from app.core import features as feature_flags

    if not await feature_flags.is_enabled(feature_flags.DUPLICATES_FLAG):
        return {"query": name, "found": False, "enabled": False}
    row = await pg_store.duplicate_for_name(name)
    if row is None:
        return {"query": name, "found": False, "enabled": True}
    return {"query": name, "found": True, "enabled": True, **row}


def reset() -> None:
    """Drop the in-process scan guard (the persisted rows are wiped separately in
    ``pg_store.reset_all``). Called on a full leaderboards reset."""
    global _last_scanned_anchor
    _last_scanned_anchor = None
