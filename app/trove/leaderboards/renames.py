"""Player-rename detection for the leaderboards dataset.

Trove leaderboards carry **no stable player id** - the store keys players by
lower-cased name (``player.name_lower``), so renaming a character mints a brand-
new ``player`` row with no link to the old one. This module reconstructs renames
from *behaviour*.

The signal
----------
A player appears on many boards at once. Restricted to **lifetime boards**
(``reset_kind`` ``default``/``none`` - Trove Mastery, Geode Mastery, Power Rank,
lifetime stat boards), a player's ``{board -> score}`` vector is a high-entropy,
near-unique **fingerprint** - and, crucially, one that a rename carries over
*unchanged* (renaming doesn't reset your mastery) and that never resets between
captures. So when a name **vanishes** between two adjacent captures while a new
name **appears** carrying the same lifetime fingerprint, that's a rename.

Two guards make it reliable and reset-proof:

* **Adjacency gate.** Only capture pairs within ``renames_max_gap_seconds``
  (default 1.5h - captures are hourly) are compared. Across a wider gap the
  fingerprint drifts (the player keeps grinding) and the board population churns,
  both of which corrupt the match, so those pairs are skipped entirely.
* **Lifetime-only fingerprint.** Resetting (daily/weekly) boards are never used.
  Their scores aren't a stable identity and, at an 11:00-UTC reset, the whole
  board churns to zero - exactly the false "everyone disappeared/reappeared" that
  would wreck a naive differ. Building only on non-resetting boards makes the
  whole detection reset-immune *by construction*, including for a pair that
  straddles a reset boundary.

The matcher (conservative)
--------------------------
Within a pair, ``disappeared`` = names present at A, gone at B; ``appeared`` =
the reverse. Each disappeared X is matched to the appeared Y whose lifetime
fingerprint overlaps X's on the SAME boards with near-equal scores (allowing only
a small positive drift = an hour's grind; a drop or a big jump disqualifies a
board). A rename is emitted only when:

* the overlap is at least ``renames_min_boards`` lifetime boards,
* X and Y are **mutual** best matches (X's best candidate is Y *and* Y's best is
  X - no double-assignment), and
* the match is **unambiguous** (X has no runner-up candidate of equal overlap).

Anything ambiguous is dropped, not guessed. Confidence folds board count, score
tightness, exclusivity (runner-up margin) and cell rarity, and is surfaced with
its sub-terms (same transparency contract as cheater detection).

Drivers
-------
* **Live** (``detect_latest``) - the warmer runs the differ on just the newest
  adjacent pair after each ingest, so the record stays current automatically.
* **Backfill** (``backfill``) - a dev-portal action walks the *whole* archive
  backwards over every ≤gap adjacent pair, idempotently populating the record
  (single-flight + live Redis progress, mirroring the ingest backlog runner).

Both write ``player_rename`` rows (idempotent on ``(from,to,to_anchor)``); the
tab reads them straight back, most-recent-first, and chains ``to -> from`` edges
into a full rename history per identity.
"""
from __future__ import annotations

import bisect
import logging
import time
from dataclasses import dataclass, field

from app.trove.leaderboards import pg_store
from app.trove.leaderboards import service as lb_service
from app.trove.leaderboards.models import is_lifetime_kind

logger = logging.getLogger(__name__)

METHOD_VERSION = 1

# Confidence is capped below 1.0: even a perfect multi-board carry-over could, in
# principle, be two unrelated accounts that happen to share several lifetime
# scores - vanishingly unlikely, but never certain.
_CONFIDENCE_CEILING = 0.97

# In-process guard so the 30-min warm cycle doesn't re-scan the same latest pair
# (which produces the same idempotent result) every iteration until a new capture
# lands. Reset on a full leaderboards reset.
_last_scanned_pair: tuple[int, int] | None = None


# ── config ───────────────────────────────────────────────────────────────────

@dataclass
class _Cfg:
    max_gap: int
    min_boards: int
    drift: float
    min_score: float
    min_conf: float
    excluded: set[int] = field(default_factory=set)
    lifetime_uuids: list[int] = field(default_factory=list)
    board_names: dict[int, str] = field(default_factory=dict)


def _parse_excluded(csv: str) -> set[int]:
    out: set[int] = set()
    for tok in (csv or "").split(","):
        s = tok.strip()
        if not s:
            continue
        try:
            out.add(int(s))
        except ValueError:
            logger.warning("renames: ignoring non-numeric excluded-board token %r", s)
    return out


async def _load_config(anchor: int | None) -> _Cfg:
    """Read the runtime-config knobs and resolve the lifetime-board set from
    ``anchor`` (the capture whose boards define the fingerprint). ``anchor`` None
    → no boards resolved yet (empty lifetime set)."""
    from app.admin import runtime_config

    excluded = _parse_excluded(
        str(await runtime_config.get_setting("renames_excluded_board_uuids"))
    )
    cfg = _Cfg(
        max_gap=int(await runtime_config.get_setting("renames_max_gap_seconds")),
        min_boards=int(await runtime_config.get_setting("renames_min_boards")),
        drift=float(await runtime_config.get_setting("renames_score_drift_pct")),
        min_score=float(await runtime_config.get_setting("renames_min_score")),
        min_conf=float(await runtime_config.get_setting("renames_min_confidence")),
        excluded=excluded,
    )
    if anchor is not None:
        await _resolve_lifetime_boards(cfg, anchor)
    return cfg


async def _resolve_lifetime_boards(cfg: _Cfg, anchor: int) -> None:
    """Populate ``cfg.lifetime_uuids`` + ``cfg.board_names`` from the boards present
    at ``anchor`` - only non-resetting player boards, minus the admin exclusions."""
    boards = await lb_service.list_boards_at(anchor)
    uuids: list[int] = []
    names: dict[int, str] = {}
    for b in boards:
        uuid = b["uuid"]
        if uuid in cfg.excluded:
            continue
        if not b.get("player_board", True):
            continue
        if not is_lifetime_kind(b.get("reset_kind", "default")):
            continue
        uuids.append(uuid)
        names[uuid] = b.get("name") or b.get("name_id") or str(uuid)
    cfg.lifetime_uuids = uuids
    cfg.board_names = names


# ── fingerprints ─────────────────────────────────────────────────────────────

async def _fingerprints(
    anchor: int, uuids: list[int], min_score: float,
) -> dict[str, dict]:
    """``{name_lower: {"display": name, "boards": {uuid: score}}}`` at ``anchor``
    over the given lifetime boards, keeping only entries scoring ≥ ``min_score``
    (tiny/round scores aren't distinctive enough to fingerprint on)."""
    if not uuids:
        return {}
    maps = await pg_store.anchor_maps(anchor, uuids)  # {uuid: {display_name: score}}
    out: dict[str, dict] = {}
    for uuid, scores in maps.items():
        for name, score in scores.items():
            if score is None or score < min_score:
                continue
            key = name.strip().lower()
            fp = out.get(key)
            if fp is None:
                fp = out[key] = {"display": name, "boards": {}}
            fp["boards"][uuid] = score
    return out


# ── matcher ──────────────────────────────────────────────────────────────────

def _match(
    fp_a: dict[str, dict], fp_b: dict[str, dict],
    from_anchor: int, to_anchor: int, cfg: _Cfg, now: int,
) -> list[dict]:
    """Bipartite match of disappeared (A-only) → appeared (B-only) names by
    lifetime fingerprint. Returns rename event dicts (conservative: mutual,
    unambiguous, ≥ ``min_boards`` boards, ≥ ``min_conf`` confidence)."""
    min_boards = max(1, cfg.min_boards)
    # Only names with enough qualifying boards to POSSIBLY reach the overlap floor.
    disappeared = {
        k: v for k, v in fp_a.items()
        if k not in fp_b and len(v["boards"]) >= min_boards
    }
    appeared = {
        k: v for k, v in fp_b.items()
        if k not in fp_a and len(v["boards"]) >= min_boards
    }
    if not disappeared or not appeared:
        return []

    # Per-board sorted score index over the appeared set, for range lookups.
    # {uuid: (sorted_scores[list], names[list] aligned)}.
    board_index: dict[int, tuple[list[float], list[str]]] = {}
    tmp: dict[int, list[tuple[float, str]]] = {}
    for name, fp in appeared.items():
        for uuid, score in fp["boards"].items():
            tmp.setdefault(uuid, []).append((score, name))
    for uuid, pairs in tmp.items():
        pairs.sort(key=lambda t: t[0])
        board_index[uuid] = ([p[0] for p in pairs], [p[1] for p in pairs])

    def _candidates_on(uuid: int, s_from: float) -> list[str]:
        """Appeared names on ``uuid`` scoring in [s_from, s_from*(1+drift)]."""
        idx = board_index.get(uuid)
        if idx is None:
            return []
        scores, names = idx
        lo = s_from * (1.0 - 1e-9)
        hi = s_from * (1.0 + cfg.drift)
        i = bisect.bisect_left(scores, lo)
        j = bisect.bisect_right(scores, hi)
        return names[i:j]

    # For each disappeared X, gather appeared candidates and the boards they match
    # on. matches: dx -> {ay: [(uuid, drift_frac, cell_count)]}
    matches: dict[str, dict[str, list[tuple[int, float, int]]]] = {}
    for dx, xfp in disappeared.items():
        per_cand: dict[str, list[tuple[int, float, int]]] = {}
        for uuid, s_from in xfp["boards"].items():
            cands = _candidates_on(uuid, s_from)
            cell_count = len(cands)  # how crowded this (board, score) cell is
            for ay in cands:
                s_to = appeared[ay]["boards"][uuid]
                drift_frac = (s_to - s_from) / s_from if s_from > 0 else 0.0
                per_cand.setdefault(ay, []).append((uuid, drift_frac, cell_count))
        # keep only candidates meeting the board-overlap floor
        per_cand = {ay: m for ay, m in per_cand.items() if len(m) >= min_boards}
        if per_cand:
            matches[dx] = per_cand

    if not matches:
        return []

    def _rank(m: list[tuple[int, float, int]]) -> tuple:
        # more matched boards, then tighter total drift.
        return (len(m), -sum(d for _, d, _ in m))

    # Best (and runner-up overlap) per disappeared X.
    best_x: dict[str, tuple[str, tuple]] = {}
    runnerup_overlap: dict[str, int] = {}
    for dx, cands in matches.items():
        ranked = sorted(cands.items(), key=lambda kv: _rank(kv[1]), reverse=True)
        best_ay, best_m = ranked[0]
        best_x[dx] = (best_ay, _rank(best_m))
        runnerup_overlap[dx] = len(ranked[1][1]) if len(ranked) > 1 else 0

    # Best disappeared per appeared Y (for the mutual-best check).
    best_y: dict[str, tuple[str, tuple]] = {}
    for dx, cands in matches.items():
        for ay, m in cands.items():
            r = _rank(m)
            cur = best_y.get(ay)
            if cur is None or r > cur[1]:
                best_y[ay] = (dx, r)

    events: list[dict] = []
    for dx, (ay, _r) in best_x.items():
        # mutual-best
        if best_y.get(ay, (None,))[0] != dx:
            continue
        best_m = matches[dx][ay]
        overlap = len(best_m)
        # unambiguous: no runner-up candidate of EQUAL overlap
        if runnerup_overlap[dx] >= overlap:
            continue
        conf, terms = _confidence(best_m, overlap, runnerup_overlap[dx], cfg)
        if conf < cfg.min_conf:
            continue
        xfp, yfp = disappeared[dx], appeared[ay]
        boards_ev = [
            {
                "uuid": uuid,
                "name": cfg.board_names.get(uuid, str(uuid)),
                "score_from": round(xfp["boards"][uuid], 2),
                "score_to": round(yfp["boards"][uuid], 2),
                "drift_pct": round(drift * 100, 3),
            }
            for uuid, drift, _cell in sorted(best_m, key=lambda t: t[0])
        ]
        events.append({
            "from_name": xfp["display"],
            "to_name": yfp["display"],
            "from_anchor": from_anchor,
            "to_anchor": to_anchor,
            "confidence": conf,
            "matched_boards": overlap,
            "method_version": METHOD_VERSION,
            "created_at": now,
            "evidence": {
                "gap_seconds": to_anchor - from_anchor,
                "boards": boards_ev,
                "terms": terms,
                "summary": _summary(xfp["display"], yfp["display"], overlap, terms),
            },
        })
    return events


def _confidence(
    matched: list[tuple[int, float, int]], overlap: int, runnerup: int, cfg: _Cfg,
) -> tuple[float, dict]:
    """Blend four independent signals into a rename confidence in [0, ceiling]:

    * **board_term** ``1 - 0.5^overlap`` - more matched lifetime boards is
      exponentially harder to fake (2→0.75, 3→0.875, 4→0.94).
    * **tightness** - how far below the drift tolerance the score carries sit
      (exact carry-over → 1.0). Fingerprints that match to the digit are the
      strongest tell.
    * **exclusivity** - runner-up margin ``(overlap - runnerup) / overlap``.
      A single clean candidate → 1.0.
    * **rarity** - fraction of matched boards whose (board, score) cell held
      exactly one appeared candidate (an uncrowded, distinctive score).

    Sub-terms are returned so the tab can show exactly how a confidence was
    reached (same contract as cheater evidence)."""
    drifts = [d for _, d, _ in matched]
    cells = [c for _, _, c in matched]
    if cfg.drift > 0:
        tightness = sum(max(0.0, 1.0 - (d / cfg.drift)) for d in drifts) / len(drifts)
    else:
        tightness = 1.0 if all(d <= 0 for d in drifts) else 0.0
    board_term = 1.0 - 0.5 ** overlap
    exclusivity = (overlap - runnerup) / overlap if overlap > 0 else 0.0
    rarity = sum(1 for c in cells if c <= 1) / len(cells)
    conf = (
        board_term
        * (0.55 + 0.45 * tightness)
        * (0.60 + 0.40 * exclusivity)
        * (0.70 + 0.30 * rarity)
    )
    conf = min(_CONFIDENCE_CEILING, round(conf, 4))
    terms = {
        "matched_boards": overlap,
        "runnerup_overlap": runnerup,
        "board_term": round(board_term, 4),
        "tightness": round(tightness, 4),
        "exclusivity": round(exclusivity, 4),
        "rarity": round(rarity, 4),
        "mean_drift_pct": round(sum(drifts) / len(drifts) * 100, 3),
        "confidence": conf,
    }
    return conf, terms


def _summary(from_name: str, to_name: str, overlap: int, terms: dict) -> str:
    exact = terms["tightness"] >= 0.999
    carry = "carried over unchanged" if exact else "carried over near-identically"
    return (
        f"“{from_name}” vanished and “{to_name}” appeared in the "
        f"same capture with the same lifetime score fingerprint {carry} across "
        f"{overlap} board{'s' if overlap != 1 else ''} "
        f"(Trove/Geode Mastery, Power Rank - boards that never reset and survive a "
        f"rename). Mutual, unambiguous best match."
    )


# ── pair processing ──────────────────────────────────────────────────────────

async def _process_pair(a: int, b: int, cfg: _Cfg, now: int,
                        fp_a: dict | None = None,
                        fp_b: dict | None = None) -> tuple[list[dict], dict, dict]:
    """Run the differ on one adjacent pair. Returns ``(events, fp_a, fp_b)`` so a
    caller walking a sequence can reuse ``fp_b`` as the next pair's ``fp_a``.
    Returns no events (but still the fingerprints) when the gap is out of range."""
    if not (0 < b - a <= cfg.max_gap):
        return [], (fp_a or {}), (fp_b or {})
    if fp_a is None:
        fp_a = await _fingerprints(a, cfg.lifetime_uuids, cfg.min_score)
    if fp_b is None:
        fp_b = await _fingerprints(b, cfg.lifetime_uuids, cfg.min_score)
    events = _match(fp_a, fp_b, a, b, cfg, now)
    return events, fp_a, fp_b


# ── live driver (warmer) ─────────────────────────────────────────────────────

async def detect_latest(*, force: bool = False) -> dict:
    """Detect renames across the two most recent captures and persist them.

    Called by the leaderboards warmer after each ingest (non-fatal) so the record
    stays current. No-op (returns a status payload) when the flag is off, there
    aren't two captures yet, or the newest pair is beyond the adjacency gap. An
    in-process guard skips re-scanning a pair already handled this process unless
    ``force``."""
    global _last_scanned_pair
    from app.core import features as feature_flags

    if not await feature_flags.is_enabled(feature_flags.RENAMES_FLAG):
        return {"status": "disabled", "detected": 0}

    ts = await lb_service.list_timestamps(limit=2, include_archive=False)
    if len(ts) < 2:
        return {"status": "insufficient_history", "detected": 0}
    b, a = ts[0], ts[1]
    if not force and _last_scanned_pair == (a, b):
        return {"status": "already_scanned", "from_anchor": a, "to_anchor": b,
                "detected": 0}
    cfg = await _load_config(b)
    if not cfg.lifetime_uuids:
        return {"status": "no_lifetime_boards", "detected": 0}
    if not (0 < b - a <= cfg.max_gap):
        _last_scanned_pair = (a, b)
        return {"status": "gap_too_large", "from_anchor": a, "to_anchor": b,
                "gap_seconds": b - a, "detected": 0}

    now = int(time.time())
    events, _fa, _fb = await _process_pair(a, b, cfg, now)
    if events:
        await pg_store.upsert_renames(events)
    _last_scanned_pair = (a, b)
    logger.info("renames: live pass %d→%d found %d rename(s)", a, b, len(events))
    return {"status": "ok", "from_anchor": a, "to_anchor": b,
            "gap_seconds": b - a, "detected": len(events)}


# ── backfill driver (dev portal) ─────────────────────────────────────────────

_STATUS_KEY = "lb:renames:status"
_RUNNING_KEY = "lb:renames:running"
_local_status: dict = {"running": False}


async def _set_status(status: dict) -> None:
    global _local_status
    _local_status = status
    from app.core.redis import get_redis
    r = get_redis()
    if r is not None:
        try:
            import json
            await r.set(_STATUS_KEY, json.dumps(status), ex=86400)
        except Exception:
            pass


async def get_status() -> dict:
    """Latest backfill progress (shared via Redis across workers)."""
    from app.core.redis import get_redis
    r = get_redis()
    if r is not None:
        try:
            import json
            raw = await r.get(_STATUS_KEY)
            if raw:
                return json.loads(raw)
        except Exception:
            pass
    return dict(_local_status)


async def backfill(*, clear_first: bool = False) -> None:
    """Walk the WHOLE archive over every ≤gap adjacent capture pair, oldest-first,
    idempotently recording renames. Reset-safe (lifetime-only fingerprint) and
    gap-gated (multi-hour outage gaps are skipped). Single-flight guarded; slides
    each pair's later fingerprint into the next pair's earlier one so every anchor
    is loaded ~once. Publishes live progress for the dev-portal poll."""
    from app.core.redis import get_redis

    r = get_redis()
    if r is not None:
        got = await r.set(_RUNNING_KEY, "1", nx=True, ex=900)
        if not got:
            logger.info("renames backfill already running - skipping duplicate")
            return

    anchors = await pg_store.all_anchors_asc()
    cfg = await _load_config(anchors[-1] if anchors else None)
    now = int(time.time())
    total_pairs = max(0, len(anchors) - 1)
    status = {
        "running": True, "total": total_pairs, "done": 0, "detected": 0,
        "skipped_gap": 0, "started_at": now, "finished_at": None,
        "last_anchor": None, "clear_first": clear_first,
        "lifetime_boards": len(cfg.lifetime_uuids),
    }
    await _set_status(status)
    try:
        if clear_first:
            deleted = await pg_store.delete_all_renames()
            status["cleared"] = deleted
            await _set_status(status)

        if not cfg.lifetime_uuids:
            logger.warning("renames backfill: no lifetime boards resolved - nothing to do")
            return

        batch: list[dict] = []
        prev_anchor: int | None = None
        prev_fp: dict | None = None
        for i in range(1, len(anchors)):
            a, b = anchors[i - 1], anchors[i]
            if not (0 < b - a <= cfg.max_gap):
                status["skipped_gap"] += 1
                status["done"] += 1
                prev_anchor, prev_fp = None, None  # gap breaks the slide
                continue
            fp_a = prev_fp if prev_anchor == a and prev_fp is not None else None
            try:
                events, fp_a2, fp_b = await _process_pair(a, b, cfg, now, fp_a=fp_a)
            except Exception:
                logger.warning("renames backfill: pair %d→%d failed", a, b, exc_info=True)
                status["done"] += 1
                prev_anchor, prev_fp = None, None
                continue
            batch.extend(events)
            status["detected"] += len(events)
            status["done"] += 1
            status["last_anchor"] = b
            prev_anchor, prev_fp = b, fp_b
            # Flush in chunks so a long run persists incrementally + a crash keeps
            # what it found. Publish progress each flush.
            if len(batch) >= 500:
                await pg_store.upsert_renames(batch)
                batch = []
            if status["done"] % 50 == 0:
                await _set_status(status)
                if r is not None:
                    try:
                        await r.expire(_RUNNING_KEY, 900)  # heartbeat
                    except Exception:
                        pass
        if batch:
            await pg_store.upsert_renames(batch)
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
        logger.info(
            "renames backfill done: %d detected across %d pairs (%d skipped for gap)",
            status["detected"], status["done"], status["skipped_gap"],
        )


# ── serving ──────────────────────────────────────────────────────────────────

async def serve_list(*, limit: int = 50, offset: int = 0) -> dict:
    """Recent renames (most-recent-first) + total, for the tab / API."""
    from app.core import features as feature_flags

    enabled = await feature_flags.is_enabled(feature_flags.RENAMES_FLAG)
    if not enabled:
        return {"enabled": False, "renames": [], "total": 0,
                "limit": limit, "offset": offset, "method_version": METHOD_VERSION}
    rows, total = await pg_store.list_renames(limit=limit, offset=offset)
    return {
        "enabled": True, "renames": rows, "total": total,
        "limit": limit, "offset": offset, "method_version": METHOD_VERSION,
    }


async def history(name: str, *, max_nodes: int = 40) -> dict:
    """The full rename chain touching ``name``: walk rename edges in BOTH
    directions (an identity can rename several times: A→B→C) and return the
    ordered timeline plus the set of aliases. Bounded to ``max_nodes`` names so a
    pathological graph can't fan out unboundedly."""
    seen_names: set[str] = set()
    seen_edges: set[int] = set()
    edges: list[dict] = []
    frontier = [name.strip().lower()]
    while frontier and len(seen_names) < max_nodes:
        cur = frontier.pop()
        if cur in seen_names:
            continue
        seen_names.add(cur)
        for e in await pg_store.renames_for_name(cur):
            if e["id"] not in seen_edges:
                seen_edges.add(e["id"])
                edges.append(e)
            for nxt in (e["from_name"].lower(), e["to_name"].lower()):
                if nxt not in seen_names:
                    frontier.append(nxt)
    edges.sort(key=lambda e: (e["to_anchor"], e["id"]))
    aliases = sorted({e["from_name"] for e in edges} | {e["to_name"] for e in edges})
    return {
        "query": name,
        "aliases": aliases,
        "current_name": edges[-1]["to_name"] if edges else name,
        "edges": edges,
        "rename_count": len(edges),
    }


def reset() -> None:
    """Drop the in-process scan guard (the persisted rows are wiped separately in
    ``pg_store.reset_all``). Called on a full leaderboards reset."""
    global _last_scanned_pair
    _last_scanned_pair = None
