"""Where decoded game data is read from and written to.

The repo ships a baseline of every file in app/trove/gamedata/. On the server the
decoders rewrite them after each patch into ``settings.gamedata_dir`` (a bind mount
shared by the api, web and bot containers), and a file there wins over the
baseline. Loads are keyed on the file's mtime, so a rebuilt file is picked up by
every process without a restart.
"""
from __future__ import annotations

import functools
import json
import os
import tempfile
from collections.abc import Callable
from pathlib import Path
from typing import Any, TypeVar

from app.core.config import settings

BASELINE_DIR = Path(__file__).resolve().parents[1] / "gamedata"

F = TypeVar("F", bound=Callable[..., Any])


def runtime_dir() -> Path | None:
    return Path(settings.gamedata_dir) if settings.gamedata_dir else None


def path(name: str) -> Path:
    """The file readers should use: the runtime rebuild if there is one."""
    rt = runtime_dir()
    if rt is not None and (rt / name).is_file():
        return rt / name
    return BASELINE_DIR / name


def signature(*names: str) -> tuple:
    """Changes whenever any of ``names`` is rebuilt."""
    out = []
    for name in names:
        p = path(name)
        try:
            out.append((str(p), p.stat().st_mtime_ns))
        except OSError:
            out.append((str(p), None))
    return tuple(out)


def cached(*names: str) -> Callable[[F], F]:
    """Like ``functools.cache``, but dropped whenever one of ``names`` changes."""
    def deco(fn: F) -> F:
        memo: dict = {}
        sig_box: list = [None]

        @functools.wraps(fn)
        def wrapper(*args):
            sig = signature(*names)
            if sig != sig_box[0]:
                memo.clear()
                sig_box[0] = sig
            if args not in memo:
                memo[args] = fn(*args)
            return memo[args]
        return wrapper  # type: ignore[return-value]
    return deco


def load(name: str, default: Any = None) -> Any:
    """Parsed JSON for ``name``, or ``default`` if it is missing or unreadable."""
    data = _load(name, signature(name))
    return default if data is _MISSING else data


_MISSING = object()


@functools.lru_cache(maxsize=64)
def _load(name: str, _sig: tuple) -> Any:
    try:
        return json.loads(path(name).read_text(encoding="utf-8"))
    except (OSError, ValueError):
        return _MISSING


def baseline(name: str) -> Any:
    """The repo copy, which is where hand-curated fields live."""
    return json.loads((BASELINE_DIR / name).read_text(encoding="utf-8"))


def write(target: Path, text: str) -> None:
    """Replace ``target`` atomically so a reader never sees half a file."""
    target.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=target.parent, prefix=f".{target.name}.")
    try:
        with os.fdopen(fd, "w", encoding="utf-8", newline="") as fh:
            fh.write(text)
        os.chmod(tmp, 0o644)  # mkstemp makes it 0600; the other containers read it
        os.replace(tmp, target)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise
