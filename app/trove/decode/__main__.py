"""Rebuild the repo copies in app/trove/gamedata/ from a local client.

    python -m app.trove.decode                  # every decoder, from the Live install
    python -m app.trove.decode class_levels     # just one
    python -m app.trove.decode --game E:\\Trove  # an extracted tree instead
    python -m app.trove.decode --check          # compare only, write nothing

The server does this by itself after each patch (runner.py); this is for
committing fresh baselines or trying a decoder change before deploying it.
"""
from __future__ import annotations

import argparse
import os
import sys

from app.trove.decode import store
from app.trove.decode.registry import DECODERS, dump
from app.trove.decode.tree import DirTree, LiveTree

LIVE = os.environ.get("TROVE_LIVE_DIR", r"C:/Program Files (x86)/Glyph/Games/Trove/Live")


def main() -> int:
    ap = argparse.ArgumentParser(prog="python -m app.trove.decode")
    ap.add_argument("names", nargs="*", choices=[[], *DECODERS], metavar="name",
                    help=f"decoders to run (default all): {', '.join(DECODERS)}")
    ap.add_argument("--live", default=LIVE, help="installed client folder (reads its .tfa archives)")
    ap.add_argument("--game", help="an extracted client tree, instead of --live")
    ap.add_argument("--check", action="store_true", help="report differences, write nothing")
    args = ap.parse_args()

    names = args.names or list(DECODERS)
    prefixes = sorted({p for n in names for p in DECODERS[n].PREFIXES})
    tree = DirTree(args.game, prefixes) if args.game else LiveTree(args.live, prefixes)

    changed = 0
    for name in names:
        module = DECODERS[name]
        data = module.build(tree)
        text = dump(module, data)
        target = store.BASELINE_DIR / module.OUTPUT
        old = target.read_text(encoding="utf-8") if target.exists() else ""
        same = old == text
        changed += not same
        print(f"{name:18} {module.count(data):5} entries  {'unchanged' if same else 'CHANGED'}")
        if not same and not args.check:
            store.write(target, text)
    return 1 if args.check and changed else 0


if __name__ == "__main__":
    sys.exit(main())
