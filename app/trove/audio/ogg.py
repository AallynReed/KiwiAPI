"""Just enough Ogg to wrap Vorbis packets back into a file.

Ogg splits every packet into 255-byte segments and records their lengths in a
page's segment table; a packet is over when a segment shorter than 255 arrives,
which is why one that divides evenly needs a trailing zero-length segment. At
most 255 segments fit in a page, so a long packet continues onto the next one,
which is then flagged so a decoder knows not to treat it as a fresh start.

Granule positions are per *page*, not per packet: a page carries the position of
the last packet that finishes on it, and ``-1`` when none does.
"""

from __future__ import annotations

import struct

MAX_SEGMENTS = 255

# Ogg's CRC is unusual - a plain MSB-first CRC-32 with no reflection and no final
# inversion, so the stock zlib one cannot stand in for it.
_CRC_TABLE: list[int] = []
for _n in range(256):
    _c = _n << 24
    for _ in range(8):
        _c = ((_c << 1) ^ 0x04C11DB7) & 0xFFFFFFFF if _c & 0x80000000 else (_c << 1) & 0xFFFFFFFF
    _CRC_TABLE.append(_c)


def _crc(data: bytes) -> int:
    crc = 0
    for byte in data:
        crc = ((crc << 8) & 0xFFFFFFFF) ^ _CRC_TABLE[((crc >> 24) & 0xFF) ^ byte]
    return crc


class OggWriter:
    """Collects packets, then lays them out as one Ogg bitstream.

    Call :meth:`flush` to force a page break - the Vorbis headers need it, since
    the identification packet has to sit alone on the first page.
    """

    def __init__(self, serial: int = 1) -> None:
        self._serial = serial & 0xFFFFFFFF
        self._pages: list[tuple[list[int], bytes, int, bool]] = []
        self._queue: list[tuple[bytes, int]] = []

    def write(self, packet: bytes, granule: int = 0) -> None:
        self._queue.append((packet, granule))

    def flush(self) -> None:
        """Close the current page group so the next packet starts a new page."""
        if not self._queue:
            return
        segments: list[int] = []
        body = bytearray()
        granule = -1
        continued = False
        for packet, pkt_granule in self._queue:
            lacing = [255] * (len(packet) // 255) + [len(packet) % 255]
            for i, length in enumerate(lacing):
                if len(segments) == MAX_SEGMENTS:
                    self._pages.append((segments, bytes(body), granule, continued))
                    # Only a page break that lands *inside* a packet continues it.
                    continued = i > 0
                    segments, body, granule = [], bytearray(), -1
                segments.append(length)
                start = i * 255
                body += packet[start:start + length]
            granule = pkt_granule
        self._pages.append((segments, bytes(body), granule, continued))
        self._queue.clear()

    def getvalue(self) -> bytes:
        self.flush()
        if not self._pages:
            return b""
        out = bytearray()
        last = len(self._pages) - 1
        for number, (segments, body, granule, continued) in enumerate(self._pages):
            flags = ((0x01 if continued else 0)
                     | (0x02 if number == 0 else 0)
                     | (0x04 if number == last else 0))
            head = bytearray(b"OggS")
            head.append(0)
            head.append(flags)
            head += struct.pack("<q", granule)
            head += struct.pack("<I", self._serial)
            head += struct.pack("<I", number)
            head += b"\x00\x00\x00\x00"          # CRC is computed over a zeroed field
            head.append(len(segments))
            head += bytes(segments)
            struct.pack_into("<I", head, 22, _crc(bytes(head) + body))
            out += head
            out += body
        return bytes(out)
