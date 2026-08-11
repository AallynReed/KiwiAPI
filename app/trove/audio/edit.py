"""Write a modified sound bank back out.

Three things can change and each has a knock-on the engine will notice:

**Replace a sound.** New bytes are rarely the same length as the old ones, so
every media entry after it shifts. ``DIDX`` has to be re-laid-out (16-byte
aligned, as Trove's own banks are), ``DATA`` rebuilt around it - and, easy to
miss, the ``HIRC`` Sound object that plays it carries its *own* copy of the media
size. Leave that stale and the engine reads the wrong number of bytes.

**Mute a sound.** A replacement like any other, with a clip of silence. Deleting
the media instead would leave every object referencing it dangling.

**Add a sound.** A bank only ever plays what the game asks it for, and the game
asks by Event id - so a new sound needs a new Event, and an Event id is just the
FNV-1 hash of its name (see :func:`app.trove.audio.names.event_id`). Trove's Flash
UI posts events *by name* through ``ExternalInterface.call("POST_SOUND_EVENT", …)``,
so a modded interface file can trigger one that never existed before.

The new Event and its Play Action are written from scratch - their layouts are
small and fully known. The Sound object is *cloned* from an existing one instead,
because everything after its first thirteen bytes is a parameter tree (bus
routing, positioning, RTPCs) that this module has no business trying to author.
Cloning inherits a working one and re-points only the ids.

Everything not touched is copied through byte for byte.
"""

from __future__ import annotations

import struct
from dataclasses import dataclass

from app.trove.audio import bank as bank_reader
from app.trove.audio import encode, names

# Trove's banks align every media blob to 16 bytes; matching that keeps a rebuilt
# bank byte-comparable with the original for everything that did not change.
DATA_ALIGN = 16

ACTION_PLAY = 0x0403

# A Play action, in full: object id, type, target, then the two empty property
# bundles, a flag byte and the id of the bank holding the target. Every Play
# action in Trove's banks is exactly these 18 bytes.
_ACTION_SIZE = 18
_ACTION_FLAGS = 0x04

MAX_ADDITIONS = 64


class EditError(Exception):
    """The requested change cannot be applied to this bank."""


@dataclass(slots=True)
class Addition:
    """One brand-new sound, reachable through an event of its own."""
    event_name: str
    media: bytes
    clone_from: int | None = None      # media id of the Sound to model it on


@dataclass(slots=True)
class Result:
    data: bytes
    replaced: int
    added: list[dict]                  # {event, event_id, media_id, object_id}


def _free_id(taken: set[int], seed: int) -> int:
    """A stable unused 32-bit id derived from *seed*.

    Deterministic so that rebuilding the same edits twice produces the same bank,
    which is what makes the output diffable and a re-download identical."""
    value = seed & 0xFFFFFFFF
    while value in taken or value == 0:
        value = (value * 2654435761 + 1) & 0xFFFFFFFF
    taken.add(value)
    return value


def _sound_template(parsed: bank_reader.Bank, prefer: int | None) -> bank_reader.HircObject:
    """An in-memory Sound object to model a new one on."""
    by_media = parsed.sound_for_media()
    if prefer is not None and prefer in by_media:
        return by_media[prefer]
    if not by_media:
        raise EditError("this bank has no sound objects to model a new one on")
    # Otherwise the smallest one - fewest parameters to inherit by accident.
    return min(by_media.values(), key=lambda o: o.size)


def rebuild(raw: bytes,
            replacements: dict[int, bytes] | None = None,
            additions: list[Addition] | None = None) -> Result:
    """Return the bank with *replacements* applied and *additions* appended."""
    replacements = dict(replacements or {})
    additions = list(additions or [])
    if len(additions) > MAX_ADDITIONS:
        raise EditError(f"at most {MAX_ADDITIONS} new sounds can be added at once")

    parsed = bank_reader.parse(raw)
    known = {m.media_id for m in parsed.media}
    for media_id in replacements:
        if media_id not in known:
            raise EditError(f"this bank has no sound {media_id}")
    if additions and not parsed.media:
        raise EditError("this bank holds no embedded media to extend")

    taken_objects = {o.object_id for o in parsed.objects} | known
    new_objects = bytearray()
    added: list[dict] = []
    extra_media: list[tuple[int, bytes]] = []

    for addition in additions:
        name = addition.event_name.strip()
        if not name:
            raise EditError("a new sound needs an event name")
        event_id = names.event_id(name)
        if event_id in taken_objects:
            raise EditError(f"this bank already has an event named {name!r}")
        taken_objects.add(event_id)

        template = _sound_template(parsed, addition.clone_from)
        media_id = _free_id(taken_objects, event_id ^ 0x5F5F5F5F)
        object_id = _free_id(taken_objects, event_id ^ 0xA5A5A5A5)
        action_id = _free_id(taken_objects, event_id ^ 0x3C3C3C3C)

        body = bytearray(parsed.raw[template.offset:template.offset + template.size])
        struct.pack_into("<I", body, 0, object_id)
        struct.pack_into("<I", body, bank_reader._SOURCE_ID_AT, media_id)
        struct.pack_into("<I", body, bank_reader._MEDIA_SIZE_AT, len(addition.media))
        new_objects += _object(bank_reader.HIRC_SOUND, bytes(body))

        action = struct.pack("<IHIBBBBI", action_id, ACTION_PLAY, object_id,
                             0, 0, 0, _ACTION_FLAGS, parsed.bank_id)
        if len(action) != _ACTION_SIZE:      # guards the format string above
            raise EditError("internal: malformed play action")
        new_objects += _object(bank_reader.HIRC_ACTION, action)
        new_objects += _object(bank_reader.HIRC_EVENT,
                               struct.pack("<IBI", event_id, 1, action_id))

        extra_media.append((media_id, addition.media))
        added.append({"event": name, "event_id": event_id,
                      "media_id": media_id, "object_id": object_id})

    return _assemble(parsed, replacements, extra_media, bytes(new_objects),
                     len(added) * 3, added)


def _object(kind: int, payload: bytes) -> bytes:
    return bytes([kind]) + struct.pack("<I", len(payload)) + payload


def _assemble(parsed: bank_reader.Bank, replacements: dict[int, bytes],
              extra_media: list[tuple[int, bytes]], new_objects: bytes,
              new_count: int, added: list[dict]) -> Result:
    """Lay out DIDX/DATA afresh and copy every other section through."""
    blobs: list[tuple[int, bytes]] = [
        (m.media_id, replacements.get(m.media_id) or parsed.media_bytes(m))
        for m in parsed.media
    ] + extra_media

    didx = bytearray()
    data = bytearray()
    sizes: dict[int, int] = {}
    for media_id, blob in blobs:
        if len(data) % DATA_ALIGN:
            data += b"\x00" * (DATA_ALIGN - len(data) % DATA_ALIGN)
        didx += struct.pack("<III", media_id, len(data), len(blob))
        data += blob
        sizes[media_id] = len(blob)

    out = bytearray()
    for section in parsed.sections:
        payload = parsed.raw[section.offset:section.offset + section.size]
        if section.tag == "DIDX":
            payload = bytes(didx)
        elif section.tag == "DATA":
            payload = bytes(data)
        elif section.tag == "HIRC":
            payload = _patch_hirc(parsed, payload, sizes, new_objects, new_count)
        out += section.tag.encode("ascii") + struct.pack("<I", len(payload)) + payload

    return Result(data=bytes(out), replaced=len(replacements), added=added)


def _patch_hirc(parsed: bank_reader.Bank, payload: bytes, sizes: dict[int, int],
                new_objects: bytes, new_count: int) -> bytes:
    """Refresh each Sound object's copy of its media size, then append new objects.

    ``uInMemoryMediaSize`` is the field that goes stale on a replacement: DIDX says
    how many bytes are there, and this says how many the engine should read. They
    have to agree.
    """
    body = bytearray(payload)
    # Object offsets were recorded against the whole file; rebase them onto the
    # section payload we are editing here.
    start = next(s.offset for s in parsed.sections if s.tag == "HIRC")
    for obj in parsed.objects:
        if obj.kind != bank_reader.HIRC_SOUND or obj.media_size_at is None:
            continue
        size = sizes.get(obj.source_id)
        if size is None:
            continue
        struct.pack_into("<I", body, obj.media_size_at - start, size)
    if new_count:
        struct.pack_into("<I", body, 0, struct.unpack_from("<I", body, 0)[0] + new_count)
        body += new_objects
    return bytes(body)


def mute(channels: int = 1, rate: int = 24000) -> bytes:
    """The media object a muted sound is replaced with."""
    return encode.silence(channels=channels, rate=rate)
