"""Write a ``.wem`` the game will accept, from ordinary 16-bit PCM.

Only two of Wwise's three codecs can be produced here, and that is a hard limit
rather than a shortcut: **Wwise Vorbis cannot be encoded.** Its bitstream leans on
a codebook library that lives inside Audiokinetic's encoder, and no open
implementation exists - decoding one is a matter of putting back what was
stripped (see :mod:`app.trove.audio.wwise_vorbis`), but writing one is not.

That costs nothing in practice. Trove's own banks already ship PCM (``0xFFFE``)
and Wwise ADPCM (``0x0002``), so the runtime demonstrably plays both:

* **ADPCM** is the default - a quarter the size of PCM at a quality that is
  inaudible for the short effects people actually replace.
* **PCM** is offered for anyone who wants the sample data untouched.

Headers are written to match the game's own byte for byte, including the padding
chunk that lands the samples on a 16-byte boundary.
"""

from __future__ import annotations

import struct

from app.trove.audio.wem import ADPCM_BLOCK, ADPCM_HEADER, FMT_ADPCM, FMT_PCM

# Wwise's speaker masks for the only two layouts worth writing.
_CHANNEL_MASK = {1: 0x4, 2: 0x3}          # front-centre / front-left+right
_CONFIG_TYPE = 1                          # "standard" speaker configuration

MAX_SAMPLES = 48000 * 60 * 10             # ten minutes at 48 kHz, per sound

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

SAMPLES_PER_BLOCK = (ADPCM_BLOCK - ADPCM_HEADER) * 2      # 64


class EncodeError(Exception):
    """The audio cannot be written as a Wwise media object."""


def _check(channels: int, rate: int, frames: int) -> None:
    if channels not in _CHANNEL_MASK:
        raise EncodeError("only mono and stereo are supported")
    if not 1000 <= rate <= 96000:
        raise EncodeError(f"sample rate {rate} is out of range")
    if frames <= 0:
        raise EncodeError("the clip is empty")
    if frames > MAX_SAMPLES:
        raise EncodeError("the clip is too long")


def _riff(fmt: bytes, data: bytes) -> bytes:
    """Assemble a RIFF/WAVE with the padding chunk Wwise uses.

    The game's own media puts ``data`` on a 16-byte boundary; a four-byte ``JUNK``
    is exactly what makes that land after the 24-byte extended ``fmt``.
    """
    chunks = (b"fmt " + struct.pack("<I", len(fmt)) + fmt
              + b"JUNK" + struct.pack("<I", 4) + b"\x00" * 4
              + b"data" + struct.pack("<I", len(data)) + data)
    body = b"WAVE" + chunks
    return b"RIFF" + struct.pack("<I", len(body)) + body


def _fmt(codec: int, channels: int, rate: int, avg: int, align: int, bits: int) -> bytes:
    """Wwise's extended ``fmt``: the standard 16-byte header, then a six-byte
    extension holding the speaker layout (channel count, config type, mask)."""
    config = channels | (_CONFIG_TYPE << 8) | (_CHANNEL_MASK[channels] << 12)
    return struct.pack("<HHIIHHHHI", codec, channels, rate, avg, align, bits,
                       6,          # cbSize - the six-byte extension below
                       0,          # reserved
                       config)


def pcm_wem(samples: bytes, channels: int, rate: int) -> bytes:
    """A PCM media object from interleaved little-endian 16-bit samples."""
    frames = len(samples) // (channels * 2)
    _check(channels, rate, frames)
    align = channels * 2
    fmt = _fmt(FMT_PCM, channels, rate, rate * align, align, 16)
    return _riff(fmt, samples[:frames * align])


def adpcm_wem(samples: bytes, channels: int, rate: int) -> bytes:
    """A Wwise ADPCM media object from interleaved 16-bit samples.

    Wwise blocks the stream per channel: each channel gets its own 0x24-byte block
    holding the running predictor and step index, then 64 four-bit deltas. The
    encoder therefore keeps one IMA state per channel and re-stamps it into every
    block header, which is what lets a decoder start from any block.
    """
    frames = len(samples) // (channels * 2)
    _check(channels, rate, frames)
    align = ADPCM_BLOCK * channels
    blocks = (frames + SAMPLES_PER_BLOCK - 1) // SAMPLES_PER_BLOCK
    out = bytearray(blocks * align)

    for channel in range(channels):
        predictor = 0
        index = 0
        for block in range(blocks):
            at = block * align + channel * ADPCM_BLOCK
            struct.pack_into("<hh", out, at, predictor, index)
            for i in range(SAMPLES_PER_BLOCK):
                frame = block * SAMPLES_PER_BLOCK + i
                offset = (frame * channels + channel) * 2
                # Past the end of the clip the block is filled by holding the last
                # value, so the tail decodes as silence rather than as a click.
                if offset + 2 <= len(samples):
                    target = struct.unpack_from("<h", samples, offset)[0]
                else:
                    target = predictor

                step = _STEP_TABLE[index]
                diff = target - predictor
                nibble = 8 if diff < 0 else 0
                if diff < 0:
                    diff = -diff
                delta = step >> 3
                if diff >= step:
                    nibble |= 4
                    diff -= step
                    delta += step
                if diff >= step >> 1:
                    nibble |= 2
                    diff -= step >> 1
                    delta += step >> 1
                if diff >= step >> 2:
                    nibble |= 1
                    delta += step >> 2

                predictor += -delta if nibble & 8 else delta
                predictor = max(-32768, min(32767, predictor))
                index = max(0, min(88, index + _INDEX_TABLE[nibble & 7]))

                byte_at = at + ADPCM_HEADER + (i >> 1)
                if i & 1:
                    out[byte_at] |= (nibble & 0x0F) << 4
                else:
                    out[byte_at] = nibble & 0x0F

    fmt = _fmt(FMT_ADPCM, channels, rate,
               rate * align // SAMPLES_PER_BLOCK, align, 4)
    return _riff(fmt, bytes(out))


def silence(channels: int = 1, rate: int = 24000, frames: int = 64) -> bytes:
    """The smallest media object that plays nothing.

    Muting a sound is a replacement like any other - the engine still resolves the
    id, still plays it, and simply has nothing to hear. That is far safer than
    removing the media, which would leave every object that references it dangling.
    """
    return adpcm_wem(b"\x00\x00" * channels * max(frames, SAMPLES_PER_BLOCK),
                     channels, rate)


def encode(samples: bytes, channels: int, rate: int, codec: str = "adpcm") -> bytes:
    if codec == "pcm":
        return pcm_wem(samples, channels, rate)
    if codec == "adpcm":
        return adpcm_wem(samples, channels, rate)
    raise EncodeError(f"cannot write {codec!r} - only adpcm and pcm can be encoded")
