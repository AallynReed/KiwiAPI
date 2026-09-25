"""Constant arrays compiled into a movie's ActionScript (AVM2 ``DoABC`` bytecode).

Some game facts live only in interface code: the welcome screen's
``WelcomeDailyBonusWindow.BONUSES`` is the one place that says Monday's bonus is
the ``shadow`` set of texts. ``static_arrays`` reads every class's static
initialiser and returns the literal arrays it assigns, without decompiling
anything else. Pure + stdlib-only.

Format: the ABC File Format chapter of the AVM2 overview (constant pool, method,
metadata, instance, class, script and method-body tables, in that order).
"""
from __future__ import annotations

import struct

from app.trove.swf.extract import SwfError, _decompress, _iter_tags

DO_ABC, DO_ABC2 = 72, 82
QNAMES = (0x07, 0x0D)
_TRAIT_METADATA = 0x04
_HAS_OPTIONAL, _HAS_PARAM_NAMES = 0x08, 0x80

# opcode -> (u30 operands, values popped, values pushed) for what a static
# initialiser does besides pushing literals; anything else stops the walk.
_OPS = {
    0xD0: (0, 0, 1), 0xD1: (0, 0, 1), 0xD2: (0, 0, 1), 0xD3: (0, 0, 1),
    0x30: (0, 1, 0), 0x1D: (0, 0, 0), 0x20: (0, 0, 1), 0x21: (0, 0, 1),
    0x28: (0, 0, 1), 0x29: (0, 1, 0), 0x5D: (1, 0, 1), 0x5E: (1, 0, 1),
    0x60: (1, 0, 1), 0x66: (1, 1, 1), 0x62: (1, 0, 1), 0x63: (1, 1, 0),
    0x6C: (1, 1, 1), 0x6D: (1, 2, 0), 0x80: (1, 1, 1), 0x58: (1, 1, 1),
}
_CALLS = {0x46: 1, 0x4A: 1, 0x4F: 0}          # (name, argc): pop argc + receiver


class _Buf:
    def __init__(self, data: bytes, pos: int = 0):
        self.d, self.p = data, pos

    def u8(self) -> int:
        self.p += 1
        return self.d[self.p - 1]

    def u16(self) -> int:
        self.p += 2
        return struct.unpack_from("<H", self.d, self.p - 2)[0]

    def u30(self) -> int:
        out = shift = 0
        for _ in range(5):
            b = self.u8()
            out |= (b & 0x7F) << shift
            if not b & 0x80:
                break
            shift += 7
        return out

    def skip(self, n: int) -> None:
        self.p += n


def _abc_blocks(swf: bytes) -> list[bytes]:
    out = []
    for code, body in _iter_tags(_decompress(swf)):
        if code == DO_ABC2:
            body = body[4:]
            body = body[body.index(b"\0") + 1:]
        elif code != DO_ABC:
            continue
        out.append(body)
    return out


def _traits(b: _Buf) -> list[tuple[int, int, int]]:
    """``(name, kind, method-or-slot)`` per trait."""
    out = []
    for _ in range(b.u30()):
        name, kind = b.u30(), b.u8()
        k = kind & 0x0F
        if k in (0, 6):
            b.u30()
            b.u30()
            if b.u30():
                b.u8()
            out.append((name, k, 0))
        elif k in (1, 2, 3, 4, 5):
            b.u30()
            out.append((name, k, b.u30()))
        else:
            raise SwfError(f"bad trait kind {kind}")
        if kind >> 4 & _TRAIT_METADATA:
            for _ in range(b.u30()):
                b.u30()
    return out


def _parse(abc: bytes) -> dict[str, dict[str, list]]:
    b = _Buf(abc)
    b.u16(), b.u16()
    ints = [0] + [b.u30() for _ in range(max(b.u30() - 1, 0))]
    uints = [0] + [b.u30() for _ in range(max(b.u30() - 1, 0))]
    doubles = [0.0]
    for _ in range(max(b.u30() - 1, 0)):
        doubles.append(struct.unpack_from("<d", abc, b.p)[0])
        b.skip(8)
    strings = [""]
    for _ in range(max(b.u30() - 1, 0)):
        n = b.u30()
        strings.append(abc[b.p:b.p + n].decode("utf-8", "replace"))
        b.skip(n)
    for _ in range(max(b.u30() - 1, 0)):
        b.u8(), b.u30()
    for _ in range(max(b.u30() - 1, 0)):
        for _ in range(b.u30()):
            b.u30()
    names = [""]
    for _ in range(max(b.u30() - 1, 0)):
        kind = b.u8()
        if kind in QNAMES:
            b.u30()
            names.append(strings[b.u30()])
        elif kind in (0x0F, 0x10):
            names.append(strings[b.u30()])
        elif kind in (0x11, 0x12):
            names.append("")
        elif kind in (0x09, 0x0E):
            names.append(strings[b.u30()])
            b.u30()
        elif kind in (0x1B, 0x1C):
            b.u30()
            names.append("")
        elif kind == 0x1D:
            b.u30()
            names.append("")
            for _ in range(b.u30()):
                b.u30()
        else:
            raise SwfError(f"bad multiname kind {kind}")
    for _ in range(b.u30()):
        params = b.u30()
        b.u30()
        for _ in range(params):
            b.u30()
        b.u30()
        flags = b.u8()
        if flags & _HAS_OPTIONAL:
            for _ in range(b.u30()):
                b.u30(), b.u8()
        if flags & _HAS_PARAM_NAMES:
            for _ in range(params):
                b.u30()
    for _ in range(b.u30()):
        b.u30()
        for _ in range(b.u30()):
            b.u30(), b.u30()
    classes = b.u30()
    class_names = []
    for _ in range(classes):
        class_names.append(names[b.u30()])
        b.u30()
        if b.u8() & 0x08:
            b.u30()
        for _ in range(b.u30()):
            b.u30()
        b.u30()
        _traits(b)
    cinits = {}
    for i in range(classes):
        cinits[b.u30()] = class_names[i]
        _traits(b)
    for _ in range(b.u30()):
        b.u30()
        _traits(b)
    out: dict[str, dict[str, list]] = {}
    for _ in range(b.u30()):
        method = b.u30()
        b.u30(), b.u30(), b.u30(), b.u30()
        size = b.u30()
        code = abc[b.p:b.p + size]
        b.skip(size)
        for _ in range(b.u30()):
            b.u30(), b.u30(), b.u30(), b.u30(), b.u30()
        _traits(b)
        if method in cinits:
            arrays = _literal_arrays(code, strings, ints, uints, doubles, names)
            if arrays:
                out[cinits[method]] = arrays
    return out


def _literal_arrays(code: bytes, strings, ints, uints, doubles, names) -> dict[str, list]:
    """``name -> [values]`` for each ``push...; newarray n; initproperty name``."""
    b, stack, out = _Buf(code), [], {}
    while b.p < len(code):
        op = b.u8()
        if op == 0x2C:
            stack.append(strings[b.u30()])
        elif op == 0x2D:
            stack.append(ints[b.u30()])
        elif op == 0x2E:
            stack.append(uints[b.u30()])
        elif op == 0x2F:
            stack.append(doubles[b.u30()])
        elif op == 0x24:
            stack.append(struct.unpack("b", bytes([b.u8()]))[0])
        elif op == 0x25:
            stack.append(b.u30())
        elif op == 0x26:
            stack.append(True)
        elif op == 0x27:
            stack.append(False)
        elif op == 0x56:
            n = b.u30()
            items = stack[-n:] if n else []
            del stack[len(stack) - n:]
            stack.append(list(items))
        elif op in (0x68, 0x61):
            name = names[b.u30()]
            value = stack.pop() if stack else None
            if stack:
                stack.pop()                          # the receiver
            if isinstance(value, list):
                out[name] = value
        elif op == 0x2A:
            stack.append(stack[-1] if stack else None)
        elif op == 0x55:
            n = b.u30()
            del stack[max(len(stack) - 2 * n, 0):]
            stack.append(None)
        elif op == 0x42:
            n = b.u30()
            del stack[max(len(stack) - n - 1, 0):]
            stack.append(None)
        elif op in _CALLS:
            b.u30()
            n = b.u30()
            del stack[max(len(stack) - n - 1, 0):]
            stack.extend([None] * _CALLS[op])
        elif op in _OPS:
            operands, pops, pushes = _OPS[op]
            for _ in range(operands):
                b.u30()
            del stack[max(len(stack) - pops, 0):]
            stack.extend([None] * pushes)
        elif op == 0x47:
            break
        else:
            break
    return out


def static_arrays(swf: bytes) -> dict[str, dict[str, list]]:
    """``class name -> {static name: literal array}`` for every class in the movie
    whose static initialiser assigns array literals (strings, numbers, booleans)."""
    out: dict[str, dict[str, list]] = {}
    for block in _abc_blocks(swf):
        try:
            out.update(_parse(block))
        except (IndexError, struct.error) as exc:
            raise SwfError(f"bad ABC block: {exc}") from None
    return out
