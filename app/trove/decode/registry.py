"""The decoders, and what identifies a version of each one's code."""
from __future__ import annotations

import hashlib
import json
from pathlib import Path
from types import ModuleType

from app.trove.decode import (
    ally_abilities,
    class_abilities,
    class_levels,
    common,
    delve_modifiers,
    gem_abilities,
    pvp_stat_ranges,
    ring_abilities,
)

DECODERS: dict[str, ModuleType] = {
    m.__name__.rsplit(".", 1)[-1]: m
    for m in (class_levels, class_abilities, ally_abilities, gem_abilities,
              ring_abilities, pvp_stat_ranges, delve_modifiers)
}


def code_version(module: ModuleType) -> str:
    """Hash of the decoder's source plus the shared helpers, so any edit to either
    reruns it on the next check without anyone bumping a number."""
    h = hashlib.sha1()
    for m in (module, common):
        h.update(Path(m.__file__ or "").read_bytes())
    return h.hexdigest()[:12]


def dump(module: ModuleType, data) -> str:
    """The file text, in the format the repo copy already uses."""
    text = json.dumps(data, indent=module.INDENT, ensure_ascii=False)
    return text + "\n" if module.FINAL_NEWLINE else text
