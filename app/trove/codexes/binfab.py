"""Pure Trove `.binfab` wire reader + locale-table extraction.

Ported from BetterTroveTools `utils/binfab_reader.py` + the locale helpers in
`models/trove/prefab_ally.py`. The format is a self-describing, protobuf-like
field stream (uleb128 keys; `field = key>>4`, `wire = key&0xF`). Trove's schema
isn't in the file, so we read the leading *flat* identity run precisely and
harvest length-prefixed strings everywhere (desync-proof). stdlib only.
"""

from __future__ import annotations

import re
import struct


def read_uleb(data: bytes, pos: int) -> tuple[int, int]:
    result = shift = 0
    while True:
        byte = data[pos]
        pos += 1
        result |= (byte & 0x7F) << shift
        if not (byte & 0x80):
            return result, pos
        shift += 7
        if shift > 63:
            raise ValueError("uleb128 too long")


def read_varint(data: bytes, offset: int) -> tuple[int | None, int]:
    """Like read_uleb but returns (None, offset) on a truncated/overlong run."""
    value = shift = 0
    cursor = offset
    while cursor < len(data) and shift <= 63:
        byte = data[cursor]
        value |= (byte & 0x7F) << shift
        cursor += 1
        if not (byte & 0x80):
            return value, cursor
        shift += 7
    return None, offset


def unzig(value: int) -> int:
    return (value >> 1) ^ -(value & 1)


def content_start(data: bytes) -> int:
    """Offset of the field stream after a `<fmt> 00 <uleb len>` header, else 0."""
    if len(data) >= 4 and data[1] == 0:
        try:
            length, pos = read_uleb(data, 2)
            if pos + length == len(data):
                return pos
        except (IndexError, ValueError):
            pass
    return 0


def _marker_blocks(data: bytes) -> list[dict[tuple[str, int], object]]:
    """Split the flat field stream into its marker-delimited component blocks.

    Each block is ``{(kind, field): value}`` where kind is ``"s"`` (string, wt 8) or
    ``"v"`` (varint, wt 0/2), first-wins per key. Fields before the first marker are
    the entity header and ignored. Unlike ``parse_flat`` this walks the WHOLE stream
    (not just the first component), so a later identity block is reachable."""
    pos = content_start(data)
    n = len(data)
    blocks: list[dict[tuple[str, int], object]] = []
    cur: dict[tuple[str, int], object] | None = None
    while pos < n:
        try:
            key, pos = read_uleb(data, pos)
        except (IndexError, ValueError):
            break
        field, wt = key >> 4, key & 0xF
        try:
            if wt in (0, 2):
                value, pos = read_uleb(data, pos)
                if cur is not None:
                    cur.setdefault(("v", field), unzig(value) if wt == 2 else value)
            elif wt == 4:
                pos += 4
            elif wt == 6:
                pos += 8
            elif wt == 8:
                length, pos = read_uleb(data, pos)
                if pos + length > n:
                    break
                raw = data[pos:pos + length]
                pos += length
                if cur is not None:
                    text = raw.decode("latin1") if raw and all(32 <= b < 127 for b in raw) else None
                    cur.setdefault(("s", field), text)
            else:                              # a wt we don't decode = a component marker
                cur = {}
                blocks.append(cur)
        except (IndexError, struct.error):
            break
    return blocks


def decode_identity(data: bytes) -> dict | None:
    """The identity/metadata component of an entity prefab.

    f1 str = name loc key ($prefabs_..._name) · f2 str = display category ·
    f5 str = description loc key · f14 == 2 => Tradable. None if no identity component
    (recipes / collection tables / locale string-tables have other structures).

    The identity is NOT always the first sub-component: "physical" items (lootboxes,
    pouches, dragon eggs, some mounts) open with transform/model/loot components, so
    their identity block sits further in. We take the first component whose field-1
    string is a ``$``-prefixed loc key. Simple items (most crafting mats/styles) keep
    the identity in the first block, so that fast path is preserved exactly - the scan
    only kicks in when the first block carries no name key (previously => None, and the
    codex fell back to the raw path stem, e.g. "Dragondiamondpouch")."""
    blocks = _marker_blocks(data)
    if not blocks:
        return None
    ident = blocks[0]
    if not ident.get(("s", 1)):
        ident = next((b for b in blocks
                      if isinstance(b.get(("s", 1)), str) and b[("s", 1)].startswith("$")),
                     ident)
    if not ident:
        return None
    trade = ident.get(("v", 14))
    return {
        "name_key": ident.get(("s", 1)),
        "category": ident.get(("s", 2)),
        "desc_key": ident.get(("s", 5)),
        "tradable": (trade == 2) if trade is not None else None,
        "flags": {fnum: val for (kind, fnum), val in ident.items() if kind == "v"},
    }


def harvest_strings(data: bytes, min_len: int = 2, max_len: int = 512) -> list[tuple[int, int, str]]:
    """Desync-proof scan for every length-prefixed ascii string: <key wt8><uleb len>
    <printable bytes>. Returns [(offset, field, text), ...]."""
    out: list[tuple[int, int, str]] = []
    n = len(data)
    for i in range(n - 1):
        try:
            key, j = read_uleb(data, i)
        except (IndexError, ValueError):
            continue
        if (key & 0xF) != 8:
            continue
        try:
            length, k = read_uleb(data, j)
        except (IndexError, ValueError):
            continue
        if min_len <= length <= max_len and k + length <= n:
            raw = data[k:k + length]
            if all(32 <= b < 127 for b in raw):
                out.append((i, key >> 4, raw.decode("ascii")))
    return out


def _real_fields(data: bytes) -> list[tuple[int, int, str]]:
    """``harvest_strings`` scans every byte offset, so it emits spurious sub-strings
    that start INSIDE a real string's bytes (e.g. ``024/...`` one byte into ``2024/...``).
    Keep only the true, non-overlapping field sequence: a field whose bytes start before
    the previous accepted field ended is a phantom and is dropped."""
    out: list[tuple[int, int, str]] = []
    last_end = -1
    for off, field, text in harvest_strings(data):
        try:
            _key, j = read_uleb(data, off)
            length, k = read_uleb(data, j)
        except (IndexError, ValueError):
            continue
        if k >= last_end:                  # doesn't overlap the previous real field
            out.append((off, field, text))
            last_end = k + length
    return out


def extract_rig_refs(data: bytes) -> dict | None:
    """A creature prefab's rig binding read STRUCTURALLY from the wire format (no name
    guessing): the skeleton it uses + each blueprint mesh basename -> ``AP_*`` key.

    The prefab's model component lists its meshes as ``(mesh-path field 0, AP_<key>
    field 1)`` pairs, sitting between the ``<name>.skeleton.gr2`` reference and the
    ``.gsf`` animation-state reference. Empty attach points are a bare ``AP_`` field-1
    (no preceding mesh); ability/particle bindings are an ``AP_`` field-1 followed by a
    field-2 ref and sit OUTSIDE that span - so both are excluded by the WIRE STRUCTURE,
    not by a ``c_`` name prefix (which silently dropped any creature whose meshes aren't
    named ``c_*`` - mobs, targets, harvesting entities, …).

    A prefab can carry SEVERAL creatures, each opened by its own ``.skeleton.gr2``
    reference: a costume bundles the character with its transformed/ultimate form and
    any pets it summons (``dinotamer_coffee`` = dinotamer + dinotamer_ultimate +
    scissorhand + quetzalcoatlus + triceratops). Only the FIRST one is this prefab's
    creature, so a following skeleton reference ends the mesh list exactly as the
    ``.gsf`` does. Without that bound - and these prefabs carry no ``.gsf`` at all - the
    later forms' meshes were read as the first creature's, landing a werewolf head and
    an ultimate torso on the same attach points as the character's own.

    Returns ``{"skeleton": "<stem>", "parts": {"<blueprint basename>": "<ap key>"},
    "refs": {"<blueprint basename>": "<mesh reference as written>"}}`` (all lowercased),
    or None for non-creatures (no skeleton / no mesh bindings).
    """
    rows = _real_fields(data)
    skeleton: str | None = None
    skel_off = gsf_off = next_skel_off = None
    for off, _field, s in rows:
        base = s.rsplit("/", 1)[-1].lower()
        if base.endswith(".skeleton.gr2"):
            if skeleton is None:
                skeleton, skel_off = base[: -len(".skeleton.gr2")], off
            elif next_skel_off is None:
                next_skel_off = off        # a second creature starts here
        elif gsf_off is None and base.endswith(".gsf"):
            gsf_off = off
    if skeleton is None or skel_off is None:
        return None
    ends = [o for o in (gsf_off, next_skel_off) if o is not None and o > skel_off]
    end = min(ends) if ends else len(data)

    parts: dict[str, str] = {}
    refs: dict[str, str] = {}
    for (off, field, s), (_n_off, n_field, n_s) in zip(rows, rows[1:], strict=False):
        if not (skel_off < off < end):
            continue                       # only the model component's mesh list
        if field == 0 and n_field == 1 and n_s.startswith("AP_"):
            ref = s.replace("\\", "/").lower()
            base = ref.rsplit("/", 1)[-1]
            parts[base] = n_s[3:].lower()
            refs[base] = ref              # same last-one-wins rule as `parts`
    if not parts:
        return None
    # `refs` keeps each mesh reference AS THE PREFAB WROTE IT, relative to `blueprints/`.
    # Trove reuses a basename across skins - the Candy Barbarian starter asks for a bare
    # `c_p_candybarbarian_torso` while Demonic Inferno asks for its own
    # `2019/ugc_adventure_box/costumes/candybarbarian_demonicinferno/c_p_candybarbarian_torso`
    # - so the folder is the only thing telling the two apart. `parts` stays keyed on the
    # basename, which is what a mod's file is named and what every other caller matches on.
    return {"skeleton": skeleton, "parts": parts, "refs": refs}


def parse_collection_table(data: bytes) -> list[dict]:
    """Decode a collections/collection_* table into category groups
    `{id, name_key, members: [collection-path]}` by walking harvested strings.

    The group id is the display label that PRECEDES the `$CollectionName_*` key. That
    label is rejected when it carries a `$` anywhere: `harvest_strings` scans every byte
    offset, so a label whose last byte has wire-type 8 in its low nibble (`h`, `x`, `H`,
    `X`, `8`, `(`) is also read as a field key - swallowing the real key's tag as a
    length and yielding a phantom that straddles into the loc key (`Mount Taming Bench`
    -> `" $CollectionName_MountTa"`). Dropping the whole overlapping-field set here is
    NOT an option: it also discards real member paths (mount members 1174 -> 1136)."""
    groups: list[dict] = []
    prev_bare = ""
    cur: dict | None = None
    for _off, _field, s in harvest_strings(data):
        if s.startswith("$CollectionName"):
            cur = {"id": prev_bare or s.removeprefix("$CollectionName_"), "name_key": s, "members": []}
            groups.append(cur)
        elif s.startswith("collections/"):
            if cur is not None:
                cur["members"].append(s)
        elif "/" not in s and "$" not in s:
            prev_bare = s
    return groups


def collection_category_map(data: bytes) -> dict[str, str]:
    """member collection-path (lowercased) -> category id, from a collection table."""
    out: dict[str, str] = {}
    for g in parse_collection_table(data):
        for m in g["members"]:
            out.setdefault(m.replace("\\", "/").lower(), g["id"])
    return out


# --- Locale tables (languages/<locale>/*.binfab) ---------------------------

_PREFIXED_KEY_RE = re.compile(r"\\\$")


def clean_localized_text(text: str) -> str:
    text = text or ""
    text = _PREFIXED_KEY_RE.sub("$", text)
    if text.startswith("$$"):
        text = "$" + text.lstrip("$")
    text = text.replace("\\n", "\n").strip("`").strip()
    if not text.startswith("$") and text.startswith('\\"'):
        text = text[1:]
    return text


def extract_localization_map(content: bytes) -> dict[str, str]:
    """`$key` -> human text from a locale string-table binfab. Keys are followed by
    a 0x18 (field 1, wt 8) separator then a uleb-length value."""
    mapping: dict[str, str] = {}
    cursor = 0
    n = len(content)
    while True:
        start = content.find(b"$", cursor)
        if start < 0:
            break
        end = start + 1
        while end < n:
            b = content[end]
            if not (48 <= b <= 57 or 65 <= b <= 90 or 97 <= b <= 122 or b == 95 or b == 36):
                break
            end += 1
        if end >= n or content[end] != 0x18:
            cursor = start + 1
            continue
        value_length, value_start = read_varint(content, end + 1)
        if value_length is None or value_length < 0 or value_start + value_length > n:
            cursor = start + 1
            continue
        value_end = value_start + value_length
        key = clean_localized_text(content[start:end].decode("ascii", errors="ignore"))
        value = clean_localized_text(content[value_start:value_end].decode("utf-8", errors="ignore"))
        if key.startswith("$") and value and not value.startswith("$"):
            mapping[key] = value
        cursor = value_end
    return mapping
