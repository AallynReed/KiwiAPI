"""Read a Wwise SoundBank (``.bnk``) - the container Trove ships its audio in.

A bank is a flat run of ``<4-byte tag><uint32 size><payload>`` sections. Four
matter here:

``BKHD``
    Version and bank id. Trove is bank version 128 throughout.
``DIDX``
    An index of the embedded media: twelve bytes per entry giving a media id and
    where its bytes sit inside ``DATA``.
``DATA``
    Those bytes, one ``.wem`` after another.
``HIRC``
    The object graph - sounds, containers, actions, events. Only two things are
    read from it: which object plays which media (so a media id can be traced
    back to an event name), and where each object records the size of the media
    it plays, because a rebuilt bank has to keep that number honest.

Everything else is passed through untouched when a bank is written back out.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass, field

BANK_VERSION = 128

HIRC_SOUND = 2
HIRC_ACTION = 3
HIRC_EVENT = 4

# Where an AkBankSourceData records the media it plays, measured from the start
# of a Sound object's payload: object id, plugin id, stream type, then the media
# id and its in-memory size.
_SOURCE_ID_AT = 9
_MEDIA_SIZE_AT = 13

MAX_SECTION_BYTES = 512 * 1024 * 1024


class BankError(Exception):
    """The bytes are not a SoundBank we can read."""


@dataclass(slots=True)
class Section:
    tag: str
    offset: int          # where the payload starts
    size: int


@dataclass(slots=True)
class Media:
    media_id: int
    offset: int          # relative to the start of DATA
    size: int


@dataclass(slots=True)
class HircObject:
    kind: int
    object_id: int
    offset: int          # where the payload starts, in the whole file
    size: int
    source_id: int | None = None    # media it plays, for Sound objects
    media_size_at: int | None = None


@dataclass(slots=True)
class Bank:
    raw: bytes = field(repr=False)
    version: int
    bank_id: int
    sections: list[Section]
    media: list[Media]
    objects: list[HircObject]

    @property
    def data_offset(self) -> int | None:
        for section in self.sections:
            if section.tag == "DATA":
                return section.offset
        return None

    def media_bytes(self, entry: Media) -> bytes:
        base = self.data_offset
        if base is None:
            raise BankError("bank has no DATA section")
        return self.raw[base + entry.offset:base + entry.offset + entry.size]

    def sound_for_media(self) -> dict[int, HircObject]:
        """Which Sound object plays each media id."""
        return {o.source_id: o for o in self.objects
                if o.kind == HIRC_SOUND and o.source_id is not None}


def _sections(raw: bytes) -> list[Section]:
    out: list[Section] = []
    off = 0
    while off + 8 <= len(raw):
        tag = raw[off:off + 4]
        size = struct.unpack_from("<I", raw, off + 4)[0]
        if size > MAX_SECTION_BYTES or off + 8 + size > len(raw):
            raise BankError(f"section '{tag.decode('latin1')}' runs past the end of the file")
        try:
            name = tag.decode("ascii")
        except UnicodeDecodeError:
            raise BankError("section tag is not ASCII") from None
        out.append(Section(name, off + 8, size))
        off += 8 + size
    if not out or out[0].tag != "BKHD":
        raise BankError("file does not start with a BKHD section")
    return out


def _media(raw: bytes, section: Section) -> list[Media]:
    if section.size % 12:
        raise BankError("DIDX size is not a whole number of entries")
    return [Media(*struct.unpack_from("<III", raw, section.offset + i * 12))
            for i in range(section.size // 12)]


def _objects(raw: bytes, section: Section) -> list[HircObject]:
    """Walk HIRC, recording each object's span and - for sounds - its media ref.

    Objects are ``<uint8 kind><uint32 size><payload>``. Only the first thirteen
    bytes of a Sound's payload are interpreted; the rest is an involved parameter
    tree that nothing here needs to understand.
    """
    if section.size < 4:
        raise BankError("HIRC section is too small")
    count = struct.unpack_from("<I", raw, section.offset)[0]
    out: list[HircObject] = []
    off = section.offset + 4
    end = section.offset + section.size
    for _ in range(count):
        if off + 5 > end:
            raise BankError("HIRC ends mid-object")
        kind = raw[off]
        size = struct.unpack_from("<I", raw, off + 1)[0]
        body = off + 5
        if size < 4 or body + size > end:
            raise BankError("HIRC object runs past the section")
        obj = HircObject(kind, struct.unpack_from("<I", raw, body)[0], body, size)
        if kind == HIRC_SOUND and size >= _MEDIA_SIZE_AT + 4:
            obj.source_id = struct.unpack_from("<I", raw, body + _SOURCE_ID_AT)[0]
            obj.media_size_at = body + _MEDIA_SIZE_AT
        out.append(obj)
        off = body + size
    return out


def parse(raw: bytes) -> Bank:
    sections = _sections(raw)
    header = sections[0]
    if header.size < 8:
        raise BankError("BKHD is too small")
    version, bank_id = struct.unpack_from("<II", raw, header.offset)
    if version != BANK_VERSION:
        raise BankError(f"unsupported bank version {version} (expected {BANK_VERSION})")

    media: list[Media] = []
    objects: list[HircObject] = []
    for section in sections:
        if section.tag == "DIDX":
            media = _media(raw, section)
        elif section.tag == "HIRC":
            objects = _objects(raw, section)

    if media:
        data = next((s for s in sections if s.tag == "DATA"), None)
        if data is None:
            raise BankError("DIDX present but DATA is missing")
        for entry in media:
            if entry.offset + entry.size > data.size:
                raise BankError(f"media {entry.media_id} runs past DATA")

    return Bank(raw=raw, version=version, bank_id=bank_id,
                sections=sections, media=media, objects=objects)
