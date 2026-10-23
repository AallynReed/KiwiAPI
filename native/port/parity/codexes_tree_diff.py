"""Compare the codex rows codexes_tree_dump.py (Python) and tree_tests.rs (Rust)
wrote: every table as a multiset of rows, JSONB compared as values.

    python native/port/parity/codexes_tree_diff.py py.jsonl rs.jsonl
"""

from __future__ import annotations

import collections
import json
import sys


def load(path: str) -> dict[str, collections.Counter]:
    out: dict[str, collections.Counter] = collections.defaultdict(collections.Counter)
    with open(path, encoding="utf-8") as f:
        for line in f:
            rec = json.loads(line)
            out[rec["t"]][json.dumps(rec["row"], sort_keys=True, ensure_ascii=False)] += 1
    return out


def main() -> int:
    a, b = load(sys.argv[1]), load(sys.argv[2])
    bad = 0
    for kind in sorted(set(a) | set(b)):
        only_a, only_b = a[kind] - b[kind], b[kind] - a[kind]
        print(f"{kind}: python {sum(a[kind].values())} rust {sum(b[kind].values())} "
              f"python-only {sum(only_a.values())} rust-only {sum(only_b.values())}")
        for label, rows in (("python", only_a), ("rust", only_b)):
            for row in list(rows)[:5]:
                print(f"  {label}-only: {row[:600]}")
        bad += sum(only_a.values()) + sum(only_b.values())
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main())
