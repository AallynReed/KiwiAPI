"""Read-only views of a game file tree for the decoders.

Paths are posix and relative to the client root (``prefabs/class/knight.binfab``,
``languages/en/delve.binfab``). Lookups ignore case - the archive keeps whatever
case the client's index uses - while listings return the stored spelling.
"""
from __future__ import annotations

import os
from collections import OrderedDict
from pathlib import Path

from app.trove.updates.archive import decompress_tfa, parse_tfi


class GameTree:
    def __init__(self, paths: list[str]):
        self._index = {p.lower(): p for p in paths}
        self._sorted = sorted(self._index.values(), key=str.lower)

    def _fetch(self, path: str) -> bytes | None:
        raise NotImplementedError

    def read(self, path: str) -> bytes | None:
        actual = self._index.get(path.lower())
        return None if actual is None else self._fetch(actual)

    def exists(self, path: str) -> bool:
        return path.lower() in self._index

    def files(self, prefix: str, suffix: str = "") -> list[str]:
        """Every file under ``prefix`` (recursive), sorted, optionally by suffix."""
        prefix, suffix = prefix.lower(), suffix.lower()
        return [p for p in self._sorted
                if p.lower().startswith(prefix) and p.lower().endswith(suffix)]

    def only(self, prefixes) -> GameTree:
        """A view of just ``prefixes``: a decoder sees the folders it declares and
        nothing that another decoder loaded alongside it."""
        return _Subset(self, prefixes)

    def dirs(self, prefix: str) -> list[str]:
        """Names of the immediate subdirectories of ``prefix``."""
        prefix = prefix.lower().rstrip("/") + "/"
        out = {p[len(prefix):].split("/", 1)[0] for p in self.files(prefix)
               if "/" in p[len(prefix):]}
        return sorted(out)


class _Subset(GameTree):
    def __init__(self, parent: GameTree, prefixes):
        wanted = tuple(p.lower() for p in prefixes)
        self._parent = parent
        super().__init__([p for p in parent._sorted if p.lower().startswith(wanted)])

    def _fetch(self, path: str) -> bytes | None:
        return self._parent._fetch(path)


class ArchiveTree(GameTree):
    """The latest version of a branch in the game-update archive (the server)."""

    def __init__(self, shas: dict[str, str], store):
        super().__init__(list(shas))
        self._shas = shas
        self._store = store

    @classmethod
    async def load(cls, branch: str, prefixes: list[str], store) -> ArchiveTree:
        from app.trove.updates.models import UpdateState
        coll = UpdateState.get_pymongo_collection()
        shas: dict[str, str] = {}
        for prefix in prefixes:
            query = {"branch": branch, "path": {"$gte": prefix, "$lt": prefix + "￿"}}
            async for row in coll.find(query, {"path": 1, "content_sha256": 1, "_id": 0}):
                shas[row["path"]] = row["content_sha256"]
        return cls(shas, store)

    def _fetch(self, path: str) -> bytes | None:
        return self._store.get(self._shas[path])


def _loose(root: Path, prefixes: list[str]) -> list[str]:
    """Prefixes that name a plain file at the client root (``Trove_x64.exe``)."""
    return [p for p in prefixes if not p.endswith("/") and (root / p).is_file()]


class LiveTree(GameTree):
    """An installed client, reading straight out of its .tfa archives; a prefix that
    names a loose file at the root is read from disk."""

    def __init__(self, root: str | os.PathLike, prefixes: list[str]):
        self._root = Path(root)
        self._where: dict[str, tuple[Path, int, int, int]] = {}
        self._loose = set(_loose(self._root, prefixes))
        for prefix in prefixes:
            for dp, _, fs in os.walk(self._root / prefix):
                if "index.tfi" not in fs:
                    continue
                d = Path(dp)
                rel = d.relative_to(self._root).as_posix()
                for e in parse_tfi((d / "index.tfi").read_bytes()):
                    self._where[f"{rel}/{e.name}"] = (d, e.archive_index, e.offset, e.size)
        self._archives: OrderedDict[tuple[Path, int], bytes] = OrderedDict()
        super().__init__([*self._where, *self._loose])

    def _fetch(self, path: str) -> bytes | None:
        if path in self._loose:
            return (self._root / path).read_bytes()
        d, ai, offset, size = self._where[path]
        key = (d, ai)
        content = self._archives.get(key)
        if content is None:
            content = decompress_tfa((d / f"archive{ai}.tfa").read_bytes())
            self._archives[key] = content
            if len(self._archives) > 16:
                self._archives.popitem(last=False)
        self._archives.move_to_end(key)
        return content[offset:offset + size]


class DirTree(GameTree):
    """An extracted client tree on disk."""

    def __init__(self, root: str | os.PathLike, prefixes: list[str]):
        self._root = Path(root)
        paths = _loose(self._root, prefixes)
        for prefix in prefixes:
            for dp, _, fs in os.walk(self._root / prefix):
                rel = Path(dp).relative_to(self._root).as_posix()
                paths += [f"{rel}/{f}" for f in fs]
        super().__init__(paths)

    def _fetch(self, path: str) -> bytes | None:
        return (self._root / path).read_bytes()
