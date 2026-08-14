"""The Blueprint Editor's engine: inspect a ``.blueprint``, apply edits, re-encode.

Stateless by design. A file arrives with the request, the answer goes back with the
response, and nothing is kept -- on save the browser posts the original bytes back
alongside the edits, so the server never has to hold a session between the two.

Edits are indexed against the decoded voxel order, which the codec keeps stable
(v5 by linear index, v3/v4 by file order). The same file always decodes to the same
sequence, so an index means the same voxel on the save request as it did on the
inspect response -- and an untouched voxel is written back byte for byte, because the
encoder is handed the original ``(type, w)`` it decoded.
"""
from __future__ import annotations

from app.trove.blueprint import codec, lint, materials, qb
from app.trove.blueprint import transform as bp_transform
from app.trove.mods_hub import workshop as mods_workshop
from app.trove.render.voxel import KIND_CODE, material_for

# The editor holds every voxel's material in browser memory and re-meshes on each
# edit, so it caps lower than the read-only viewer's 250k.
EDITOR_VOXEL_CAP = 150_000

# A blueprint is a small file: the largest of the 72,584 in the live game catalogue is
# 515 KB. 4 MB is eight times that - room for anything a modder builds, while still
# rejecting a mis-picked archive early instead of inflating it.
MAX_BLUEPRINT_BYTES = 4 * 1024 * 1024

# A .qb set is four uncompressed-ish grids rather than one packed model, so it is much
# bulkier than the blueprint it compiles to.
MAX_QB_BYTES = 48 * 1024 * 1024


class EditorError(ValueError):
    """A blueprint the editor can't open, or an edit it won't apply."""


def _decode_or_raise(raw: bytes) -> codec.DecodedBlueprint:
    if len(raw) > MAX_BLUEPRINT_BYTES:
        raise EditorError("That file is too large to be a Trove blueprint.")
    if codec.is_empty_blueprint(raw):
        raise EditorError("This blueprint is an empty placeholder - there's nothing to edit.")
    try:
        return codec.decode_full(raw)
    except codec.BlueprintError as exc:
        raise EditorError(str(exc)) from exc


def inspect(raw: bytes, *, name: str = "blueprint") -> dict:
    """Decode a blueprint into the editor payload.

    On top of the arrays the 3D viewers already consume (``x/y/z/rgb/kind/level/spec``)
    this carries the raw ``type`` and ``w`` per voxel, plus an ``edit`` flag marking
    which voxels the material palette is allowed to rewrite. The client needs all three
    to show a voxel's real material and to grey out the ones it must not touch.

    CPU-bound - call via ``asyncio.to_thread``."""
    decoded = _decode_or_raise(raw)
    voxels = decoded.voxels
    if len(voxels) > EDITOR_VOXEL_CAP:
        raise EditorError(
            f"This model has {len(voxels):,} voxels, past the editor's "
            f"{EDITOR_VOXEL_CAP:,} limit. It can still be viewed in 3D.")

    xs: list[int] = []; ys: list[int] = []; zs: list[int] = []
    rgb: list[int] = []; kind: list[int] = []; level: list[int] = []; spec: list[int] = []
    types: list[int] = []; ws: list[int] = []; editable: list[int] = []
    paintable: list[int] = []
    procedural = placeholders = locked = 0
    seen: dict[tuple[int, int], int] = {}

    for v in voxels:
        vt, w = int(v["type"]), int(v["w"])
        r, g, b, k, lv, sp = material_for(v["r"], v["g"], v["b"], w, vt)
        xs.append(v["x"]); ys.append(v["y"]); zs.append(v["z"])
        rgb.append((r << 16) | (g << 8) | b)
        kind.append(KIND_CODE.get(k, 0)); level.append(lv); spec.append(sp)
        types.append(vt); ws.append(w)
        can_edit = materials.is_editable(vt)
        editable.append(1 if can_edit else 0)
        if not can_edit:
            locked += 1
        if vt == materials.PLACEHOLDER_TYPE:
            placeholders += 1
        # A procedural voxel's stored colour is a placeholder the game overwrites at
        # runtime, so painting it would change the file and nothing in game. Marked
        # separately from `edit`: that governs the MATERIAL, this governs the COLOUR,
        # and a voxel can be one without the other.
        is_proc = materials.is_procedural(vt, v["r"], v["g"], v["b"])
        paintable.append(0 if is_proc else 1)
        if is_proc:
            procedural += 1
        seen[(vt, w)] = seen.get((vt, w), 0) + 1

    entities = codec.parse_entity_section(decoded.entity_blob)
    attach = codec.attachment_point(decoded)
    return {
        "name": name,
        "count": len(voxels),
        "size": list(decoded.size),
        "version": decoded.version,
        # Where the game grips or seats this model. Outside the box for a hat or mask
        # (the gap is the head), inside the handle for a weapon, absent on v3/v4.
        "attachment": list(attach) if attach else None,
        "creation_types": list(lint.CREATION_TYPES),
        "transforms": [{"op": op, "label": bp_transform.OPERATION_LABELS[op]}
                       for op in bp_transform.OPERATIONS],
        "x": xs, "y": ys, "z": zs, "rgb": rgb,
        "kind": kind, "level": level, "spec": spec,
        "type": types, "w": ws, "edit": editable, "paint": paintable,
        "materials": [
            {"type": t, "w": w, "count": n, "label": materials.describe(t, w),
             "editable": materials.is_editable(t)}
            for (t, w), n in sorted(seen.items(), key=lambda kv: -kv[1])
        ],
        "stats": {
            "voxels": len(voxels),
            "locked": locked,
            "procedural": procedural,
            "placeholders": placeholders,
            "entities": entities["count"],
        },
        "palette": materials.palette(),
    }


def _coerce_edits(edits, total: int) -> dict[int, dict]:
    """Validate the incoming edit list into ``{voxel index: change}``.

    Rejects rather than clamps anything structural: an out-of-range index or a
    non-palette type means the client is out of step with the file it posted, and
    applying a best guess would write a model the user never saw."""
    if not isinstance(edits, list):
        raise EditorError("The edit list wasn't understood.")
    if len(edits) > total:
        raise EditorError("More edits than the blueprint has voxels.")
    out: dict[int, dict] = {}
    for entry in edits:
        if not isinstance(entry, dict):
            raise EditorError("The edit list wasn't understood.")
        try:
            idx = int(entry["i"])
        except (KeyError, TypeError, ValueError):
            raise EditorError("An edit is missing its voxel index.") from None
        if not 0 <= idx < total:
            raise EditorError("An edit points outside the blueprint.")
        change: dict = {}
        if "type" in entry or "w" in entry:
            # Both or neither: ``w`` means a different thing per type (specular finish
            # on a solid, opacity on glass), so half a material is not a material.
            try:
                raw_type, raw_w = int(entry["type"]), int(entry["w"])
            except (KeyError, TypeError, ValueError):
                raise EditorError("A material edit needs both a type and a w.") from None
            try:
                change["type"], change["w"] = materials.validate_material(raw_type, raw_w)
            except ValueError as exc:
                raise EditorError(str(exc)) from None
        if "rgb" in entry:
            try:
                packed = int(entry["rgb"])
            except (TypeError, ValueError):
                raise EditorError("An edit has an invalid colour.") from None
            if not 0 <= packed <= 0xFFFFFF:
                raise EditorError("An edit has an invalid colour.")
            change["rgb"] = packed
        if change:
            out[idx] = {**out.get(idx, {}), **change}
    return out


def transform(raw: bytes, edits, ops) -> tuple[bytes, dict]:
    """Rotate and/or mirror the edited model, returning a new blueprint.

    Edits are baked in first, so the result is one file the page can reopen - which it
    must, because a transform renumbers every voxel and the caller's edit indices stop
    meaning anything the moment the axes move. CPU-bound - call via
    ``asyncio.to_thread``."""
    edited, _ = apply_edits(raw, edits)
    decoded = codec.decode_full(edited)
    before = decoded.size
    try:
        moved = bp_transform.apply(decoded, ops)
        data = codec.encode(moved.voxels, version=moved.version, pos=moved.pos,
                            entity_blob=moved.entity_blob, offset=moved.offset,
                            size=moved.size)
    except (bp_transform.TransformError, codec.BlueprintError) as exc:
        raise EditorError(str(exc)) from exc
    attach = codec.attachment_point(moved)
    return data, {
        "applied": list(ops),
        "size_before": list(before),
        "size_after": list(moved.size),
        "attachment": list(attach) if attach else None,
        "entities": codec.parse_entity_section(moved.entity_blob)["count"],
    }


def export_qb(raw: bytes, edits, *, stem: str = "model") -> tuple[bytes, dict]:
    """Export the edited model as the four authoring ``.qb`` files, zipped.

    Edits are applied first, for the same reason ``check`` applies them: the export is
    of the model as it stands, not as it was opened. CPU-bound - call via
    ``asyncio.to_thread``."""
    edited, _ = apply_edits(raw, edits)
    try:
        built = qb.from_blueprint(codec.decode_full(edited), stem=stem)
    except (qb.QbError, codec.BlueprintError) as exc:
        raise EditorError(str(exc)) from exc
    archive = mods_workshop.to_zip(list(built["files"].items()))
    return archive, {"notes": built["notes"], "attachment": built["attachment"],
                     "size": built["size"], "files": sorted(built["files"])}


def import_qb(files: dict[str, bytes]) -> tuple[bytes, dict]:
    """Compile a ``.qb`` (plus any material maps supplied with it) into a blueprint.

    CPU-bound - call via ``asyncio.to_thread``."""
    total = sum(len(v) for v in files.values())
    if total > MAX_QB_BYTES:
        raise EditorError("Those .qb files are too large to compile.")
    try:
        return qb.to_blueprint(files)
    except (qb.QbError, codec.BlueprintError) as exc:
        raise EditorError(str(exc)) from exc


def check(raw: bytes, edits, kind: str = "other") -> dict:
    """Run the Trove Creations checks against the model *as it would be saved*.

    Edits are applied first on purpose: linting the file that arrived would grade work
    the user has already moved on from, and the question they're asking is "is what I'm
    about to download acceptable". CPU-bound - call via ``asyncio.to_thread``."""
    edited, _ = apply_edits(raw, edits)
    return lint.check(codec.decode_full(edited), kind)


def apply_edits(raw: bytes, edits) -> tuple[bytes, dict]:
    """Apply material/colour edits to a blueprint and re-encode it.

    Returns ``(bytes, summary)``. The version, origin, bounding box and entity section
    all come straight off the decode, so the saved file sits exactly where the original
    did and its decos are untouched. With an empty edit list the output is byte-identical
    to the input for v3/v4, and payload-identical for v5 (only the zlib container differs).

    CPU-bound - call via ``asyncio.to_thread``."""
    decoded = _decode_or_raise(raw)
    changes = _coerce_edits(edits, len(decoded.voxels))

    out: list[dict] = []
    recoloured = rematerialised = ignored = 0
    for i, v in enumerate(decoded.voxels):
        change = changes.get(i)
        if not change:
            out.append(v)
            continue
        nv = dict(v)
        if "type" in change:
            # Refuse on the ORIGINAL type: the palette may only rewrite a voxel whose
            # meaning the editor showed the user. A placeholder or terrain voxel keeps
            # what it had rather than being converted into a solid block.
            if not materials.is_editable(v["type"]):
                ignored += 1
            else:
                nv["type"], nv["w"] = change["type"], change["w"]
                rematerialised += 1
        if "rgb" in change:
            # The game tints procedural voxels itself and ignores what's stored, so
            # writing a colour there would change the file and nothing the player
            # sees. Refused for the same reason a material change is: the editor
            # doesn't make edits that quietly don't happen.
            if materials.is_procedural(v["type"], v["r"], v["g"], v["b"]):
                ignored += 1
            else:
                packed = change["rgb"]
                nv["r"] = (packed >> 16) & 0xFF
                nv["g"] = (packed >> 8) & 0xFF
                nv["b"] = packed & 0xFF
                recoloured += 1
        out.append(nv)

    try:
        data = codec.encode(out, version=decoded.version, pos=decoded.pos,
                            entity_blob=decoded.entity_blob, offset=decoded.offset,
                            size=decoded.size)
    except codec.BlueprintError as exc:
        raise EditorError(str(exc)) from exc
    return data, {
        "voxels": len(out),
        "recoloured": recoloured,
        "rematerialised": rematerialised,
        "ignored": ignored,
        "version": decoded.version,
        "bytes": len(data),
    }
