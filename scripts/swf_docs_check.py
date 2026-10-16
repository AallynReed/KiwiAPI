"""Compare the game's ui/*.swf against the hashes recorded in site/static/swf-docs/index.json.

    python scripts/swf_docs_check.py                    # Live install, report only
    python scripts/swf_docs_check.py --game E:\\Trove    # an extracted tree instead
    python scripts/swf_docs_check.py --extract out/     # also write changed SWFs there
    python scripts/swf_docs_check.py --update           # record current hashes + sizes

Run --update only after the pages of every CHANGED file have been brought up to date:
the stored hash means "the docs describe these bytes".
"""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import sys
from datetime import datetime, timezone
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))

from app.trove.decode.tree import DirTree, LiveTree  # noqa: E402

LIVE = os.environ.get("TROVE_LIVE_DIR", r"C:/Program Files (x86)/Glyph/Games/Trove/Live")
DOCS = Path(__file__).resolve().parents[1] / "site" / "static" / "swf-docs"
SUMMARY = re.compile(r"# [^\n]*\s*\n> ?([^\n]+)")
ROW = re.compile(r"^(\| \[(?P<swf>[\w.]+\.swf)\]\(\./[\w.]+\.md\) \| )[^|]+( \| [^|]+ \| ).*( \|)$", re.M)


def fmt_size(n: int) -> str:
    kb = n / 1024
    return f"{kb / 1024:.1f} MB" if kb >= 1024 else f"{kb:.0f} KB"


def short(summary: str) -> str:
    return summary if len(summary) <= 140 else summary[:137] + "..."


def write_crlf(path: Path, text: str) -> None:
    path.write_bytes(text.replace("\r\n", "\n").replace("\n", "\r\n").encode("utf-8"))


def main() -> int:
    ap = argparse.ArgumentParser(prog="python scripts/swf_docs_check.py")
    ap.add_argument("--live", default=LIVE, help="installed client folder (reads its .tfa archives)")
    ap.add_argument("--game", help="an extracted client tree, instead of --live")
    ap.add_argument("--extract", help="write every changed or new SWF into this folder")
    ap.add_argument("--update", action="store_true", help="record current hashes and sizes")
    args = ap.parse_args()

    tree = DirTree(args.game, ["ui/"]) if args.game else LiveTree(args.live, ["ui/"])
    files: dict[str, bytes] = {}
    for p in tree.files("ui/", ".swf"):
        data = tree.read(p)
        if p.count("/") == 1 and data is not None:
            files[p.split("/")[-1]] = data

    index_path = DOCS / "index.json"
    index = json.loads(index_path.read_text(encoding="utf-8"))
    items = {it["swf"]: it for g in index["groups"] for it in g["items"] if it["swf"].endswith(".swf")}

    changed, new = [], sorted(set(files) - set(items))
    for name in sorted(set(files) & set(items)):
        digest = hashlib.sha256(files[name]).hexdigest()
        if items[name].get("sha256") != digest:
            changed.append(name)
            print(f"CHANGED  {name:32} {items[name].get('size', '?'):>8} -> {fmt_size(len(files[name]))}")
    for name in new:
        print(f"NEW      {name:32} {fmt_size(len(files[name]))} (no page yet)")
    for name in sorted(set(items) - set(files)):
        print(f"MISSING  {name:32} documented but not in the client")
    print(f"{len(files) - len(changed) - len(new)} of {len(files)} unchanged"
          f" (recorded {index.get('checked', 'never')})")

    if args.extract:
        out = Path(args.extract)
        out.mkdir(parents=True, exist_ok=True)
        for name in changed + new:
            (out / name).write_bytes(files[name])

    if args.update:
        for name, it in items.items():
            if name in files:
                it["size"] = fmt_size(len(files[name]))
                it["sha256"] = hashlib.sha256(files[name]).hexdigest()
            lead = SUMMARY.match((DOCS / it["file"]).read_text(encoding="utf-8"))
            if lead:
                it["summary"] = lead.group(1).strip()
        index["checked"] = datetime.now(timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ")
        write_crlf(index_path, json.dumps(index, indent=2, ensure_ascii=False) + "\n")
        readme = DOCS / "README.md"
        text = readme.read_text(encoding="utf-8").replace("\r\n", "\n")
        text = ROW.sub(lambda m: f"{m.group(1)}{items[m['swf']]['size']}{m.group(3)}"
                                 f"{short(items[m['swf']]['summary'])}{m.group(4)}"
                       if m["swf"] in items else m.group(0), text)
        write_crlf(readme, text)
        print("index.json and README.md updated")
    return 1 if changed or new else 0


if __name__ == "__main__":
    sys.exit(main())
