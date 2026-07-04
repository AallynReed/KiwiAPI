"""Recipe catalogue membership from `prefabs/collections/collection_recipe.binfab`.

The catalogue's reliable signal is MEMBERSHIP + source order: which recipes are part
of the current recipe collection lane. It's additive, never destructive - a recipe
absent from the catalogue keeps its parsed identity (source absence is not deletion),
it's just flagged `in_catalogue=False` (the handoff's source-only recipe).

Like the profession/bench prefabs, this file packs recipes as BARE `recipe_*` tokens
with binary framing bytes glued on - so we match each token against the authoritative
recipe-id set (the `recipes/` stems) and trim the tail to the longest real id. That
lifts membership from ~98% (raw tokens) to ~100%.

The file also carries category/group labels, but they're interleaved with the packed
token bytes and can't be attributed to a member reliably, so we intentionally DON'T
emit a catalogue category (the output-derived `recipe.category` is the trusted one).

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import harvest_strings

_RECIPE_TOKEN_RE = re.compile(r"recipe_[a-z0-9_]+")


def parse_recipe_catalogue(data: bytes, known_ids: set[str]) -> dict[str, dict]:
    """`collection_recipe.binfab` bytes -> `{recipe_id: {order, in_catalogue, duplicate?}}`.

    Each `recipe_*` token is trimmed from the tail to the longest id that actually
    exists (`known_ids`), so glued framing bytes are stripped and tokens matching no
    real recipe are dropped. First occurrence sets the source order; a repeated id is
    flagged `duplicate` (handoff: treat catalogue dupes as review-only)."""
    out: dict[str, dict] = {}
    order = 0
    for _off, _field, s in harvest_strings(data):
        for m in _RECIPE_TOKEN_RE.finditer(s.lower()):
            tok = m.group(0)
            while tok and tok not in known_ids:      # trim glued framing bytes
                tok = tok[:-1]
            if not tok:
                continue
            if tok in out:
                out[tok]["duplicate"] = True
            else:
                out[tok] = {"order": order, "in_catalogue": True}
                order += 1
    return out
