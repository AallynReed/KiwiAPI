"""Turn a list of edits from the browser into a bank the game will load.

The split of work here is deliberate. The browser decodes whatever audio file the
user picked - mp3, wav, ogg, flac, m4a, whatever it can already play - resamples
it and sends plain 16-bit samples. That means this side needs no audio decoder,
no ffmpeg, and no opinion about container formats; it only has to write Wwise's
own encoding, which is the part a browser cannot do.

What comes back is either the rebuilt ``.bnk`` or that bank wrapped in a ``.tmod``
ready to drop into the game's mods folder.

Nothing is stored. The bank is read from the archive, the edits are applied in
memory, and the result is streamed straight back to whoever asked for it.
"""

from __future__ import annotations

import logging
import re

from app.core.errors import APIError, ErrorCode
from app.trove import tmod
from app.trove.audio import edit, encode

logger = logging.getLogger("kiwi.audio")

MAX_EDITS = 200
MAX_CLIP_BYTES = 16 * 1024 * 1024        # per replacement, as raw 16-bit PCM
MAX_TOTAL_CLIP_BYTES = 28 * 1024 * 1024

# Trove matches a .tmod's filename against the title stored inside it, so the
# title has to survive being used as a filename. Anything a filesystem would
# object to goes; what is left is collapsed so stripping a character cannot leave
# a double space behind. The page applies the identical rule before it names the
# download - if the two ever disagreed, the game would reject the mod.
_UNSAFE = re.compile(r"""[^A-Za-z0-9 _.\-()&'!,]+""")
_SPACES = re.compile(r"\s+")
_EVENT_NAME = re.compile(r"^[A-Za-z0-9_]{3,96}$")


def _fail(message: str) -> APIError:
    return APIError(400, ErrorCode.bad_request, message)


def safe_title(title: str) -> str:
    cleaned = _SPACES.sub(" ", _UNSAFE.sub(" ", title or "")).strip()[:80].strip()
    return cleaned or "Trove Sound Pack"


def _clip(clips: dict[str, bytes], edit_spec: dict) -> tuple[bytes, int, int]:
    """Pull one uploaded clip out and sanity-check the shape it claims to be."""
    key = str(edit_spec.get("clip") or "")
    samples = clips.get(key)
    if samples is None:
        raise _fail("A replacement is missing its audio.")
    channels = int(edit_spec.get("channels") or 1)
    rate = int(edit_spec.get("rate") or 0)
    if channels not in (1, 2):
        raise _fail("Replacement audio must be mono or stereo.")
    if not 1000 <= rate <= 96000:
        raise _fail("Replacement audio has an unusable sample rate.")
    if len(samples) < channels * 2:
        raise _fail("A replacement clip is empty.")
    return samples, channels, rate


def apply_edits(raw: bytes, spec: dict, clips: dict[str, bytes]) -> edit.Result:
    """Build the edited bank. Pure and CPU-bound - call it in a thread."""
    edits = spec.get("edits") or []
    if not isinstance(edits, list) or not edits:
        raise _fail("No changes were requested.")
    if len(edits) > MAX_EDITS:
        raise _fail(f"At most {MAX_EDITS} changes can be applied at once.")
    total = sum(len(v) for v in clips.values())
    if total > MAX_TOTAL_CLIP_BYTES:
        raise _fail("That is more replacement audio than one build can carry.")
    for blob in clips.values():
        if len(blob) > MAX_CLIP_BYTES:
            raise _fail("One of the replacement clips is too long.")

    codec = str(spec.get("codec") or "adpcm")
    if codec not in ("adpcm", "pcm"):
        raise _fail("Replacement audio can only be written as ADPCM or PCM.")

    replacements: dict[int, bytes] = {}
    additions: list[edit.Addition] = []
    for item in edits:
        if not isinstance(item, dict):
            raise _fail("A change was not understood.")
        kind = str(item.get("kind") or "")
        try:
            if kind == "mute":
                replacements[int(item["id"])] = edit.mute()
            elif kind == "replace":
                samples, channels, rate = _clip(clips, item)
                replacements[int(item["id"])] = encode.encode(samples, channels, rate, codec)
            elif kind == "add":
                name = str(item.get("event") or "").strip()
                if not _EVENT_NAME.match(name):
                    raise _fail("An event name must be 3-96 letters, digits or underscores.")
                samples, channels, rate = _clip(clips, item)
                clone = item.get("clone_from")
                additions.append(edit.Addition(
                    event_name=name,
                    media=encode.encode(samples, channels, rate, codec),
                    clone_from=int(clone) if clone is not None else None,
                ))
            else:
                raise _fail(f"Unknown change type {kind!r}.")
        except (KeyError, TypeError, ValueError):
            raise _fail("A change was missing something it needed.") from None
        except encode.EncodeError as exc:
            raise _fail(f"That audio could not be converted: {exc}") from None

    try:
        return edit.rebuild(raw, replacements, additions)
    except edit.EditError as exc:
        raise _fail(str(exc)) from None


def package(result: edit.Result, bank_path: str, spec: dict) -> tuple[bytes, str, str]:
    """Return ``(bytes, filename, media type)`` for the requested output."""
    stem = bank_path.rsplit("/", 1)[-1]
    if str(spec.get("output") or "bnk") != "tmod":
        return result.data, stem, "application/octet-stream"

    mod = spec.get("mod") or {}
    title = safe_title(str(mod.get("title") or ""))
    properties = {
        "title": title,
        "author": str(mod.get("author") or "")[:64],
        "notes": str(mod.get("notes") or "")[:500],
        "type": "Audio",
    }
    try:
        # The archive path IS the path the game loads the bank from, so the mod
        # overrides the real file simply by shipping it at the same place.
        blob = tmod.build_tmod(1, properties, [(bank_path, result.data)])
    except tmod.TmodError as exc:
        raise _fail(f"The mod could not be packed: {exc}") from None
    # Trove matches a .tmod's filename against the title inside it, so the two
    # must agree exactly, case included.
    return blob, f"{title}.tmod", "application/octet-stream"
