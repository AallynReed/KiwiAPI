"""Turn a ``.bnk`` blob into a browsable, cached list of playable sounds.

Splitting the work in two keeps opening a bank cheap even when the bank is not.
``mus_main.bnk`` is 87 MB of music; decoding all of it to answer "what is in
here?" would be minutes of work thrown away the moment someone plays one track.

So the *manifest* - ids, names, codecs, durations - is built without decoding
anything: the header, the media index and the sidecar are all that is read, and
each sound's raw ``.wem`` is filed into the shared content store on the way past.
It is cached under the bank's own content hash, so a bank that ships unchanged
across fifty game versions is indexed once.

Decoding then happens per sound, on demand, from the stored ``.wem`` - so playing
one effect never touches the other 1,675 in the file. The result is served with
the media's own hash as its ``ETag``, which is what actually keeps repeat plays
free.
"""

from __future__ import annotations

import asyncio
import io
import logging
import zipfile

from app.core.config import settings
from app.core.errors import APIError, ErrorCode
from app.trove.audio import bank as bank_reader
from app.trove.audio import names as name_reader
from app.trove.audio import wem as wem_reader
from app.trove.render import bp_cache
from app.trove.updates.cas import ContentStore

logger = logging.getLogger("kiwi.audio")

_store = ContentStore(settings.mods_store_dir)

MAX_ZIP_BYTES = 512 * 1024 * 1024


def _build_sync(raw: bytes, sidecar: str | None) -> dict:
    """Index one bank. CPU-bound but decode-free - run it in a thread."""
    parsed = bank_reader.parse(raw)
    sounds_by_id: dict[int, name_reader.SoundName] = {}
    events: dict[int, str] = {}
    if sidecar:
        sounds_by_id, events = name_reader.parse(sidecar)

    played_by = parsed.sound_for_media()
    sounds = []
    for entry in parsed.media:
        data = parsed.media_bytes(entry)
        sha, _ = _store.put(data)
        named = sounds_by_id.get(entry.media_id)
        record = {
            "id": entry.media_id,
            "name": named.name if named else None,
            "group": named.group if named else "",
            "path": named.path if named else "",
            "source": named.source if named else "",
            "notes": named.notes if named else "",
            "bytes": entry.size,
            "sha": sha,
            "object_id": (o.object_id if (o := played_by.get(entry.media_id)) else None),
        }
        try:
            info = wem_reader.parse(data)
        except wem_reader.WemError as exc:
            # A media object we cannot read still belongs in the list - it just
            # cannot be played, and saying why beats leaving a silent gap.
            record |= {"codec": None, "channels": 0, "sample_rate": 0,
                       "duration": 0.0, "error": str(exc)}
        else:
            record |= {"codec": info.codec, "channels": info.channels,
                       "sample_rate": info.sample_rate,
                       "duration": round(info.duration, 3), "error": None}
        sounds.append(record)

    sounds.sort(key=lambda s: (s["name"] or "").lower() or f"~{s['id']}")
    playable = sum(1 for s in sounds if s["error"] is None)
    return {
        "bank": {
            "version": parsed.version,
            "bank_id": parsed.bank_id,
            "sections": [s.tag for s in parsed.sections],
            "objects": len(parsed.objects),
            "events": len(events),
        },
        "sounds": sounds,
        "count": len(sounds),
        "playable": playable,
        "total_duration": round(sum(s["duration"] for s in sounds), 1),
    }


async def manifest(raw: bytes, content_sha: str,
                   sidecar: str | None, sidecar_sha: str | None = None) -> dict:
    """The cached sound index for one bank, building it on a miss."""

    async def build() -> dict:
        try:
            return await asyncio.to_thread(_build_sync, raw, sidecar)
        except bank_reader.BankError as exc:
            logger.info("audio: cannot read bank %s: %s", content_sha[:12], exc)
            raise APIError(422, ErrorCode.bad_request,
                           "This file could not be read as a Wwise sound bank.") from None

    cached = await bp_cache.get_or_build(bp_cache.key_for_bank(content_sha, sidecar_sha), build)
    return await asyncio.to_thread(cached.payload)


async def audio_bytes(sha: str) -> tuple[bytes, str, str] | None:
    """Decode one stored ``.wem`` into something a browser can play."""
    data = await asyncio.to_thread(_store.get, sha)
    if data is None:
        return None
    try:
        return await asyncio.to_thread(wem_reader.convert, data)
    except wem_reader.WemError as exc:
        raise APIError(422, ErrorCode.bad_request, f"This sound could not be decoded: {exc}") from None


async def raw_bytes(sha: str) -> bytes | None:
    """The undecoded ``.wem``, for anyone who wants the game's own bytes."""
    return await asyncio.to_thread(_store.get, sha)


def _zip_name(sound: dict, extension: str, taken: set[str]) -> str:
    stem = sound.get("name") or f"sound_{sound['id']}"
    stem = "".join(c if c.isalnum() or c in "._- " else "_" for c in stem).strip() or "sound"
    group = sound.get("group") or ""
    group = "".join(c if c.isalnum() or c in "._- " else "_" for c in group).strip()
    base = f"{group}/{stem}" if group else stem
    name = f"{base}.{extension}"
    n = 2
    while name in taken:
        name = f"{base}_{n}.{extension}"
        n += 1
    taken.add(name)
    return name


def build_zip(sounds: list[dict]) -> bytes:
    """Every playable sound in one archive, foldered by its Wwise container.

    Already-compressed audio is stored rather than deflated - it would not shrink,
    and a 1,600-sound bank is slow enough to package without the wasted pass.
    """
    out = io.BytesIO()
    taken: set[str] = set()
    total = 0
    with zipfile.ZipFile(out, "w", zipfile.ZIP_STORED) as archive:
        for sound in sounds:
            if sound.get("error"):
                continue
            data = _store.get(sound["sha"])
            if data is None:
                continue
            try:
                decoded, _mime, extension = wem_reader.convert(data)
            except wem_reader.WemError:
                continue
            total += len(decoded)
            if total > MAX_ZIP_BYTES:
                raise APIError(413, ErrorCode.bad_request,
                               "These sounds are too large to download as one archive.")
            archive.writestr(_zip_name(sound, extension, taken), decoded)
    return out.getvalue()
