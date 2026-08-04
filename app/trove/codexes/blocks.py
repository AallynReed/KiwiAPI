"""The placeable/block model table (`prefabs/blocks/blocks.binfab`).

A placeable prefab names no model of its own - it is ~200 bytes carrying a locale key,
a category and its own path. THIS is where the game keeps the mapping, one record per
placeable: `<prefab path> <prefab path> <model name>` on wire fields 0, 0, 1, where the
model is a blueprint basename (no extension, no `[author]` credit).

It is authoritative, so it wins over the name-shaped inference in
`extract.conventional_blueprint` wherever it has an entry - and it needs to: the
convention resolved every `deco_arrow_*_left` to its RIGHT-facing sibling and back
again, because a name alone cannot tell two mirrored models apart.

Pure + stdlib.
"""

from __future__ import annotations

from app.trove.codexes.binfab import _real_fields

# Records key off a placeable/block prefab path; anything else in the table is framing.
_PATH_PREFIXES = ("placeable/", "block/")


def parse_block_models(content: bytes) -> dict[str, str]:
    """``prefab path (lowercased) -> model name``.

    The path repeats on field 0 (id + display path) and the model follows on field 1;
    a record with no field 1 simply has no model, and is skipped rather than paired
    with the next record's path.
    """
    models: dict[str, str] = {}
    pending: str | None = None
    for _off, field, text in _real_fields(content):
        if field == 0 and text.lower().startswith(_PATH_PREFIXES):
            pending = text.lower()
        elif field == 1 and pending:
            models.setdefault(pending, text)
            pending = None
    return models
