"""One-off: erase unwanted downtime segments from the persisted /status timeline.

Two selection modes:

``--max-seconds N`` (default 300)
    Noise-length blips. The prober retries a failing probe back-to-back before it
    believes an outage (``_probe_with_retries``), but a miss that outlasts every
    attempt still records a "down" segment that closes a minute later. Never a
    real outage - just clutter in the /status incident log and a dent in uptime %.

``--common``
    Outages recorded SIMULTANEOUSLY in every environment. eu / us / pts are
    independent game hosts, so a genuine outage rarely hits all three to the
    second; what does is the SHARED auth gateway - ``_verdict`` marks an env down
    when the login probe fails, so one auth blip flips all three at once. Segments
    are matched by time overlap (widened by ``--tolerance``, since the prober
    probes the envs sequentially and their starts drift a few seconds apart).
    NOTE: a real game-wide maintenance window also hits all three - check the dry
    run before applying.

Either way the chain is HEALED, not just cut: survivors are stretched to cover
the hole and adjacent same-status segments are merged, so the timeline stays
continuous (``get_history`` sums segment durations to get its uptime denominator
- a hole would silently shrink it) and uptime recomputes as if it never happened.

The OPEN (ongoing) segment is never deleted: ``_record_transition`` reads it to
decide whether the status changed, and its length isn't final yet. It can still
serve as evidence that another env was down at the same time.

Run it INSIDE the API container (that's where Mongo is reachable):

    docker compose exec api python scripts/prune_downtimes.py                  # dry run, blips
    docker compose exec api python scripts/prune_downtimes.py --common         # dry run, all-env
    docker compose exec api python scripts/prune_downtimes.py --common --apply # wipe

Dry run by default - nothing is written without ``--apply``.
"""
import argparse
import sys
from datetime import datetime, timezone

from pymongo import MongoClient

from app.core.config import settings

COLLECTION = "trove_status_events"
FOREVER = 1 << 62  # stand-in end for the open segment


def stamp(unix: int | None) -> str:
    if unix is None:
        return "ongoing"
    return datetime.fromtimestamp(unix, timezone.utc).strftime("%Y-%m-%d %H:%M:%S UTC")


def duration(seconds: int) -> str:
    if seconds < 60:
        return f"{seconds}s"
    return f"{seconds // 60}m {seconds % 60}s"


def is_downtime(r: dict) -> bool:
    # Legacy rows use status "maintenance"; anything not "online" is downtime.
    return r.get("status") != "online"


def end_of(r: dict) -> int:
    end = r.get("ended_at")
    return FOREVER if end is None else end


def select_short(rows: list[dict], max_seconds: int) -> set:
    """Closed downtime segments no longer than ``max_seconds``."""
    return {
        r["_id"] for r in rows
        if is_downtime(r) and r.get("ended_at") is not None
        and (r["ended_at"] - r["started_at"]) <= max_seconds
    }


def select_common(by_env: dict[str, list[dict]], tolerance: int) -> dict[str, set]:
    """Closed downtime segments that EVERY other environment was also down for.

    Match = time overlap, each segment widened by ``tolerance`` on both sides so
    the few seconds of drift between sequential per-env probes still counts as
    the same event. Returns {env: {ids to delete}}."""
    downs = {
        env: [(r["started_at"], end_of(r)) for r in rows if is_downtime(r)]
        for env, rows in by_env.items()
    }
    doomed: dict[str, set] = {}
    for env, rows in by_env.items():
        others = [e for e in by_env if e != env]
        hit = set()
        for r in rows:
            if not is_downtime(r) or r.get("ended_at") is None:
                continue  # never delete the open segment
            lo, hi = r["started_at"] - tolerance, r["ended_at"] + tolerance
            if all(any(s <= hi and lo <= e for s, e in downs[o]) for o in others):
                hit.add(r["_id"])
        doomed[env] = hit
    return doomed


def plan(rows: list[dict], doomed_ids: set) -> tuple[list[dict], list[dict], list[dict]]:
    """Plan one environment's prune given the ids to remove. Returns (doomed,
    absorbed, changed): the removals, segments deleted because a same-status
    neighbour absorbed them, and survivors whose bounds moved (mutated in
    place). Pure - the caller decides whether to write."""
    rows = sorted(rows, key=lambda r: r["started_at"])
    if not rows:
        return [], [], []
    span_start = rows[0]["started_at"]
    span_end = rows[-1].get("ended_at")  # None while the last segment is open

    doomed = [r for r in rows if r["_id"] in doomed_ids]
    if not doomed:
        return [], [], []
    keep = [r for r in rows if r["_id"] not in doomed_ids]
    changed: dict = {}

    # Stitch: the survivors must still tile the original span with no holes, so
    # each one grows into the gap left by whatever was removed after it.
    if keep:
        if keep[0]["started_at"] != span_start:
            keep[0]["started_at"] = span_start
            changed[keep[0]["_id"]] = keep[0]
        for a, b in zip(keep, keep[1:], strict=False):  # pairwise neighbours
            if a.get("ended_at") != b["started_at"]:
                a["ended_at"] = b["started_at"]
                changed[a["_id"]] = a
        last = keep[-1]
        if last.get("ended_at") is not None and span_end is not None and last["ended_at"] != span_end:
            last["ended_at"] = span_end
            changed[last["_id"]] = last

    # Merge: removing a downtime leaves online|online back to back. Collapse each
    # same-status run into its head so the history stays one segment per state.
    absorbed: list[dict] = []
    i = 0
    while i < len(keep):
        head = keep[i]
        j = i + 1
        while (j < len(keep) and keep[j].get("status") == head.get("status")
               and keep[j].get("online") == head.get("online")):
            head["ended_at"] = keep[j].get("ended_at")
            absorbed.append(keep[j])
            j += 1
        if j > i + 1:
            changed[head["_id"]] = head
        i = j

    absorbed_ids = {r["_id"] for r in absorbed}
    return doomed, absorbed, [r for r in changed.values() if r["_id"] not in absorbed_ids]


def main() -> int:
    ap = argparse.ArgumentParser(description=__doc__)
    ap.add_argument("--common", action="store_true",
                    help="erase outages recorded in EVERY environment at once "
                         "(the shared auth gateway) instead of short blips")
    ap.add_argument("--max-seconds", type=int, default=300,
                    help="short-blip mode: longest downtime to erase (default 300)")
    ap.add_argument("--tolerance", type=int, default=120,
                    help="--common: seconds of drift still counted as the same event")
    ap.add_argument("--apply", action="store_true",
                    help="actually write; without it this is a dry run")
    args = ap.parse_args()

    client: MongoClient = MongoClient(settings.mongo_uri, tz_aware=True)
    col = client[settings.mongo_db][COLLECTION]
    # Every env present in the data, not just the ones we probe today (legacy
    # rows used other names).
    envs = sorted(col.distinct("env"))
    by_env = {env: list(col.find({"env": env}).sort("started_at", 1)) for env in envs}

    if args.common:
        what = f"outages shared by all {len(envs)} environment(s) (±{args.tolerance}s)"
        selection = select_common(by_env, args.tolerance)
    else:
        what = f"downtimes <= {args.max_seconds}s"
        selection = {env: select_short(rows, args.max_seconds) for env, rows in by_env.items()}
    print(f"{'APPLY' if args.apply else 'DRY RUN'}: {what} "
          f"across {len(envs)} environment(s): {', '.join(envs) or '(none)'}\n")

    removed_total = merged_total = seconds_total = 0
    for env in envs:
        rows = by_env[env]
        doomed, absorbed, changed = plan(rows, selection[env])
        outages_before = sum(1 for r in rows if is_downtime(r))
        print(f"[{env}] {len(rows)} segment(s), {outages_before} outage(s) recorded")
        if not doomed:
            print("  nothing to erase\n")
            continue
        for r in doomed:
            secs = r["ended_at"] - r["started_at"]
            print(f"  - {r.get('status')}  {stamp(r['started_at'])} -> "
                  f"{stamp(r.get('ended_at'))}  ({duration(secs)})")
            seconds_total += secs
        print(f"  => {len(doomed)} erased, {len(absorbed)} neighbour(s) merged away, "
              f"{len(changed)} segment(s) re-stretched, "
              f"{outages_before - len(doomed)} outage(s) left")
        removed_total += len(doomed)
        merged_total += len(absorbed)

        if args.apply:
            # Survivors are widened BEFORE the deletes: a crash between the two
            # then leaves a harmless overlap rather than a hole in the timeline.
            for r in changed:
                col.update_one(
                    {"_id": r["_id"]},
                    {"$set": {"started_at": r["started_at"], "ended_at": r.get("ended_at")}},
                )
            col.delete_many({"_id": {"$in": [r["_id"] for r in doomed + absorbed]}})
            print("  written")
        print()

    print(f"{'Erased' if args.apply else 'Would erase'} {removed_total} downtime segment(s) "
          f"({duration(seconds_total)} of recorded downtime), "
          f"merging {merged_total} neighbour(s) away.")
    if removed_total and not args.apply:
        print("Re-run with --apply to write it.")
    client.close()
    return 0


if __name__ == "__main__":
    sys.exit(main())
