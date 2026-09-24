"""The decoders, and what identifies a version of each one's code."""
from __future__ import annotations

import ast
import hashlib
import json
import sys
from pathlib import Path
from types import ModuleType

from app.trove.decode import (
    ability,
    ally_abilities,
    badges,
    class_abilities,
    class_levels,
    collectibles,
    common,
    companions,
    cosmetics,
    delve_modifiers,
    fields,
    fish,
    gem_abilities,
    mementos,
    mount_abilities,
    npcs,
    placeables,
    pvp_stat_ranges,
    quests,
    recipes,
    ring_abilities,
    titles,
    wire,
)

_PACKAGE = "app.trove.decode."
DECODERS: dict[str, ModuleType] = {
    m.__name__.rsplit(".", 1)[-1]: m
    for m in (class_levels, class_abilities, ally_abilities, gem_abilities,
              ring_abilities, mount_abilities, pvp_stat_ranges, delve_modifiers,
              companions, badges, fish, mementos, recipes, collectibles,
              cosmetics, titles, npcs, placeables, quests)
}


def _uses(module: ModuleType) -> list[ModuleType]:
    """The ``app.trove.decode`` modules ``module`` draws on, transitively (a decoder
    borrowing another decoder's helpers must rerun when those change)."""
    found: dict[str, ModuleType] = {}

    def visit(m: ModuleType) -> None:
        for node in ast.walk(ast.parse(Path(m.__file__ or "").read_text(encoding="utf-8"))):
            names = [node.module or ""] if isinstance(node, ast.ImportFrom) else []
            if isinstance(node, ast.ImportFrom) and node.module == _PACKAGE.rstrip("."):
                names = [f"{_PACKAGE}{a.name}" for a in node.names]
            for name in names:
                dep = sys.modules.get(name)
                if dep is not None and name.startswith(_PACKAGE) and name not in found:
                    found[name] = dep
                    visit(dep)

    visit(module)
    return [found[k] for k in sorted(found)]


def code_version(module: ModuleType) -> str:
    """Hash of the decoder's source plus everything it uses from this package, so
    any edit to either reruns it on the next check without anyone bumping a number."""
    h = hashlib.sha1()
    seen = []
    for m in (module, common, wire, fields, ability, *_uses(module)):
        if m not in seen:
            seen.append(m)
            h.update(Path(m.__file__ or "").read_bytes())
    return h.hexdigest()[:12]


def dump(module: ModuleType, data) -> str:
    """The file text, in the format the repo copy already uses."""
    text = json.dumps(data, indent=module.INDENT, ensure_ascii=False)
    return text + "\n" if module.FINAL_NEWLINE else text
