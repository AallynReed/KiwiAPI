"""Recover human names for a bank's contents from the sidecar Wwise writes.

A ``.bnk`` identifies everything by 32-bit id and carries no names at all. But
Trove ships the build log Wwise emits beside each bank - ``ui.bnk`` has a
``ui.txt`` - and that file is a tab-separated table of exactly what was baked in:

    In Memory Audio   ID        Name                   Audio source file  ...
                      1099092   ui_gems_upgrade_sm_01  C:\\Trove\\...wem  ...

So a media id becomes ``ui_gems_upgrade_sm_01`` plus the Wwise object path it
lives at. That path is worth as much as the name: its second-to-last segment is
the container the sound belongs to, which is how the variations of one effect -
``ui_gems_upgrade_sm_01`` through ``_04`` - are grouped back together without
having to walk the object graph inside the bank.

The sidecar is a convenience, not a requirement: a bank with no ``.txt`` beside it
still lists, just with bare ids.
"""

from __future__ import annotations

from dataclasses import dataclass

# Rows are indented under a header line whose first column names the section.
AUDIO_SECTION = "In Memory Audio"
EVENT_SECTION = "Event"


@dataclass(slots=True)
class SoundName:
    name: str
    path: str            # Wwise object path
    group: str           # container the sound sits in, "" at the top level
    source: str          # the .wav/.wem the sound was built from
    notes: str


def _rows(text: str) -> dict[str, list[dict[str, str]]]:
    """Split the sidecar into ``section -> [row]``, keyed by each section's own
    header row so a column moving between Wwise versions cannot silently shift
    the data under it."""
    sections: dict[str, list[dict[str, str]]] = {}
    columns: list[str] = []
    current: list[dict[str, str]] | None = None
    for line in text.splitlines():
        if not line.strip():
            continue
        cells = line.split("\t")
        if cells[0].strip():
            columns = [c.strip() for c in cells]
            current = sections.setdefault(cells[0].strip(), [])
            continue
        if current is None:
            continue
        current.append({columns[i]: cells[i].strip()
                        for i in range(min(len(columns), len(cells)))})
    return sections


def _group(path: str) -> str:
    """The container a sound sits in - the path segment above the sound itself."""
    parts = [p for p in path.split("\\") if p]
    return parts[-2] if len(parts) >= 2 else ""


def parse(text: str) -> tuple[dict[int, SoundName], dict[int, str]]:
    """Return ``(media id -> name, event id -> event name)``."""
    sections = _rows(text)
    sounds: dict[int, SoundName] = {}
    for row in sections.get(AUDIO_SECTION, []):
        raw_id, name = row.get("ID", ""), row.get("Name", "")
        if not raw_id.isdigit() or not name:
            continue
        path = row.get("Wwise Object Path", "")
        source = row.get("Audio source file", "")
        # Only the file name is useful; the rest is the build machine's layout.
        source = source.replace("/", "\\").rsplit("\\", 1)[-1]
        sounds[int(raw_id)] = SoundName(name=name, path=path, group=_group(path),
                                        source=source, notes=row.get("Notes", ""))

    events: dict[int, str] = {}
    for row in sections.get(EVENT_SECTION, []):
        raw_id, name = row.get("ID", ""), row.get("Name", "")
        if raw_id.isdigit() and name:
            events[int(raw_id)] = name
    return sounds, events


def event_id(name: str) -> int:
    """Wwise's name hash: 32-bit FNV-1 over the lowercased name.

    This is how the game turns the string in
    ``ExternalInterface.call("POST_SOUND_EVENT", "Play_ui_button_select")`` into
    the id an Event object carries, so a bank can be given a *new* event simply by
    hashing the name it should answer to.
    """
    value = 2166136261
    for byte in name.lower().encode("utf-8"):
        value = (value * 16777619) & 0xFFFFFFFF
        value ^= byte
    return value


def sidecar_path(bank_path: str) -> str:
    """Where the sidecar for a bank lives: same directory, ``.txt`` instead."""
    return bank_path[: -len(".bnk")] + ".txt" if bank_path.endswith(".bnk") else bank_path + ".txt"
