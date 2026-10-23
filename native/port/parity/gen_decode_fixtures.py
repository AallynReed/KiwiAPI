#!/usr/bin/env python3
"""Golden outputs for the game-data decoders (native/kiwi-server/src/decode).

    python gen_decode_fixtures.py full GAME_ROOT OUT_DIR [name ...]
        every decoder (or the named ones) over an extracted client tree, written
        as OUT_DIR/<OUTPUT> exactly as the runner writes it, plus OUT_DIR/_counts.json
    python gen_decode_fixtures.py subset GAME_ROOT [name ...]
        cut those small trees from an extracted client: the first files each
        build reads, within a byte budget
    python gen_decode_fixtures.py view SRC_DIR OUT_DIR
        the Workshop binfab preview of every file under SRC_DIR, one .json each
        (testdata/decode/view holds a small committed set: binfabs/ and expected/)
    python gen_decode_fixtures.py fixtures
        the small committed trees under testdata/decode/<name>/game: each one's
        golden output as expected.json next to it (and code_versions.json)

Run with the trove-prod venv from the repo root, with kiwi_native importable
(production parses with it), e.g. PYTHONPATH=<dir holding kiwi_native.pyd>.
"""
from __future__ import annotations

import json
import sys
import time
import traceback
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))

from app.trove.decode.registry import DECODERS, code_version, dump  # noqa: E402
from app.trove.decode.tree import DirTree  # noqa: E402

TESTDATA = ROOT / "native" / "kiwi-server" / "testdata" / "decode"


def full(game: Path, out: Path, names: list[str]) -> None:
    out.mkdir(parents=True, exist_ok=True)
    names = names or list(DECODERS)
    prefixes = sorted({p for n in names for p in DECODERS[n].PREFIXES})
    tree = DirTree(game, prefixes)
    counts = {}
    for name in names:
        module = DECODERS[name]
        t0 = time.time()
        try:
            data = module.build(tree.only(module.PREFIXES))
            (out / module.OUTPUT).write_bytes(dump(module, data).encode("utf-8"))
            counts[name] = module.count(data)
        except Exception as exc:  # noqa: BLE001
            counts[name] = f"{type(exc).__name__}: {exc}"
            traceback.print_exc()
        print(f"{name:20} {counts[name]!s:>8}  {time.time() - t0:.1f}s", flush=True)
    (out / "_counts.json").write_text(json.dumps(counts, indent=1), encoding="utf-8")


class _Recording(DirTree):
    """A DirTree noting every file a build reads, in order."""

    def __init__(self, root, prefixes):
        super().__init__(root, prefixes)
        self.seen: list[str] = []

    def _fetch(self, path):
        if path not in self.seen:
            self.seen.append(path)
        return super()._fetch(path)


def subset(game: Path, names: list[str], budget: int, max_file: int) -> None:
    """Small committed trees: the first prefabs each build reads, then text tables,
    within a byte budget."""
    import shutil
    for name in names or list(DECODERS):
        module = DECODERS[name]
        tree = _Recording(game, list(module.PREFIXES))
        try:
            module.build(tree.only(module.PREFIXES))
        except Exception:  # noqa: BLE001
            pass
        dest = TESTDATA / name / "game"
        shutil.rmtree(dest, ignore_errors=True)
        used = 0
        text = [p for p in tree.seen if p.startswith("languages/")]
        for limit, paths in ((budget * 7 // 10, [p for p in tree.seen if p not in text]), (budget, text)):
            for path in paths:
                size = (game / path).stat().st_size
                if size > max_file or used + size > limit:
                    continue
                (dest / path).parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(game / path, dest / path)
                used += size
        print(name, used)


def view(src: Path, out: Path) -> None:
    """The Workshop binfab preview body for every file under ``src`` (as the route
    sends it, with ``path``), or ``!message`` where it answers 422."""
    from app.trove.decode import view as v

    out.mkdir(parents=True, exist_ok=True)
    for path in sorted(p for p in src.rglob("*") if p.is_file() and p.suffix != ".json"):
        data = path.read_bytes()
        rel = path.relative_to(src).as_posix()
        try:
            if len(data) > v.MAX_BYTES:
                raise v.BinfabViewError("This file is too large to lay out.")
            payload = v._describe_native(data)
            if payload is None:
                payload = v.describe(data)
            text = json.dumps({"path": rel, **payload}, ensure_ascii=False, separators=(",", ":"))
        except v.BinfabViewError as exc:
            text = f"!{exc}"
        (out / (rel.replace("/", "__") + ".json")).write_bytes(text.encode("utf-8"))


def _server_versions() -> dict[str, str]:
    """``code_version`` of every decoder over LF sources, as the server's checkout has them."""
    import pathlib

    original = pathlib.Path.read_bytes
    pathlib.Path.read_bytes = lambda self: original(self).replace(b"\r\n", b"\n")
    try:
        return {name: code_version(module) for name, module in DECODERS.items()}
    finally:
        pathlib.Path.read_bytes = original


def fixtures() -> None:
    versions = _server_versions()
    (TESTDATA / "code_versions.json").write_text(json.dumps(versions, indent=1) + "\n", encoding="utf-8")
    for case in sorted(p for p in TESTDATA.iterdir() if (p / "game").is_dir()):
        name = case.name.split("@", 1)[0]
        module = DECODERS[name]
        tree = DirTree(case / "game", list(module.PREFIXES))
        try:
            text = dump(module, module.build(tree.only(module.PREFIXES)))
        except Exception as exc:  # noqa: BLE001
            text = f"!{type(exc).__name__}: {exc}"
        (case / "expected.json").write_bytes(text.encode("utf-8"))
        print(case.name, len(text))


if __name__ == "__main__":
    if sys.argv[1] == "full":
        full(Path(sys.argv[2]), Path(sys.argv[3]), sys.argv[4:])
    elif sys.argv[1] == "view":
        view(Path(sys.argv[2]), Path(sys.argv[3]))
    elif sys.argv[1] == "subset":
        subset(Path(sys.argv[2]), sys.argv[3:], budget=100_000, max_file=72_000)
    else:
        fixtures()
