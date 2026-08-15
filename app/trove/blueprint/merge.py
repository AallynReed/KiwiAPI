"""Flatten a stack of blueprints into one.

Trove has no layered model format, so the file that comes out is always a single
``.blueprint``. Layering is an editing-time idea, and that is exactly what makes it
useful: **nothing is destroyed while you work**. A model laid over another hides what is
underneath rather than replacing it, so sliding it back off brings the covered voxels
straight back. The stack is only resolved at output, and the stacking ORDER is what
decides which voxel wins a shared cell - the topmost layer that has one.

**Alignment is the hard part.** Two models are two independent grids, each starting at
(0, 0, 0), and stacking them corner-to-corner is almost never what anyone means. The
default is Troxel's rule: line the ATTACHMENT POINTS up. Both models are already defined
relative to where the game holds them, so a sword hilt meets a sword hilt and a hat brim
meets a hat brim without anyone counting voxels. Centre and corner are there for models
that have no attachment point (v3/v4, decorations), and a manual nudge rides on top of
whichever is chosen.

Everything a move has to carry, flattening has to carry for every layer:

* **The box** grows to hold all of them, and if any lands at a negative coordinate the
  whole grid shifts, because a blueprint's grid starts at zero.
* **The attachment point** of the BASE survives as the result's own - the output is the
  base model with things added, so it keeps its grip.
* **Placed decos** from every layer are translated and concatenated, so a cornerstone
  flattened with another keeps the furniture from both.

Materials cross over untouched, including the ones the palette refuses to edit: this is
a file-level operation, not a paint stroke, so a deco placeholder stays a deco
placeholder rather than being reinterpreted as a solid block.
"""
from __future__ import annotations

from app.trove.blueprint import codec

ALIGN_MODES = ("attachment", "centre", "corner")
ALIGN_LABELS = {
    "attachment": "Line up the attachment points",
    "centre": "Line up the centres",
    "corner": "Line up the corners",
}

# Two models could in principle union into something enormous; this is the same ceiling
# the editor draws at, checked before anything is built.
MAX_MERGED_VOXELS = 150_000


class MergeError(ValueError):
    """Two blueprints that can't be combined as asked."""


def align_offset(base: codec.DecodedBlueprint, over: codec.DecodedBlueprint,
                 mode: str) -> tuple[int, int, int]:
    """Where the incoming model's (0,0,0) lands in the base model's grid."""
    if mode == "corner":
        return (0, 0, 0)
    if mode == "centre":
        # (size - 1) / 2 is the middle VOXEL, not the middle of the box - a voxel sits
        # on its coordinate rather than in the cell after it.
        return tuple(
            round((base.size[i] - 1) / 2 - (over.size[i] - 1) / 2) for i in range(3)
        )  # type: ignore[return-value]
    a, b = codec.attachment_point(base), codec.attachment_point(over)
    if a is None or b is None:
        raise MergeError(
            "Lining up the attachment points needs both models to have one, and at "
            "least one of these doesn't (older v3/v4 blueprints don't store one). "
            "Line up the centres or the corners instead.")
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def flatten(base: codec.DecodedBlueprint, layers) -> tuple[codec.DecodedBlueprint, dict]:
    """Resolve a stack into one blueprint.

    ``layers`` is ``[(decoded, mode, offset)]`` in STACKING ORDER, bottom to top, with
    the base beneath all of them. A cell claimed by more than one is taken by the
    highest layer that has it - that ordering is the only thing that decides a conflict,
    and it is decided here at output rather than by anything the editor did earlier.

    CPU-bound - call via ``asyncio.to_thread``."""
    if not base.voxels:
        raise MergeError("The base blueprint needs at least one voxel.")

    total = len(base.voxels) + sum(len(d.voxels) for d, _, _ in layers)
    if total > MAX_MERGED_VOXELS:
        raise MergeError(
            f"Together those are more than {MAX_MERGED_VOXELS:,} voxels, past what the "
            f"editor can hold.")

    placements: list[tuple[int, int, int]] = []
    cells: dict[tuple[int, int, int], dict] = {}
    for v in base.voxels:
        cells[(v["x"], v["y"], v["z"])] = v

    # Bottom to top, so the last writer at a cell is the topmost layer that has one.
    hidden = 0
    for decoded, mode, offset in layers:
        if mode not in ALIGN_MODES:
            raise MergeError(f"Unknown alignment: {mode}.")
        if not decoded.voxels:
            raise MergeError("A layer has no voxels.")
        ax, ay, az = align_offset(base, decoded, mode)
        t = (ax + offset[0], ay + offset[1], az + offset[2])
        placements.append(t)
        for v in decoded.voxels:
            key = (v["x"] + t[0], v["y"] + t[1], v["z"] + t[2])
            if key in cells:
                hidden += 1
            cells[key] = {**v, "x": key[0], "y": key[1], "z": key[2]}

    # A blueprint's grid starts at zero, so anything that landed negative moves the lot.
    mnx = min(0, min(k[0] for k in cells))
    mny = min(0, min(k[1] for k in cells))
    mnz = min(0, min(k[2] for k in cells))
    shift = (-mnx, -mny, -mnz)

    voxels = []
    for (x, y, z), v in cells.items():
        voxels.append({**v, "x": x + shift[0], "y": y + shift[1], "z": z + shift[2]})
    # Ordered like a fresh decode so the encoder writes a tidy, deterministic file.
    voxels.sort(key=lambda v: (v["y"], v["z"], v["x"]))

    size = (max(max(v["x"] for v in voxels) + 1, base.size[0] + shift[0]),
            max(max(v["y"] for v in voxels) + 1, base.size[1] + shift[1]),
            max(max(v["z"] for v in voxels) + 1, base.size[2] + shift[2]))

    entity_blob, entity_note = _merge_entities(
        base, [(d, t) for (d, _, _), t in zip(layers, placements, strict=True)], shift)

    # The result is the base with something added, so it keeps the base's grip.
    attach = codec.attachment_point(base)
    if attach is not None:
        attach = (attach[0] + shift[0], attach[1] + shift[1], attach[2] + shift[2])
        pos = (attach[0] - (size[0] - 1), -attach[1], -attach[2])
    elif base.version == 5:
        pos = (-(size[0] // 2), base.pos[1], -(size[2] // 2))
    else:
        pos = (-(size[0] // 2), -(size[1] // 2), -(size[2] // 2))

    offset_out = ((-(size[0] // 2), -(size[1] // 2), -(size[2] // 2))
                  if base.version in (3, 4) else base.offset)

    merged = codec.DecodedBlueprint(base.version, size, pos, voxels, entity_blob, offset_out)
    return merged, {
        "voxels": len(voxels),
        "from_base": len(base.voxels),
        "from_layers": sum(len(d.voxels) for d, _, _ in layers),
        "layers": len(layers),
        "hidden": hidden,
        "placed_at": [list(t) for t in placements],
        "size": list(size),
        "attachment": list(attach) if attach else None,
        "entities": entity_note,
    }


def _merge_entities(base, placed, shift) -> tuple[bytes, str]:
    """Concatenate every layer's entity list, each moved into the flattened grid.

    If any section can't be read exactly we keep the base's untouched and say what was
    left behind - quietly dropping someone's furniture is not an option, and neither is
    writing a section we'd be guessing at."""
    b = codec.read_entity_records(base.entity_blob)
    reads = [codec.read_entity_records(d.entity_blob) for d, _ in placed]
    if b is None or any(r is None for r in reads):
        return base.entity_blob, ("the placed objects couldn't be read on one of the "
                                  "models, so only the base's were kept")
    brec, tail = b
    out = [(x + shift[0], y + shift[1], z + shift[2], sub) for (x, y, z, sub) in brec]
    carried = 0
    for (_, t), read in zip(placed, reads, strict=True):
        recs, _ = read           # type: ignore[misc]
        carried += len(recs)
        out += [(x + t[0] + shift[0], y + t[1] + shift[1], z + t[2] + shift[2], sub)
                for (x, y, z, sub) in recs]
    if not out:
        return base.entity_blob, ""
    note = (f"{carried} placed object{'s' if carried != 1 else ''} came across too"
            if carried else "")
    return codec.write_entity_records(out, tail), note
