"""Rebase a MarketListing Mongo dump's timestamps to "now" for demo
purposes.

Why this exists
---------------
We have a 1.27 GB MarketListing dump on disk from a previous capture
window. Real market timestamps in that file are months stale. The
public /market page should look freshly-pulled when we light it up,
so this script:

  1. streams the input ``trove_api.MarketListing.json`` (a top-level
     JSON array), finds the LARGEST ``last_seen`` across every doc
  2. computes ``offset = int(time.time()) - max_last_seen``
  3. streams again, rewriting BOTH ``last_seen`` and ``created_at``
     on each doc with the offset added, dropping any doc whose
     shifted ``last_seen`` is older than 7 days
  4. writes ndjson (one MongoDB extended-JSON doc per line) to
     ``trove_api.MarketListing.shifted.ndjson`` so a follow-up
     loader (or ``mongoimport``) can ingest in bulk

The source file is read as a streaming JSON array — we never load
1+ GB into memory at once. ``json.JSONDecoder.raw_decode`` parses one
object at a time from a sliding buffer.

Run::

    python scripts/market_shift_demo_data.py \\
        --input  "S:/Desktop/Databases/trove_api.MarketListing.json" \\
        --output "S:/Desktop/Databases/trove_api.MarketListing.shifted.ndjson"
"""
from __future__ import annotations

import argparse
import json
import sys
import time
from pathlib import Path


# ---- Streaming JSON-array parser -------------------------------------------


def stream_json_array(path: Path, chunk_size: int = 1 << 20):
    """Yield items from a top-level JSON array, one at a time, without
    holding the whole file in memory. Works on the dump's
    ``[{...},{...},...]`` shape produced by ``mongoexport`` /
    ``mongodump --jsonArray``.

    Mechanism: open as text, keep a sliding buffer, use
    ``json.JSONDecoder.raw_decode`` to peel one value off the head
    repeatedly. On a partial value we read another chunk and retry.
    """
    decoder = json.JSONDecoder()
    with path.open("r", encoding="utf-8") as fp:
        buf = ""
        # ── Locate the opening ``[`` (mongoexport sometimes wraps with
        # whitespace / a newline before it).
        while "[" not in buf:
            chunk = fp.read(chunk_size)
            if not chunk:
                raise ValueError(f"{path}: no JSON array found")
            buf += chunk
        buf = buf[buf.index("[") + 1:]

        while True:
            # ── Skip whitespace + commas between items.
            stripped = buf.lstrip(" \t\n\r,")
            if not stripped:
                chunk = fp.read(chunk_size)
                if not chunk:
                    return
                buf = chunk
                continue
            buf = stripped
            if buf[0] == "]":
                return
            # ── Try to peel one object off the head of buf.
            try:
                obj, idx = decoder.raw_decode(buf)
                buf = buf[idx:]
                yield obj
            except json.JSONDecodeError:
                # Partial — pull another chunk and retry.
                chunk = fp.read(chunk_size)
                if not chunk:
                    raise
                buf += chunk


# ---- Pass 1: find max timestamp --------------------------------------------


def find_max_last_seen(path: Path) -> tuple[int, int]:
    """Returns ``(max_last_seen, doc_count)``. Prints a progress dot
    every 250k docs so a multi-minute scan doesn't look frozen."""
    max_ts = 0
    n = 0
    for doc in stream_json_array(path):
        ls = doc.get("last_seen", 0)
        ca = doc.get("created_at", 0)
        if ls > max_ts:
            max_ts = ls
        if ca > max_ts:
            max_ts = ca
        n += 1
        if n % 250_000 == 0:
            print(f"  scanned {n:,} docs, current max={max_ts}", flush=True)
    return max_ts, n


# ---- Pass 2: rewrite + filter ---------------------------------------------


def rewrite_with_offset(
    input_path: Path,
    output_path: Path,
    offset: int,
    window_seconds: int,
) -> tuple[int, int]:
    """Pass through the input, shift both timestamps by ``offset``,
    keep only docs whose shifted ``last_seen`` is within
    ``[now - window_seconds, now]``. Returns ``(written, skipped)``.

    Output format: one JSON object per line. The ``_id`` field is
    preserved verbatim (it's MongoDB's extended-JSON binary envelope
    — re-importable by ``mongoimport`` or our own loader without
    further surgery)."""
    now = int(time.time())
    cutoff = now - window_seconds
    written = 0
    skipped = 0

    with output_path.open("w", encoding="utf-8") as out:
        for doc in stream_json_array(input_path):
            new_ls = doc.get("last_seen", 0) + offset
            new_ca = doc.get("created_at", 0) + offset
            if new_ls < cutoff:
                # Outside the 7-day window — drop. Keeps the demo dataset
                # focused on what the page would actually surface.
                skipped += 1
                continue
            # Bound on the right too — if max_ts in the input was less
            # than the actual ``now`` (clock drift, partial dump), some
            # shifted values might exceed ``now``. Clamp them so the
            # UI doesn't display a future ``last_seen``.
            if new_ls > now:
                new_ls = now
            if new_ca > now:
                new_ca = now
            doc["last_seen"] = new_ls
            doc["created_at"] = new_ca
            out.write(json.dumps(doc, separators=(",", ":")))
            out.write("\n")
            written += 1
            if (written + skipped) % 250_000 == 0:
                print(
                    f"  pass 2: wrote {written:,} / "
                    f"skipped {skipped:,} / "
                    f"total {written + skipped:,}",
                    flush=True,
                )
    return written, skipped


# ---- CLI -------------------------------------------------------------------


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument(
        "--input",
        type=Path,
        default=Path(r"S:/Desktop/Databases/trove_api.MarketListing.json"),
        help="Input dump (JSON array of MarketListing docs).",
    )
    p.add_argument(
        "--output",
        type=Path,
        default=Path(r"S:/Desktop/Databases/trove_api.MarketListing.shifted.ndjson"),
        help="Output ndjson — one shifted doc per line.",
    )
    p.add_argument(
        "--window-days",
        type=int,
        default=7,
        help="Keep only docs whose SHIFTED last_seen is within this "
             "many days of now. Default 7 (the in-game listing TTL).",
    )
    p.add_argument(
        "--skip-find-max",
        action="store_true",
        help="If you already know the offset, skip pass 1 — pass --offset.",
    )
    p.add_argument(
        "--offset",
        type=int,
        default=None,
        help="Override the computed offset (seconds added to every "
             "timestamp). Useful for re-running pass 2 only.",
    )
    return p.parse_args()


def main() -> int:
    args = parse_args()
    if not args.input.is_file():
        print(f"ERROR: input missing: {args.input}", file=sys.stderr)
        return 2
    args.output.parent.mkdir(parents=True, exist_ok=True)

    now = int(time.time())

    # ── Pass 1: find the largest last_seen in the dump.
    if args.offset is not None:
        offset = args.offset
        print(f"using --offset={offset} (pass 1 skipped)")
        max_ts = now - offset
        total = -1
    else:
        print(f"pass 1: scanning {args.input}")
        t0 = time.time()
        max_ts, total = find_max_last_seen(args.input)
        print(f"  pass 1: {total:,} docs in {time.time() - t0:.1f}s")
        if max_ts <= 0:
            print("ERROR: no positive timestamps found", file=sys.stderr)
            return 3
        offset = now - max_ts
        print(f"  max_last_seen = {max_ts}  (offset to now = {offset:+,}s)")

    # ── Pass 2: rewrite with offset, filter to window.
    window_seconds = args.window_days * 86_400
    print(
        f"pass 2: writing {args.output} "
        f"(offset {offset:+,}s, window {args.window_days}d)"
    )
    t0 = time.time()
    written, skipped = rewrite_with_offset(
        args.input, args.output, offset, window_seconds,
    )
    print(
        f"  pass 2: {written:,} kept, {skipped:,} dropped "
        f"in {time.time() - t0:.1f}s"
    )

    return 0


if __name__ == "__main__":
    sys.exit(main())
