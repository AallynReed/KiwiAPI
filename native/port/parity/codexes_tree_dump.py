"""Run the real Python codex indexer over a directory tree and dump every row.

The archive (UpdateState + CAS) and Postgres are faked: paths come from the
directory (Mongo's path order), a blob's "sha" is its relative path, and every
pg_store write is captured. The Rust side dumps the same rows
(src/codexes/tree_tests.rs) and codexes_tree_diff.py compares them.

    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe \
        native/port/parity/codexes_tree_dump.py E:/Trove OUT.jsonl [Trove_x64.exe]
"""

from __future__ import annotations

import asyncio
import json
import os
import re
import sys
from types import SimpleNamespace

sys.path.insert(0, os.path.abspath(os.path.join(os.path.dirname(__file__), "..", "..", "..")))

from app.trove.codexes import indexer as ix  # noqa: E402
from app.trove.codexes import pg_store  # noqa: E402

ROOTS = ("prefabs", "languages", "blueprints")
EXE = "Trove_x64.exe"


def walk(root: str) -> list[str]:
    out = []
    for top in ROOTS:
        for d, _dirs, files in os.walk(os.path.join(root, top)):
            for f in files:
                out.append(os.path.relpath(os.path.join(d, f), root).replace("\\", "/"))
    return sorted(out)


class Store:
    def __init__(self, root: str, exe: str | None):
        self.root, self.exe = root, exe

    def get(self, sha: str) -> bytes | None:
        p = self.exe if sha == EXE else os.path.join(self.root, sha)
        if p and os.path.isfile(p):
            with open(p, "rb") as f:
                return f.read()
        return None


class Cursor:
    def __init__(self, docs):
        self.docs = docs

    async def to_list(self, length=None):
        return list(self.docs)

    def __aiter__(self):
        self._it = iter(self.docs)
        return self

    async def __anext__(self):
        try:
            return next(self._it)
        except StopIteration:
            raise StopAsyncIteration from None


class Coll:
    def __init__(self, paths):
        self.paths = paths

    def find(self, flt, projection=None):
        cond = flt["path"]
        if "$regex" in cond:
            rx = re.compile(cond["$regex"], re.I if "i" in cond.get("$options", "") else 0)
            hits = [p for p in self.paths if rx.search(p)]
        else:
            hits = [p for p in self.paths if cond["$gte"] <= p < cond["$lt"]]
        return Cursor([{"path": p, "content_sha256": p} for p in hits])


def install(root: str, exe: str | None, sink: list):
    paths = walk(root)
    if exe:
        paths = sorted(paths + [EXE])
    known = set(paths)
    coll = Coll(paths)

    async def find_one(q):
        return SimpleNamespace(content_sha256=q["path"]) if q["path"] in known else None

    ix.UpdateState = SimpleNamespace(get_pymongo_collection=lambda: coll, find_one=find_one)

    def cap(kind):
        async def f(*args, **kw):
            sink.append((kind, args, kw))
            rows = args[-1] if args else []
            return len(rows) if isinstance(rows, list) else 0
        return f

    async def noop(*a, **k):
        return None

    async def empty(*a, **k):
        return {}

    pg_store.existing_shas = empty
    pg_store.upsert_entries = cap("entries")
    pg_store.replace_children = cap("children")
    pg_store.delete_entries = cap("delete")
    pg_store.replace_requirements = cap("requirements")
    pg_store.replace_upgrades = cap("upgrades")
    pg_store.replace_links_for = cap("links_for")
    pg_store.replace_rig_bindings = cap("rigs")
    pg_store.set_parser_version = noop
    pg_store.set_rig_version = noop
    pg_store.touch_meta = noop


def dump(out, kind, row):
    out.write(json.dumps({"t": kind, "row": list(row)}, ensure_ascii=False) + "\n")


DELTA_ROOT = "prefabs/collections/"


def main() -> None:
    args = [a for a in sys.argv[1:] if a != "--delta"]
    delta = "--delta" in sys.argv
    root, out_path = args[0], args[1]
    exe = args[2] if len(args) > 2 else None
    sink: list = []
    install(root, exe, sink)
    store = Store(root, exe)
    if delta:
        # Every collection prefab "modified" in one version: no recipe, so the maps
        # load without the resolver (no prefab index, no ability names).
        changes = [{"path": p, "type": "modified", "content_sha256": p}
                   for p in walk(root) if p.startswith(DELTA_ROOT)]
        coll = SimpleNamespace(find=lambda flt, projection=None: Cursor(changes))
        ix.UpdateChange = SimpleNamespace(get_pymongo_collection=lambda: coll)
        counts = asyncio.run(ix.reindex_changes("live-us", store, 7))
    else:
        counts = asyncio.run(ix.reindex("live-us", store, force=True))
        asyncio.run(ix.reindex_rigs("live-us", store))
    with open(out_path, "w", encoding="utf-8") as out:
        for kind, args, kw in sink:
            if kind == "entries":
                for row in args[0]:
                    dump(out, "entry", row[:-1])
            elif kind == "children":
                for name in ("stats", "abilities", "links"):
                    for row in kw[name]:
                        dump(out, name[:-1] if name != "abilities" else "ability", row)
            elif kind == "requirements":
                for row in args[1]:
                    dump(out, "requirement", row)
            elif kind == "upgrades":
                for row in args[1]:
                    dump(out, "upgrade", row)
            elif kind == "links_for":
                for row in args[2]:
                    dump(out, "link", row)
            elif kind == "rigs":
                for row in args[1]:
                    dump(out, "rig", row)
    print(json.dumps(counts))


if __name__ == "__main__":
    main()
