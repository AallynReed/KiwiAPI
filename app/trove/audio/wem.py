"""Read one Wwise media object (a ``.wem``) and turn it into something playable.

A ``.wem`` is a RIFF file whose ``fmt`` chunk names one of three codecs, and each
needs a different kind of work to become browser-playable:

============  ==========================  ==========================================
format tag    codec                       what happens here
============  ==========================  ==========================================
``0xFFFF``    Wwise Vorbis                Ogg framing and headers are rebuilt around
                                          the untouched packets - no re-encoding, so
                                          the audio is bit-identical to the game's.
``0x0002``    Wwise ADPCM                 4-bit IMA, decoded to 16-bit PCM.
``0xFFFE``    PCM                         already samples; only the header changes.
============  ==========================  ==========================================

Vorbis comes out as ``.ogg`` and the other two as ``.wav``, both of which every
current browser decodes natively.

Only the modern Wwise layout is handled - the one where the Vorbis parameters ride
inside an extended 0x42-byte ``fmt`` chunk rather than a separate ``vorb`` chunk.
Trove is built entirely from that layout, and guessing at an older one would mean
emitting audio that is silently wrong rather than refusing it.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from app.trove.audio.ogg import OggWriter
from app.trove.audio.wwise_vorbis import BitReader, BitWriter, VorbisError, rebuild_setup

FMT_PCM = 0xFFFE
FMT_ADPCM = 0x0002
FMT_VORBIS = 0xFFFF

CODEC_NAMES = {FMT_PCM: "pcm", FMT_ADPCM: "adpcm", FMT_VORBIS: "vorbis"}

# The extended fmt layout that carries the Vorbis parameters inline.
VORBIS_FMT_SIZE = 0x42
_VORB_AT = 0x18              # where those parameters start inside fmt
_SETUP_OFFSET_AT = 0x10      # ... and where, inside those, the packet offsets sit
_FIRST_PACKET_AT = 0x14
_BLOCKSIZE_AT = 0x28

ADPCM_BLOCK = 0x24           # bytes per channel per block
ADPCM_HEADER = 4             # int16 predictor + int16 step index

MAX_DECODED_BYTES = 96 * 1024 * 1024


class WemError(Exception):
    """The bytes are not a Wwise media object we can read."""


@dataclass(slots=True)
class WemInfo:
    codec: str
    channels: int
    sample_rate: int
    bits_per_sample: int
    block_align: int
    avg_bytes_per_sec: int
    sample_count: int          # 0 when the codec does not record one
    duration: float
    data_offset: int
    data_size: int


# ─── container ────────────────────────────────────────────────────────────

def _chunks(raw: bytes) -> dict[bytes, tuple[int, int]]:
    """Map every RIFF chunk id to its ``(offset, size)``. Later duplicates win,
    matching how Wwise itself reads these."""
    if len(raw) < 12 or raw[:4] != b"RIFF" or raw[8:12] != b"WAVE":
        raise WemError("not a RIFF/WAVE container")
    out: dict[bytes, tuple[int, int]] = {}
    off = 12
    while off + 8 <= len(raw):
        tag = raw[off:off + 4]
        size = struct.unpack_from("<I", raw, off + 4)[0]
        if off + 8 + size > len(raw):
            break
        out[tag] = (off + 8, size)
        off += 8 + size + (size & 1)
    if b"fmt " not in out or b"data" not in out:
        raise WemError("missing fmt or data chunk")
    return out


def parse(raw: bytes) -> WemInfo:
    """Header facts about one media object, without decoding it."""
    chunks = _chunks(raw)
    fmt_off, fmt_size = chunks[b"fmt "]
    if fmt_size < 16:
        raise WemError("fmt chunk is too small")
    codec, channels, rate, avg, align, bits = struct.unpack_from("<HHIIHH", raw, fmt_off)
    if codec not in CODEC_NAMES:
        raise WemError(f"unsupported codec 0x{codec:04x}")
    # Wwise will happily encode a distant ambience at a few kHz, so the floor here
    # is only meant to catch a header that is not a header at all.
    if not 1 <= channels <= 8 or not 1000 <= rate <= 192000:
        raise WemError("implausible channel count or sample rate")
    data_off, data_size = chunks[b"data"]

    if codec == FMT_VORBIS:
        if fmt_size != VORBIS_FMT_SIZE:
            raise WemError(f"unsupported Vorbis header layout (fmt size {fmt_size})")
        samples = struct.unpack_from("<I", raw, fmt_off + _VORB_AT)[0]
    elif codec == FMT_ADPCM:
        per_block = (ADPCM_BLOCK - ADPCM_HEADER) * 2
        blocks = data_size // align if align else 0
        samples = blocks * per_block
    else:
        frame = channels * max(bits // 8, 1)
        samples = data_size // frame if frame else 0

    return WemInfo(
        codec=CODEC_NAMES[codec], channels=channels, sample_rate=rate,
        bits_per_sample=bits, block_align=align, avg_bytes_per_sec=avg,
        sample_count=samples, duration=(samples / rate) if rate else 0.0,
        data_offset=data_off, data_size=data_size,
    )


def convert(raw: bytes) -> tuple[bytes, str, str]:
    """Return ``(bytes, mime, extension)`` for a browser-playable rendering."""
    info = parse(raw)
    if info.codec == "vorbis":
        return _to_ogg(raw, info), "audio/ogg", "ogg"
    if info.codec == "adpcm":
        return _wav(_decode_adpcm(raw, info), info.channels, info.sample_rate), "audio/wav", "wav"
    return _wav(_pcm_samples(raw, info), info.channels, info.sample_rate), "audio/wav", "wav"


# ─── Vorbis ───────────────────────────────────────────────────────────────

def _packets(raw: bytes, start: int, end: int) -> list[bytes]:
    """Split Wwise's length-prefixed packet run. Each packet is a 16-bit size
    followed by that many bytes, with no granule position - the newer layout."""
    out: list[bytes] = []
    off = start
    while off + 2 <= end:
        size = struct.unpack_from("<H", raw, off)[0]
        off += 2
        if size == 0 or off + size > end:
            break
        out.append(raw[off:off + size])
        off += size
    return out


def _to_ogg(raw: bytes, info: WemInfo) -> bytes:
    chunks = _chunks(raw)
    fmt_off, _ = chunks[b"fmt "]
    data_off, data_size = chunks[b"data"]
    vorb = fmt_off + _VORB_AT
    setup_at = struct.unpack_from("<I", raw, vorb + _SETUP_OFFSET_AT)[0]
    first_at = struct.unpack_from("<I", raw, vorb + _FIRST_PACKET_AT)[0]
    short_pow = raw[vorb + _BLOCKSIZE_AT]
    long_pow = raw[vorb + _BLOCKSIZE_AT + 1]
    if not 6 <= short_pow <= long_pow <= 13:
        raise WemError(f"implausible block sizes 2^{short_pow}/2^{long_pow}")
    if not setup_at <= first_at <= data_size:
        raise WemError("packet offsets fall outside the data chunk")

    size = struct.unpack_from("<H", raw, data_off + setup_at)[0]
    body = raw[data_off + setup_at + 2:data_off + setup_at + 2 + size]
    try:
        setup = rebuild_setup(body, info.channels)
    except VorbisError as exc:
        raise WemError(str(exc)) from None

    out = OggWriter(serial=1)
    out.write(_ident(info, short_pow, long_pow))
    out.flush()                       # the identification packet owns page one
    out.write(_comment())
    out.write(b"\x05vorbis" + setup.body)
    out.flush()

    packets = _packets(raw, data_off + first_at, data_off + data_size)
    if not packets:
        raise WemError("stream carries no audio packets")
    blocksizes = (1 << short_pow, 1 << long_pow)
    modes = [p[0] & ((1 << setup.mode_bits) - 1) if p else 0 for p in packets]

    granules: list[int] = []
    position = 0
    previous = 0
    for i, mode in enumerate(modes):
        current = blocksizes[setup.mode_blockflags[mode]]
        if i:
            # A packet emits the overlap of its own window with the last one.
            position += (previous + current) // 4
        previous = current
        granules.append(position)
    # The recorded sample count is authoritative; a shorter final granule is how
    # Vorbis trims the padding the last window would otherwise leave behind.
    if info.sample_count and info.sample_count <= granules[-1]:
        granules[-1] = info.sample_count

    if setup.mode_bits == 0:
        # A single-mode stream has no mode field to pack against, and Wwise leaves
        # those packets whole - type bit and all. Rewriting them would shift the
        # payload off by a bit and decode to noise.
        for i, packet in enumerate(packets):
            out.write(packet, granules[i])
        return out.getvalue()

    previous_blockflag = 0
    for i, packet in enumerate(packets):
        mode = modes[i]
        blockflag = setup.mode_blockflags[mode]
        following = setup.mode_blockflags[modes[i + 1]] if i + 1 < len(modes) else 0
        out.write(_restore_packet(packet, mode, setup.mode_bits, blockflag,
                                  previous_blockflag, following), granules[i])
        previous_blockflag = blockflag
    return out.getvalue()


def _restore_packet(packet: bytes, mode: int, mode_bits: int, blockflag: int,
                    previous: int, following: int) -> bytes:
    """Put back the bits Wwise strips from every audio packet.

    Gone are the leading packet-type bit - always zero for audio - and, on long
    blocks, the two flags saying whether the neighbouring windows are long. Both
    are recoverable: the type is a constant, and the window flags are just the
    block types of the packets either side. Everything after the first byte is
    untouched payload, but it has to be re-emitted through the bit writer because
    the insertion shifts it off its byte boundary.
    """
    bits = BitReader(packet)
    out = BitWriter()
    out.write(0, 1)
    out.write(bits.read(mode_bits), mode_bits)
    rest = bits.read(8 - mode_bits)
    if blockflag:
        out.write(previous, 1)
        out.write(following, 1)
    out.write(rest, 8 - mode_bits)
    for byte in packet[1:]:
        out.write(byte, 8)
    return out.bytes()


def _ident(info: WemInfo, short_pow: int, long_pow: int) -> bytes:
    return b"\x01vorbis" + struct.pack(
        "<IBIiiiBB", 0, info.channels, info.sample_rate,
        0, info.avg_bytes_per_sec * 8, 0,
        short_pow | (long_pow << 4), 1,
    )


def _comment() -> bytes:
    vendor = b"Kiwi API"
    return b"\x03vorbis" + struct.pack("<I", len(vendor)) + vendor + struct.pack("<I", 0) + b"\x01"


# ─── PCM and ADPCM ────────────────────────────────────────────────────────

_STEP_TABLE = (
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45,
    50, 55, 60, 66, 73, 80, 88, 97, 107, 118, 130, 143, 157, 173, 190, 209, 230,
    253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658, 724, 796, 876, 963,
    1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327,
    3660, 4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442,
    11487, 12635, 13899, 15289, 16818, 18500, 20350, 22385, 24623, 27086, 29794,
    32767,
)
_INDEX_TABLE = (-1, -1, -1, -1, 2, 4, 6, 8)


def _pcm_samples(raw: bytes, info: WemInfo) -> bytes:
    if info.bits_per_sample != 16:
        raise WemError(f"unsupported PCM depth {info.bits_per_sample}")
    return raw[info.data_offset:info.data_offset + info.data_size]


def _decode_adpcm(raw: bytes, info: WemInfo) -> bytes:
    """Decode Wwise's 4-bit IMA into interleaved 16-bit PCM.

    The stream is blocked per channel: each channel gets its own 0x24-byte block
    holding a 4-byte state header - the running predictor and step index - and 64
    nibbles of deltas. For stereo the two channels' blocks simply alternate, so
    each block pair decodes independently and is woven back together at the end.
    """
    channels = info.channels
    stride = ADPCM_BLOCK * channels
    if info.block_align != stride:
        raise WemError(f"unexpected ADPCM block size {info.block_align}")
    per_block = (ADPCM_BLOCK - ADPCM_HEADER) * 2
    blocks = info.data_size // stride
    if blocks * per_block * channels * 2 > MAX_DECODED_BYTES:
        raise WemError("decoded audio would be too large")

    data = raw[info.data_offset:info.data_offset + info.data_size]
    out = bytearray(blocks * per_block * channels * 2)
    for block in range(blocks):
        base = block * stride
        for channel in range(channels):
            offset = base + channel * ADPCM_BLOCK
            predictor, index = struct.unpack_from("<hh", data, offset)
            index = max(0, min(88, index))
            write = (block * per_block * channels + channel) * 2
            for i in range((ADPCM_BLOCK - ADPCM_HEADER) * 2):
                byte = data[offset + ADPCM_HEADER + (i >> 1)]
                nibble = (byte & 0x0F) if not i & 1 else (byte >> 4)
                step = _STEP_TABLE[index]
                delta = step >> 3
                if nibble & 1:
                    delta += step >> 2
                if nibble & 2:
                    delta += step >> 1
                if nibble & 4:
                    delta += step
                if nibble & 8:
                    delta = -delta
                predictor = max(-32768, min(32767, predictor + delta))
                index = max(0, min(88, index + _INDEX_TABLE[nibble & 7]))
                struct.pack_into("<h", out, write, predictor)
                write += channels * 2
    return bytes(out)


def _wav(samples: bytes, channels: int, rate: int) -> bytes:
    """Wrap interleaved 16-bit PCM in the smallest valid WAVE header."""
    align = channels * 2
    header = struct.pack(
        "<4sI4s4sIHHIIHH4sI",
        b"RIFF", 36 + len(samples), b"WAVE",
        b"fmt ", 16, 1, channels, rate, rate * align, align, 16,
        b"data", len(samples),
    )
    return header + samples
