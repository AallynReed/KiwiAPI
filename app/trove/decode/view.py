"""One ``.binfab`` laid out for reading - the Mod Workshop's binfab preview.

``describe`` turns ``wire.parse`` into a JSON tree: components, their sections
(root class first) and every field by index, typed. A field is labelled only where
``fields.py`` already has evidence for it; everything else stays a bare index. A
language table (field 0 = ``{0 key, 1 text}`` rows) comes back as plain key/text
pairs as well, since that is all a text mod changes.

The input is a stranger's upload and the reader is a search, so ``describe_isolated``
runs it in a child process (this module's ``__main__``) under a wall-clock kill and,
off Windows, an address-space cap - a crafted file costs one bounded process, never
the API worker.
"""
from __future__ import annotations

import asyncio
import json
import math
import sys
from pathlib import Path
from typing import Any

from app.trove.decode import fields as F
from app.trove.decode.ability import _is_damage, _is_healing, is_modifier
from app.trove.decode.wire import Obj, Repeated, WireError, parse

MAX_BYTES = 1024 * 1024
MAX_NODES = 60_000
TIMEOUT_SECONDS = 30
MEMORY_LIMIT_BYTES = 1024 * 1024 * 1024
_CONCURRENT = asyncio.Semaphore(2)
_ROOT = Path(__file__).resolve().parents[3]

_COMPONENT_NAMES = {F.IDENTITY: "Identity", F.PROC_SPAWNER: "Combat-event effect spawner"}
_IDENTITY = {F.ID_NAME: "name", F.ID_CATEGORY: "category",
             F.ID_DESCRIPTION: "description", F.ID_ICON: "icon"}
_EFFECT_BASE = {F.EFFECT_DURATION: "duration (s)", F.EFFECT_DESCRIPTION: "description",
                F.EFFECT_NAME: "name"}


class BinfabViewError(ValueError):
    """The file could not be laid out (the router maps it to a 4xx)."""


class _Budget:
    def __init__(self) -> None:
        self.left, self.truncated = MAX_NODES, False

    def spend(self) -> bool:
        if self.left <= 0:
            self.truncated = True
            return False
        self.left -= 1
        return True


def _component_name(cid: int) -> str | None:
    if cid in _COMPONENT_NAMES:
        return _COMPONENT_NAMES[cid]
    if cid in F.ACTIONS:
        return "Ability action"
    if cid in F.EFFECTS:
        return "Effect"
    return None


def _component_labels(cid: int, n_sections: int) -> dict[int, dict[int, str]]:
    """``{section index: {field index: label}}`` for one component."""
    leaf = n_sections - 1
    out: dict[int, dict[int, str]] = {}
    if cid == F.IDENTITY:
        out[leaf] = dict(_IDENTITY)
    if cid in F.ACTIONS:
        energy, cooldown = F.ACTIONS[cid]
        labels = out.setdefault(leaf, {})
        if energy is not None:
            labels[energy] = "energy cost"
        if cooldown is not None:
            labels[cooldown] = "cooldown"
    if cid in F.EFFECTS:
        out.setdefault(0, {}).update(_EFFECT_BASE)
    if cid == F.PROC_SPAWNER:
        out.setdefault(F.PROC_SECTION, {})[F.PROC_COOLDOWN] = "proc cooldown"
    return out


def _section_labels(section: dict) -> tuple[dict[int, str], str | None]:
    """Labels and a one-line note a section earns by its own shape."""
    if _is_damage(section):
        return dict(F.DAMAGE), "Damage parameters"
    if _is_healing(section):
        return dict(F.HEALING), "Healing parameters"
    if is_modifier(section):
        op = section.get(F.MOD_OP, 0)
        stat = F.stat_key(section.get(F.MOD_STAT, 0)).removeprefix("$Stat_")
        verb = F.MOD_OPS[op] if 0 <= op < len(F.MOD_OPS) else f"op {op}"
        return {F.MOD_STAT: "stat", F.MOD_OP: "operation", F.MOD_VALUE: "value",
                F.MOD_LABEL: "label", F.MOD_FLAGS: "flags"}, \
            f"Stat modifier: {stat} {verb} {section.get(F.MOD_VALUE)}"
    return {}, None


def _scalar(v: Any) -> dict:
    if isinstance(v, bool):
        return {"t": "i", "v": int(v)}
    if isinstance(v, int):
        # JSON numbers past 2^53 lose digits in a browser; an int64 reads as text.
        return {"t": "i", "v": v if abs(v) < 2 ** 53 else str(v)}
    if isinstance(v, float):
        return {"t": "f", "v": v if math.isfinite(v) else str(v)}
    if isinstance(v, str):
        return {"t": "s", "v": v}
    if isinstance(v, bytes):
        return {"t": "b", "n": len(v), "v": v[:48].hex()}
    return {"t": "s", "v": repr(v)}


def _node(v: Any, budget: _Budget, labels: dict[int, dict[int, str]] | None = None) -> dict | None:
    if not budget.spend():
        return None
    if v is None:
        return {"t": "z"}
    if isinstance(v, Obj):
        sections = []
        for si, section in enumerate(v):
            own, note = _section_labels(section)
            named = {**own, **((labels or {}).get(si) or {})}
            sections.append(_section(section, budget, named, note))
        return {"t": "o", "v": sections}
    if isinstance(v, Repeated):
        return {"t": "r", "v": [_node(x, budget) for x in v]}
    if isinstance(v, tuple):
        return {"t": "p", "v": [_scalar(x)["v"] for x in v]}
    if isinstance(v, list):
        return {"t": "a", "v": [_node(x, budget) for x in v]}
    if isinstance(v, dict):
        return {"t": "m", "v": [[k if isinstance(k, str) else _scalar(k)["v"], _node(x, budget)]
                                for k, x in v.items()]}
    return _scalar(v)


def _section(section: dict, budget: _Budget, labels: dict[int, str], note: str | None) -> dict:
    out: dict = {"f": [{"i": idx, **({"l": labels[idx]} if idx in labels else {}),
                        "v": _node(value, budget)}
                       for idx, value in sorted(section.items())]}
    if note:
        out["note"] = note
    return out


def _locale_rows(root: Obj | None) -> list[list[str]] | None:
    rows = root.leaf.get(0) if root is not None else None
    if not isinstance(rows, list) or not rows:
        return None
    pairs = []
    for row in rows:
        leaf = row.leaf if isinstance(row, Obj) else {}
        if not (isinstance(leaf.get(0), str) and isinstance(leaf.get(1), str)):
            return None
        pairs.append([leaf[0], leaf[1]])
    return pairs


def describe(data: bytes) -> dict:
    """The parsed file as a JSON-able tree. Blocking and unbounded in time - call it
    through ``describe_isolated`` for anything uploaded."""
    if not data:
        raise BinfabViewError("That file is empty.")
    if len(data) > MAX_BYTES:
        raise BinfabViewError("This file is too large to lay out.")
    try:
        pf = parse(data)
    except WireError:
        raise BinfabViewError("This file couldn't be read as a Trove prefab.") from None
    budget = _Budget()
    out: dict = {"kind": pf.kind, "size": len(data)}
    if pf.kind == "entity":
        out["type_id"] = pf.type_id
        out["components"] = [
            {"id": cid, **({"name": name} if (name := _component_name(cid)) else {}),
             "v": _node(obj, budget, _component_labels(cid, len(obj)))}
            for cid, obj in pf.components]
    else:
        rows = _locale_rows(pf.root)
        if rows is not None:
            out["kind"] = "locale"
            out["rows"] = rows[:MAX_NODES]
            budget.truncated = len(rows) > MAX_NODES
        else:
            out["root"] = _node(pf.root, budget)
    out["truncated"] = budget.truncated
    return out


def _limit_memory() -> None:
    if sys.platform != "win32":
        import resource

        resource.setrlimit(resource.RLIMIT_AS, (MEMORY_LIMIT_BYTES, MEMORY_LIMIT_BYTES))


async def describe_isolated(data: bytes) -> dict:
    """``describe`` in a child process: killed past ``TIMEOUT_SECONDS``, and on
    Linux unable to map more than ``MEMORY_LIMIT_BYTES``."""
    if len(data) > MAX_BYTES:
        raise BinfabViewError("This file is too large to lay out.")
    async with _CONCURRENT:
        proc = await asyncio.create_subprocess_exec(
            sys.executable, "-m", "app.trove.decode.view", cwd=str(_ROOT),
            stdin=asyncio.subprocess.PIPE, stdout=asyncio.subprocess.PIPE,
            stderr=asyncio.subprocess.DEVNULL)
        try:
            out, _ = await asyncio.wait_for(proc.communicate(data), TIMEOUT_SECONDS)
        except TimeoutError:
            proc.kill()
            await proc.wait()
            raise BinfabViewError("This file took too long to lay out.") from None
    try:
        result = json.loads(out or b"null")
    except ValueError:
        result = None
    if not isinstance(result, dict):
        raise BinfabViewError("This file couldn't be read as a Trove prefab.")
    if "error" in result:
        raise BinfabViewError(str(result["error"]))
    return result


def _main() -> None:
    # Set by the child on itself: a preexec_fn could deadlock forking a threaded parent.
    _limit_memory()
    try:
        result = describe(sys.stdin.buffer.read())
    except BinfabViewError as exc:
        result = {"error": str(exc)}
    sys.stdout.write(json.dumps(result, separators=(",", ":")))


if __name__ == "__main__":
    _main()
