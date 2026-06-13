"""Biome rotations (ported from BetterTroveTools home.py).

Three independent rotations, each enriched with sub-biome metadata from
gamedata/biomes.json:

- d15 ("normal"): the 3-hour adventure-world biome rotation (3 biomes at a time).
- wild mana: a weekly rotation (this week + the two prior, 3 biomes shown).
- stampy: a weekly 48-hour event biome.

All anchors/lists are copied verbatim from the source so the indices line up.
Timestamps are real-UTC unix seconds.
"""

from datetime import datetime, timedelta

from app.trove.server_time import UTC, _load, real_utc_now

DAY = 86400

# d15 rotation: three parallel sub-biome lists indexed by the same 3-hour offset.
_D15_EPOCH = datetime(2024, 6, 18, 14, 0, 0, tzinfo=UTC)   # 2024-06-18 14:00 UTC
_D15_INTERVAL = timedelta(hours=3)
_BIOME1 = [
    "Sundered Uplands", "Cerise Sandsea", "Deep Forest", "Alkali Flats",
    "Dead of Winter", "Sundered Uplands", "Firefly Party", "Desert of Secrets",
    "Weathered Wastelands", "Frozen Wastes", "Frigga's Fjord", "Abandoned Boneyard",
]
_BIOME2 = [
    "Cursed Vale", "Hollow Dunes", "Bewitching Wood", "Primal Preserve",
    "Hollow Dunes", "Ancient Heights", "Viking Burial Grounds", "Spellbound Thicket",
    "Saurian Swamp", "Restless Range", "Uncanny Valley",
]
_BIOME3 = [
    "Sugar Steppes", "Volcanic Fields", "The Lost Isles", "Luminopolis",
    "The Lost Isles", "Blazing Emberlands", "Cocoa Craters", "Data Spires",
    "The Lost Isles", "Cupcake Canyon", "Dragon's Teeth", "Luminopolis",
    "The Lost Isles", "Data Spires",
]

_MANA_BASE = datetime(2023, 11, 20, 11, 0, 0, tzinfo=UTC)
_MANA_BIOMES = [
    "Neon City", "Jurassic Jungle", "Dragonfire Peaks", "Forbidden Spires",
    "Sundered Uplands", "Medieval Highlands", "Permafrost", "Cursed Vale",
    "Desert Frontier", "Fae Forest", "Candoria",
]

_STAMPY_BASE = datetime(2023, 9, 30, 11, 0, 0, tzinfo=UTC)
_STAMPY_DURATION = timedelta(hours=48)
_STAMPY_BIOMES = [
    "Desert Frontier", "The Lost Isles", "Geode Topside", "Neon City", "Dragonfire Peaks",
    "Permafrost", "Candoria", "Cursed Vale", "Forbidden Spires", "Fae Forest",
    "Medieval Highlands", "Jurassic Jungle", "Sundered Uplands",
]

# Icon fallbacks for parent biomes not resolvable from biomes.json.
_FALLBACK = {
    "Neon City": "neon", "Jurassic Jungle": "dinosaur", "Dragonfire Peaks": "dragon",
    "Forbidden Spires": "spires", "Sundered Uplands": "giantland", "Medieval Highlands": "forest",
    "Permafrost": "tundra", "Cursed Vale": "undead", "Desert Frontier": "frontier",
    "Fae Forest": "fae", "Candoria": "candy", "Geode Topside": "dunes", "The Lost Isles": "pirate",
}


def _icon_map() -> dict:
    out: dict[str, str] = {}
    for sub in _load("biomes.json").values():
        parent = sub.get("biome")
        if parent and parent not in out:
            out[parent] = sub.get("icon", "unknown")
    return out


def _sub_biome(key: str) -> dict:
    """A d15 entry is a sub-biome key -> full metadata from biomes.json."""
    sb = _load("biomes.json").get(key)
    if sb:
        return {"name": sb.get("name", key), "final_name": sb.get("final_name", key),
                "icon": sb.get("icon", "unknown")}
    return {"name": key, "final_name": key, "icon": "unknown"}


def _parent_biome(name: str) -> dict:
    """A wild-mana/stampy entry is a parent biome name -> icon via the icon map."""
    icon = _icon_map().get(name) or _FALLBACK.get(name, "unknown")
    return {"name": name, "final_name": name, "icon": icon}


def biome_rotation(now: datetime | None = None, count: int = 16) -> dict:
    """The d15 (3-hour) adventure-world biome rotation: current + upcoming."""
    real = now or real_utc_now()
    interval = _D15_INTERVAL.total_seconds()
    elapsed = (real - _D15_EPOCH).total_seconds()
    consumed = int(elapsed // interval)
    start = real - timedelta(seconds=elapsed % interval)

    rotations = []
    for i in range(count + 1):
        offset = consumed + i
        s = start + i * _D15_INTERVAL
        rotations.append({
            "starts_at": int(s.timestamp()),
            "ends_at": int((s + _D15_INTERVAL).timestamp()),
            "biomes": [
                _sub_biome(_BIOME1[offset % len(_BIOME1)]),
                _sub_biome(_BIOME2[offset % len(_BIOME2)]),
                _sub_biome(_BIOME3[offset % len(_BIOME3)]),
            ],
        })
    return {"current": rotations[0], "upcoming": rotations[1:]}


def wild_mana(now: datetime | None = None, count: int = 8) -> dict:
    """The weekly Wild Mana biome rotation: current + upcoming."""
    real = now or real_utc_now()
    week = 7 * DAY
    weeks = int((real - _MANA_BASE).total_seconds() // week)
    out = []
    for i in range(count):
        w = weeks + i
        s = _MANA_BASE + timedelta(seconds=w * week)
        out.append({
            "starts_at": int(s.timestamp()),
            "ends_at": int((s + timedelta(seconds=week)).timestamp()),
            "biomes": [
                _parent_biome(_MANA_BIOMES[w % len(_MANA_BIOMES)]),
                _parent_biome(_MANA_BIOMES[(w - 1) % len(_MANA_BIOMES)]),
                _parent_biome(_MANA_BIOMES[(w - 2) % len(_MANA_BIOMES)]),
            ],
        })
    return {"current": out[0], "upcoming": out[1:]}


def stampy(now: datetime | None = None, count: int = 8) -> dict:
    """The weekly Stampy event (48-hour window): current + upcoming."""
    real = now or real_utc_now()
    weeks_offset = int((real - _STAMPY_BASE).total_seconds() // (7 * DAY))
    events = []
    for w in range(weeks_offset - 1, weeks_offset + 10):
        s = _STAMPY_BASE + timedelta(weeks=w)
        e = s + _STAMPY_DURATION
        if e > real:
            events.append({
                "starts_at": int(s.timestamp()),
                "ends_at": int(e.timestamp()),
                "biomes": [_parent_biome(_STAMPY_BIOMES[w % len(_STAMPY_BIOMES)])],
            })
            if len(events) == count:
                break
    return {"current": events[0] if events else None, "upcoming": events[1:]}
