"""Structured `.binfab` reader - a port of the client's own property reader.

Reverse-engineered from Trove_x64.exe (reader FUN_14091d070, skipper FUN_14091cd30).

A field key is a zigzag varint32 ``k``: ``wt = k & 7``, ``idx = k >> 3``, and when
``wt == 7`` the key is extended: ``wt = k & 0x3f``, ``idx = k >> 6``.

====  =========================================================================
wt    value
====  =========================================================================
0     zigzag varint
1     unsigned varint
2     float32
3     8 bytes (double or int64 - the wire does not say which)
4     uvarint length + UTF-8 bytes
6     packed: a key whose idx is the count and wt the element type (vector3 =
      three float32s)
0x0f  END of the current section
0x17  object
0x1f  array: header key (count, element wt), then zigzag varint64 ops
      ``slot<<3 | op`` - op 2 carries a value, op 0 only creates the slot,
      op 4 ends the array
0x27  map (varint64 ops, same op codes, the key in place of the slot)
0x2f  custom: uvarint length + opaque bytes
0x37  bigmap (op byte + zigzag varint64 key)
0x3f  stringmap (op byte + string key)
====  =========================================================================

An object is one END-terminated section per class in its inheritance chain, root
class first. How many sections an object has is not on the wire - the game knows
from the type - so this reader works it out: at every END it considers both "the
object is done" and "another section follows", and keeps the reading that fills
the enclosing length exactly with the fewest sections. The search is memoised per
start offset, so it stays close to linear rather than exponential.

Entity prefabs are ``zz type id, zz varint64, uvarint size`` followed by
``(zz component id, uvarint length, object)`` records. Everything else (class
tables, recipes) is one bare object.
"""
from __future__ import annotations

import struct
import sys
from dataclasses import dataclass, field
from typing import Any

END, OBJ, ARR, MAP, CUSTOM, BIGMAP, STRMAP = 0x0F, 0x17, 0x1F, 0x27, 0x2F, 0x37, 0x3F
_EMPTY = object()                   # an array slot created but given no value
MAX_SECTIONS = 8
# No class declares anywhere near this many properties; a field key past it is
# misaligned bytes. (Array headers reuse the index as a count, so only fields check.)
MAX_FIELD = 255
COUNT_PENALTY = 1000
GARBAGE_PENALTY = 100_000


def _garbled(text: str) -> bool:
    return "\ufffd" in text or any(ord(c) < 0x20 and c not in "\t\n\r" for c in text)


class WireError(ValueError):
    pass


class Repeated(list):
    """Values of one index written back to back (classes that reuse an index)."""


class Obj(list):
    """An object: its sections (``dict[field index, value]``), root class first."""

    @property
    def leaf(self) -> dict:
        """The most-derived class's fields - where a component's own data lives."""
        return self[-1] if self else {}

    def get(self, idx: int, default: Any = None) -> Any:
        """A field from the leaf section."""
        return self.leaf.get(idx, default)


@dataclass
class Prefab:
    kind: str                                     # "entity" or "object"
    type_id: int | None = None
    components: list[tuple[int, Obj]] = field(default_factory=list)
    root: Obj | None = None

    def component(self, cid: int) -> Obj | None:
        return next((o for c, o in self.components if c == cid), None)

    def component_ids(self) -> list[int]:
        return [c for c, _ in self.components]


class _Reader:
    """Reads ``data[:end]``. Every ``*_ends`` method maps each possible end offset of
    the thing starting at ``p`` to its value, memoised per start offset."""

    def __init__(self, data: bytes, end: int):
        self.d, self.e = data, end
        self._memo: dict[tuple[str, int], dict[int, Any]] = {}

    def uvar(self, p: int, limit: int = 35) -> tuple[int, int]:
        v = s = 0
        d, e = self.d, self.e
        while True:
            if p >= e or s > limit:
                raise WireError("varint")
            b = d[p]
            p += 1
            v |= (b & 0x7F) << s
            s += 7
            if b < 0x80:
                return v, p

    def zz(self, p: int, limit: int = 35) -> tuple[int, int]:
        v, p = self.uvar(p, limit)
        return (v >> 1) ^ -(v & 1), p

    def key(self, p: int) -> tuple[int, int, int]:
        k, p = self.zz(p)
        if k < 0:
            raise WireError("negative key")
        wt, idx = k & 7, k >> 3
        if wt == 7:
            wt, idx = k & 0x3F, k >> 6
        return idx, wt, p

    def take(self, p: int, n: int) -> bytes:
        if n < 0 or p + n > self.e:
            raise WireError("eof")
        return self.d[p:p + n]

    def scalar(self, p: int, wt: int) -> tuple[Any, int]:
        if wt == 0:
            return self.zz(p)
        if wt == 1:
            return self.uvar(p)
        if wt == 2:
            return round(struct.unpack("<f", self.take(p, 4))[0], 6), p + 4
        if wt == 3:
            raw = self.take(p, 8)
            as_int = struct.unpack("<q", raw)[0]
            # -1, 0 and small counts are int64; anything with a real exponent byte is a double.
            if raw[7] in (0, 0xFF) and raw[6] in (0, 0xFF):
                return as_int, p + 8
            return struct.unpack("<d", raw)[0], p + 8
        if wt == 4:
            n, q = self.uvar(p)
            return self.take(q, n).decode("utf-8", "replace"), q + n
        if wt == 6:
            n, ewt, q = self.key(p)
            if n > 256 or ewt not in (0, 1, 2, 3):
                raise WireError("packed")
            out = []
            for _ in range(n):
                v, q = self.scalar(q, ewt)
                out.append(v)
            return tuple(out), q
        if wt == CUSTOM:
            n, q = self.uvar(p)
            return self.take(q, n), q + n
        raise WireError(f"wire type {wt:#x}")

    # Every *_ends method maps each end offset the thing at ``p`` can reach to
    # ``(value, cost)``: the number of object sections in that reading, plus a large
    # penalty for each array whose value count strays from its header. Where two
    # readings reach the same end, the cheaper one wins.

    def value_ends(self, p: int, wt: int) -> dict[int, tuple[Any, int]]:
        if wt == OBJ:
            return self.obj_ends(p)
        if wt == ARR:
            return self.array_ends(p)
        if wt in (MAP, BIGMAP, STRMAP):
            return self.map_ends(p, wt)
        v, q = self.scalar(p, wt)
        # Strings are text (paths, keys, labels); binary rides in CUSTOM. Control
        # bytes mean this "string" is really misaligned structure.
        return {q: (v, GARBAGE_PENALTY if wt == 4 and _garbled(v) else 0)}

    def _memoised(self, kind: str, p: int, build) -> dict[int, tuple[Any, int]]:
        k = (kind, p)
        hit = self._memo.get(k)
        if hit is None:
            self._memo[k] = {}              # a cycle reads as "no parse"
            try:
                hit = build(p)
            except WireError:
                hit = {}
            self._memo[k] = hit
        return hit

    @staticmethod
    def _search(start, step) -> dict[int, tuple[Any, int]]:
        """Cheapest-first search. ``step(state, acc)`` yields ``("done", end, value, cost)``
        or ``("next", state, acc, cost)``; each state keeps its cheapest accumulator."""
        best: dict[Any, tuple[int, Any]] = {start: (0, None)}
        queue = [start]
        done: dict[int, tuple[Any, int]] = {}
        while queue:
            state = queue.pop()
            cost, acc = best[state]
            for kind, key, val, extra in step(state, acc):
                total = cost + extra
                if kind == "done":
                    if key not in done or total < done[key][1]:
                        done[key] = (val, total)
                elif key not in best or total < best[key][0]:
                    best[key] = (total, val)
                    queue.append(key)
        return done

    def section_ends(self, p: int) -> dict[int, tuple[dict, int]]:
        # A writer emits each property once per section, in the class's enumeration
        # order. An index may only come back straight after itself (a few classes
        # declare several properties under one index); refusing any other repeat is
        # what rejects a nested object swallowing its parent's END, since the parent
        # would then have to write an earlier field a second time.
        def step(state, chain):
            pos, used, last = state
            try:
                idx, wt, q = self.key(pos)
                if wt == END:
                    yield "done", q, _section(chain), 0
                    return
                if idx > MAX_FIELD or (idx in used and idx != last):
                    return
                alts = self.value_ends(q, wt)
            except WireError:
                return
            for end, (v, c) in alts.items():
                yield "next", (end, used | {idx}, idx), (chain, idx, v), c

        return self._memoised("section", p, lambda p: self._search((p, frozenset(), -1), step))

    def obj_ends(self, p: int) -> dict[int, tuple[Obj, int]]:
        def step(state, secs):
            pos, n = state
            secs = secs or []
            if n >= MAX_SECTIONS:
                return
            for end, (sec, c) in self.section_ends(pos).items():
                grown = [*secs, sec]
                yield "done", end, Obj(grown), c + 1
                yield "next", (end, n + 1), grown, c + 1

        return self._memoised("obj", p, lambda p: self._search((p, 0), step))

    def array_ends(self, p: int) -> dict[int, tuple[list, int]]:
        def build(p: int):
            count, ewt, q = self.key(p)

            # The state carries how many values were written, so two readings that
            # reach the same byte with different element counts stay apart until the
            # header count can judge them.
            def step(state, chain):
                pos, written = state
                try:
                    k, r = self.zz(pos, 70)
                    op, slot = k & 7, k >> 3
                    if op == 4:
                        # The header's index is the array's size, and a writer puts
                        # a value in (nearly) every slot. A reading whose value count
                        # strays from it has usually folded one element into its
                        # neighbour as an extra section - same bytes, same section
                        # total - so it pays for every value it is off by.
                        items = _slots(chain)
                        values = [None if v is _EMPTY else v for _, v in sorted(items.items())]
                        yield "done", r, values, COUNT_PENALTY * abs(count - written)
                        return
                    if op == 0:
                        yield "next", (r, written), (chain, slot, _EMPTY), 0
                        return
                    if op != 2 or written >= count + 1:
                        return
                    alts = self.value_ends(r, ewt)
                except WireError:
                    return
                for end, (v, c) in alts.items():
                    yield "next", (end, written + 1), (chain, slot, v), c

            return self._search((q, 0), step)

        return self._memoised("array", p, build)

    def map_ends(self, p: int, kind: int) -> dict[int, tuple[dict, int]]:
        def build(p: int):
            _, vwt, q = self.key(p)
            if vwt != OBJ:
                raise WireError("map header")

            def step(pos, chain):
                try:
                    if kind == MAP:
                        k, r = self.zz(pos, 70)
                        op, mk = k & 7, k >> 3
                    else:
                        op, r = self.take(pos, 1)[0], pos + 1
                        if kind == BIGMAP:
                            mk, r = self.zz(r, 70)
                        else:
                            n, r = self.uvar(r)
                            mk, r = self.take(r, n).decode("utf-8", "replace"), r + n
                    if op == 4:
                        yield "done", r, {k: (None if v is _EMPTY else v) for k, v in _slots(chain).items()}, 0
                        return
                    if op in (0, 3):
                        yield "next", r, (chain, mk, _EMPTY), 0
                        return
                    if op not in (1, 2):
                        return
                    alts = self.value_ends(r, vwt)
                except WireError:
                    return
                for end, (v, c) in alts.items():
                    yield "next", end, (chain, mk, v), c

            return self._search(q, step)

        return self._memoised(f"map{kind}", p, build)


# Readings accumulate as linked chains ``(parent, key, value)`` and become dicts only
# when complete: copying a dict per step made a 5,000-entry array cost gigabytes.
def _unchain(chain) -> list[tuple[Any, Any]]:
    out = []
    while chain is not None:
        chain, key, value = chain
        out.append((key, value))
    out.reverse()
    return out


def _section(chain) -> dict:
    out: dict = {}
    for idx, v in _unchain(chain):
        if idx in out:
            prev = out[idx]
            v = Repeated([*prev, v]) if isinstance(prev, Repeated) else Repeated([prev, v])
        out[idx] = v
    return out


def _slots(chain) -> dict:
    """Array/map slots in write order; a created-empty slot never overwrites a value."""
    out: dict = {}
    for key, v in _unchain(chain):
        if v is _EMPTY:
            out.setdefault(key, v)
        else:
            out[key] = v
    return out


def read_object(data: bytes, start: int = 0, end: int | None = None) -> Obj:
    """``data[start:end]`` read as exactly one object."""
    end = len(data) if end is None else end
    hit = _Reader(data, end).obj_ends(start).get(end)
    if hit is None:
        raise WireError("no reading fills the object exactly")
    return hit[0]


def _entity(data: bytes) -> Prefab | None:
    head = _Reader(data, len(data))
    try:
        tid, p = head.zz(0)
        _, p = head.zz(p, 70)
        size, p = head.uvar(p)
    except WireError:
        return None
    if size <= 0 or p + size != len(data):
        return None
    comps: list[tuple[int, Obj]] = []
    try:
        while p < len(data):
            cid, p = head.zz(p)
            n, p = head.uvar(p)
            if p + n > len(data):
                return None
            comps.append((cid, read_object(data, p, p + n)))
            p += n
    except WireError:
        return None
    return Prefab("entity", tid, comps)


# The search follows misaligned readings a long way down before they fail, so a
# few prefabs (claim/goldenthread) need more Python frames than the default 1,000
# even though the real data nests under 20 deep.
RECURSION_LIMIT = 20_000


def parse(data: bytes) -> Prefab:
    """A whole prefab file. Raises WireError when no reading fits."""
    if sys.getrecursionlimit() < RECURSION_LIMIT:
        sys.setrecursionlimit(RECURSION_LIMIT)
    try:
        return _entity(data) or Prefab("object", root=read_object(data))
    except RecursionError as exc:
        raise WireError("nesting too deep") from exc


def walk(value: Any):
    """Every scalar under a parsed value, depth first."""
    if isinstance(value, dict):
        for v in value.values():
            yield from walk(v)
    elif isinstance(value, (list, tuple)):
        for v in value:
            yield from walk(v)
    else:
        yield value


def strings(value: Any) -> list[str]:
    return [v for v in walk(value) if isinstance(v, str)]
