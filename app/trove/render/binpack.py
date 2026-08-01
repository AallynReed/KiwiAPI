"""Binary container for voxel payloads (``KVX1``).

A model's bulk is six parallel arrays per part, and as JSON they are millions of
decimal digits: the browser spends longer in ``JSON.parse`` building boxed JS
numbers than the network spends fetching them. This packs those arrays as raw
typed-array bytes and leaves everything else - sizes, part names, rest matrices,
animation metadata - in a JSON header, so the client gets back the *same object
shape* with typed arrays in place of number arrays and its rendering code doesn't
change at all.

Layout (little-endian throughout)::

    0   'KVX1'
    4   u32  header length (unpadded)
    8   header JSON, then zero-padded to the next 4-byte boundary
    …   the arrays, each 4-byte aligned, in header order

The header carries ``_bin: {path: [offset, count, dtype]}``, where ``path`` is a
dotted route into the payload (``x``, ``parts.3.rgb``) and ``offset`` is relative
to the start of the body - so the header can be written before the body's absolute
position is known. The reader walks each path and drops the typed array in.

An array whose values don't fit its type is simply left in the JSON header, so a
freak model degrades to the old representation instead of being encoded wrong.
"""

from __future__ import annotations

import json
import sys
from array import array

MAGIC = b"KVX1"

# The per-voxel arrays, and the type each is packed as. `rgb` stays a packed
# 0xRRGGBB u32 rather than three bytes: it costs one byte per voxel (which gzip
# mostly eats, the high byte being always zero) and keeps every consumer's
# `(rgb >> 16) & 255` working unchanged.
_ARRAYS: dict[str, str] = {
    "x": "i16", "y": "i16", "z": "i16",
    "rgb": "u32", "kind": "u8", "level": "u8",
}
_TYPECODE = {"i16": "h", "u32": "I", "u8": "B"}


def _pack(values: list, dtype: str) -> bytes | None:
    """Raw little-endian bytes for one array, or None if a value won't fit the
    type (the caller then leaves that array in the JSON header)."""
    try:
        buf = array(_TYPECODE[dtype], values)
    except (OverflowError, TypeError, ValueError):
        return None
    if sys.byteorder == "big":
        buf.byteswap()
    return buf.tobytes()


def _split(payload: dict) -> tuple[dict, list[tuple[str, list, str]]]:
    """``(header, [(path, values, dtype)])`` - the payload with its bulk arrays
    lifted out. Handles both shapes: a single blueprint (arrays at the top level)
    and an assembled creature (arrays per part)."""
    out: list[tuple[str, list, str]] = []
    parts = payload.get("parts")
    if isinstance(parts, list):                       # assembled creature
        header = dict(payload)
        header["parts"] = [{k: v for k, v in p.items() if k not in _ARRAYS} for p in parts]
        for i, part in enumerate(parts):
            for name, dtype in _ARRAYS.items():
                if isinstance(part.get(name), list):
                    out.append((f"parts.{i}.{name}", part[name], dtype))
        return header, out

    header = {k: v for k, v in payload.items() if k not in _ARRAYS}
    for name, dtype in _ARRAYS.items():
        if isinstance(payload.get(name), list):
            out.append((name, payload[name], dtype))
    return header, out


def encode(payload: dict) -> bytes:
    """Pack a viewer payload into the ``KVX1`` container."""
    header, arrays = _split(payload)

    body = bytearray()
    index: dict[str, list] = {}
    for path, values, dtype in arrays:
        raw = _pack(values, dtype)
        if raw is None:                               # out of range -> keep it as JSON
            _set_path(header, path, values)
            continue
        index[path] = [len(body), len(values), dtype]
        body += raw
        body += b"\0" * (-len(body) % 4)              # keep the next array aligned

    if index:
        header["_bin"] = index
    head = json.dumps(header, separators=(",", ":")).encode("utf-8")

    out = bytearray(MAGIC)
    out += len(head).to_bytes(4, "little")
    out += head
    out += b"\0" * (-len(out) % 4)                    # body starts 4-byte aligned
    out += body
    return bytes(out)


def _set_path(obj: dict, path: str, value) -> None:
    """Write ``value`` at a dotted path, taking numeric segments as list indices."""
    parts = path.split(".")
    cur = obj
    for seg in parts[:-1]:
        cur = cur[int(seg)] if isinstance(cur, list) else cur[seg]
    last = parts[-1]
    if isinstance(cur, list):
        cur[int(last)] = value
    else:
        cur[last] = value


def decode(blob: bytes) -> dict:
    """Unpack a ``KVX1`` container back into the plain payload. Python-side mirror
    of the browser reader - used by the tests, not on any request path."""
    if blob[:4] != MAGIC:
        raise ValueError("not a KVX1 payload")
    head_len = int.from_bytes(blob[4:8], "little")
    header = json.loads(blob[8:8 + head_len].decode("utf-8"))
    body = 8 + head_len + (-(8 + head_len) % 4)
    index = header.pop("_bin", {})
    for path, (offset, count, dtype) in index.items():
        start = body + offset
        buf = array(_TYPECODE[dtype])
        buf.frombytes(blob[start:start + count * buf.itemsize])
        if sys.byteorder == "big":
            buf.byteswap()
        _set_path(header, path, list(buf))
    return header


__all__ = ["MAGIC", "decode", "encode"]
