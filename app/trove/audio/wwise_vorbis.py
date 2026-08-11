"""Turn a Wwise Vorbis stream back into an Ogg file the browser can play.

Wwise does not store Ogg. It keeps the raw Vorbis *audio* packets and throws away
everything a decoder needs to make sense of them, because its own runtime already
knows all of it:

* the three Vorbis headers (identification, comment, setup) are gone - the first
  two are pure boilerplate, and the third is stored in a stripped private form;
* every field the encoder could recompute is dropped from that setup header - the
  time-domain placeholder, the 16-bit type tags, the framing bit;
* the Huffman codebooks are replaced by 10-bit indices into a fixed library that
  ships inside the Wwise runtime, not inside the file;
* the Ogg framing is gone - packets are laid end to end behind a 16-bit length;
* and each audio packet loses its leading packet-type bit, plus the two window
  flags on long blocks, since those follow from the surrounding packets.

So this module does not *decode* anything: the Vorbis payload is already valid and
the browser decodes it natively. It rebuilds the discarded scaffolding around it -
re-expanding the codebooks from :data:`CODEBOOKS_FILE`, re-emitting the setup
header in standard form, re-deriving the window flags by looking one packet ahead,
and paging the result into an Ogg container.

The codebook library is Wwise's own, recovered by the ww2ogg project; without it
the indices in the file are unresolvable and the stream simply cannot be rebuilt.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass
from pathlib import Path

CODEBOOKS_FILE = Path(__file__).with_name("packed_codebooks_aoTuV_603.bin")

# Wwise's own cap is far lower; this only stops a corrupt length field from
# driving an unbounded loop.
MAX_PACKETS = 2_000_000


class VorbisError(Exception):
    """The stream is not Wwise Vorbis we can rebuild."""


def _ilog(x: int) -> int:
    """Vorbis ``ilog``: the number of significant bits in *x* (0 for x <= 0)."""
    n = 0
    while x > 0:
        n += 1
        x >>= 1
    return n


# ─── bit plumbing ─────────────────────────────────────────────────────────
#
# Vorbis packs bits low-to-high within each byte, so both ends work LSB-first.

class BitReader:
    __slots__ = ("data", "pos")

    def __init__(self, data: bytes) -> None:
        self.data = data
        self.pos = 0

    def read(self, n: int) -> int:
        v = 0
        data, pos = self.data, self.pos
        for i in range(n):
            byte = pos >> 3
            if byte >= len(data):
                raise VorbisError("ran off the end of a packet")
            v |= ((data[byte] >> (pos & 7)) & 1) << i
            pos += 1
        self.pos = pos
        return v


class BitWriter:
    __slots__ = ("_out", "_acc", "_nbits")

    def __init__(self) -> None:
        self._out = bytearray()
        self._acc = 0
        self._nbits = 0

    def write(self, value: int, n: int) -> None:
        acc, nbits = self._acc, self._nbits
        acc |= (value & ((1 << n) - 1)) << nbits
        nbits += n
        while nbits >= 8:
            self._out.append(acc & 0xFF)
            acc >>= 8
            nbits -= 8
        self._acc, self._nbits = acc, nbits

    def bytes(self) -> bytes:
        """Flush to a byte boundary and return the packet. Padding bits are zero,
        which is what Vorbis expects after a packet's framing bit."""
        if self._nbits:
            self._out.append(self._acc & 0xFF)
            self._acc = 0
            self._nbits = 0
        return bytes(self._out)


# ─── codebook library ─────────────────────────────────────────────────────

class CodebookLibrary:
    """The packed Wwise codebook table, indexed by the ids found in a stream.

    Layout is a blob of packed codebooks, an array of ``uint32`` offsets into it,
    and a trailing ``uint32`` pointing at that array. The offset array carries one
    extra entry so each codebook's length is the gap to its successor.
    """

    __slots__ = ("_data", "_offsets")

    def __init__(self, raw: bytes) -> None:
        if len(raw) < 4:
            raise VorbisError("codebook library is truncated")
        table = struct.unpack_from("<I", raw, len(raw) - 4)[0]
        if table > len(raw) - 4 or (len(raw) - 4 - table) % 4:
            raise VorbisError("codebook library has a bad offset table")
        count = (len(raw) - table) // 4
        self._data = raw
        self._offsets = list(struct.unpack_from(f"<{count}I", raw, table))

    def __len__(self) -> int:
        return len(self._offsets) - 1

    def expand(self, cb_id: int, out: BitWriter) -> None:
        """Write codebook *cb_id* to *out* in standard Vorbis form.

        The packed form is the same data with every field the decoder can infer
        narrowed or dropped, so this is a field-by-field widening rather than a
        decode: 4-bit dimensions become 16, 14-bit entry counts become 24, the
        ``BCV`` sync word and the 4-bit lookup type come back.
        """
        if not 0 <= cb_id < len(self):
            raise VorbisError(f"codebook {cb_id} is not in the library")
        start, end = self._offsets[cb_id], self._offsets[cb_id + 1]
        bits = BitReader(self._data[start:end])

        dimensions = bits.read(4)
        entries = bits.read(14)
        out.write(0x564342, 24)          # "BCV"
        out.write(dimensions, 16)
        out.write(entries, 24)

        ordered = bits.read(1)
        out.write(ordered, 1)
        if ordered:
            out.write(bits.read(5), 5)   # initial length
            current = 0
            while current < entries:
                n = bits.read(_ilog(entries - current))
                out.write(n, _ilog(entries - current))
                current += n
            if current > entries:
                raise VorbisError("ordered codebook overruns its entry count")
        else:
            length_bits = bits.read(3)
            sparse = bits.read(1)
            if not 1 <= length_bits <= 5:
                raise VorbisError(f"bad codeword length width {length_bits}")
            out.write(sparse, 1)
            for _ in range(entries):
                present = True
                if sparse:
                    present = bool(bits.read(1))
                    out.write(int(present), 1)
                if present:
                    out.write(bits.read(length_bits), 5)

        lookup = bits.read(1)
        out.write(lookup, 4)
        if lookup == 1:
            out.write(bits.read(32), 32)     # minimum value
            out.write(bits.read(32), 32)     # delta value
            value_bits = bits.read(4)
            out.write(value_bits, 4)
            out.write(bits.read(1), 1)       # sequence flag
            for _ in range(_quantvals(entries, dimensions)):
                out.write(bits.read(value_bits + 1), value_bits + 1)
        elif lookup:
            raise VorbisError(f"lookup type {lookup} is not valid")


def _quantvals(entries: int, dimensions: int) -> int:
    """``_book_maptype1_quantvals`` from libvorbis: the largest ``v`` whose
    ``dimensions``-th power still fits inside ``entries``."""
    if dimensions <= 0:
        raise VorbisError("codebook has no dimensions")
    bits = _ilog(entries)
    vals = entries >> ((bits - 1) * (dimensions - 1) // dimensions)
    while True:
        acc = vals ** dimensions
        acc1 = (vals + 1) ** dimensions
        if acc <= entries < acc1:
            return vals
        vals += -1 if acc > entries else 1


_LIBRARY: CodebookLibrary | None = None


def library() -> CodebookLibrary:
    """The shared codebook library, loaded once."""
    global _LIBRARY
    if _LIBRARY is None:
        try:
            _LIBRARY = CodebookLibrary(CODEBOOKS_FILE.read_bytes())
        except OSError as exc:
            raise VorbisError(f"codebook library is unavailable: {exc}") from None
    return _LIBRARY


# ─── setup header ─────────────────────────────────────────────────────────

@dataclass(slots=True)
class Setup:
    body: bytes                 # the rebuilt setup header, minus its packet prefix
    mode_blockflags: list[int]  # long-block flag per mode
    mode_bits: int              # width of the mode field in an audio packet


def rebuild_setup(packed: bytes, channels: int) -> Setup:
    """Re-emit a Wwise setup packet as a standard Vorbis one.

    Read and write run in lockstep: every field is copied at its Vorbis width,
    and the ones Wwise dropped because they are constant - the time-domain
    placeholder, the type tags on floors/residues/mappings, the framing bit - are
    written back from nothing.
    """
    bits = BitReader(packed)
    out = BitWriter()
    lib = library()

    codebook_count = bits.read(8) + 1
    out.write(codebook_count - 1, 8)
    for _ in range(codebook_count):
        lib.expand(bits.read(10), out)

    # Time-domain transforms are unused by Vorbis I but still occupy the stream.
    out.write(0, 6)
    out.write(0, 16)

    floor_count = bits.read(6) + 1
    out.write(floor_count - 1, 6)
    for _ in range(floor_count):
        out.write(1, 16)                       # floor type 1 is the only one Wwise emits
        partitions = bits.read(5)
        out.write(partitions, 5)
        classes = [bits.read(4) for _ in range(partitions)]
        for c in classes:
            out.write(c, 4)
        dimensions = []
        for _ in range((max(classes) + 1) if classes else 0):
            dim = bits.read(3)
            out.write(dim, 3)
            subclasses = bits.read(2)
            out.write(subclasses, 2)
            if subclasses:
                out.write(bits.read(8), 8)     # master book
            for _ in range(1 << subclasses):
                out.write(bits.read(8), 8)     # subclass books, biased by one
            dimensions.append(dim + 1)
        out.write(bits.read(2), 2)             # multiplier, biased by one
        rangebits = bits.read(4)
        out.write(rangebits, 4)
        for c in classes:
            for _ in range(dimensions[c]):
                out.write(bits.read(rangebits), rangebits)

    residue_count = bits.read(6) + 1
    out.write(residue_count - 1, 6)
    for _ in range(residue_count):
        residue_type = bits.read(2)
        if residue_type > 2:
            raise VorbisError(f"residue type {residue_type} is not valid")
        out.write(residue_type, 16)
        out.write(bits.read(24), 24)           # begin
        out.write(bits.read(24), 24)           # end
        out.write(bits.read(24), 24)           # partition size, biased by one
        classifications = bits.read(6) + 1
        out.write(classifications - 1, 6)
        out.write(bits.read(8), 8)             # classbook
        cascade = []
        for _ in range(classifications):
            low = bits.read(3)
            out.write(low, 3)
            flag = bits.read(1)
            out.write(flag, 1)
            high = 0
            if flag:
                high = bits.read(5)
                out.write(high, 5)
            cascade.append(low | (high << 3))
        for mask in cascade:
            for bit in range(8):
                if mask & (1 << bit):
                    out.write(bits.read(8), 8)

    mapping_count = bits.read(6) + 1
    out.write(mapping_count - 1, 6)
    for _ in range(mapping_count):
        out.write(0, 16)                       # mapping type 0
        submap_flag = bits.read(1)
        out.write(submap_flag, 1)
        submaps = 1
        if submap_flag:
            submaps = bits.read(4) + 1
            out.write(submaps - 1, 4)
        coupled = bits.read(1)
        out.write(coupled, 1)
        if coupled:
            steps = bits.read(8) + 1
            out.write(steps - 1, 8)
            width = _ilog(channels - 1)
            for _ in range(steps):
                out.write(bits.read(width), width)   # magnitude
                out.write(bits.read(width), width)   # angle
        reserved = bits.read(2)
        if reserved:
            raise VorbisError("mapping reserved field is not zero")
        out.write(0, 2)
        if submaps > 1:
            for _ in range(channels):
                out.write(bits.read(4), 4)     # channel -> submap
        for _ in range(submaps):
            out.write(bits.read(8), 8)         # unused time config
            out.write(bits.read(8), 8)         # floor
            out.write(bits.read(8), 8)         # residue

    mode_count = bits.read(6) + 1
    out.write(mode_count - 1, 6)
    blockflags = []
    for _ in range(mode_count):
        blockflag = bits.read(1)
        out.write(blockflag, 1)
        blockflags.append(blockflag)
        out.write(0, 16)                       # window type
        out.write(0, 16)                       # transform type
        out.write(bits.read(8), 8)             # mapping

    out.write(1, 1)                            # framing
    return Setup(out.bytes(), blockflags, _ilog(mode_count - 1))
