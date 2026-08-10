"""Trove's character customization table (``prefabs/custom_heads_service.binfab``).

This is the part of a Trove character that isn't a style you equip: the **race** you
picked at creation, and the head, hair and eyes that come with it. The game draws a head
whether or not you own a hat, which is why a costume alone renders faceless without this.

The file is one block per race, each opening with a ``$CustomHead_Race_<name>`` marker and
then listing its pieces as ``$CustomHead_Piece_<blueprint>`` / ``<blueprint>`` pairs, so a
race's pieces are simply the ones between its marker and the next. Nine races ship: Lady,
Guy, Undead, Robot, Dragon, Ghost Pirate, Face Paint, Lunar Lancer and Headless. Hair is
shared by every race (141 styles, most credited to their community author in the usual
``[name]`` suffix); heads and eyes are per race.

Where each piece attaches is the game's own answer, taken from prefabs that bind these
exact meshes:

  head   AP ``head``   ``npc/biped_medium.binfab`` binds ``c_p_head_male_01`` there
  eyes   AP ``face``   three NPC prefabs bind ``c_p_*_eyes`` there (bard, chloromancer,
                       solarion) - which is also why equipping a face style hides them
  hair   AP ``hair``   the attach point every player rig carries under that name

**Colour is deliberately absent.** The pieces are authored in a reference colour the game
recolours at runtime (heads in a skin tone, hair and eyes in red), and that rule is not in
the shipped data: no shader performs it, this table carries no palette, the locale tables
name no swatches, and ``ui/charcustomize.swf`` builds its picker from colours the engine
hands it. So the pieces render in their authored colours rather than in a guessed
approximation of Trove's.
"""

from __future__ import annotations

import re

RACE_RE = re.compile(rb"\$CustomHead_Race_([A-Za-z0-9_]+)")
PIECE_RE = re.compile(rb"\$CustomHead_Piece_([A-Za-z0-9_\[\]]+)")

# Piece kind -> the attach point the game binds that mesh to (see the module docstring).
PIECE_APS = {"head": "head", "hair": "hair", "eyes": "face"}


def piece_kind(blueprint: str) -> str | None:
    """``head`` / ``hair`` / ``eyes`` for a piece blueprint, or None for the ``empty``
    placeholder the Headless race uses."""
    b = (blueprint or "").lower()
    if "_hair" in b:
        return "hair"
    if "_head" in b:
        return "head"
    if "eye" in b:
        return "eyes"
    return None


def parse(data: bytes) -> list[dict]:
    """``[{race, key, pieces: {kind: [blueprint, …]}}]`` in the file's own order.

    A race with no head at all (Headless) still comes back - it is a real choice, and
    the caller decides whether to offer it."""
    races = [(m.start(), m.group(1).decode("ascii", "ignore")) for m in RACE_RE.finditer(data)]
    if not races:
        return []
    bounds = [o for o, _n in races] + [len(data)]
    pieces = [(m.start(), m.group(1).decode("ascii", "ignore")) for m in PIECE_RE.finditer(data)]

    out: list[dict] = []
    for i, (_off, name) in enumerate(races):
        lo, hi = bounds[i], bounds[i + 1]
        grouped: dict[str, list[str]] = {}
        for po, bp in pieces:
            if not (lo <= po < hi):
                continue
            kind = piece_kind(bp)
            if kind and bp not in grouped.setdefault(kind, []):
                grouped[kind].append(bp)
        out.append({
            "race": name,
            "key": name.lower(),
            "name_key": f"$CustomHead_Race_{name}",
            "pieces": grouped,
        })
    return out
