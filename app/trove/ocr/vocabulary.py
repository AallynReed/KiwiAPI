"""The closed, multilingual Trove character-stat vocabulary + fuzzy label match.

Loaded from ``gamedata/character_stats.json``. Because the label set is CLOSED,
we don't need the OCR to read a label perfectly - we normalize (lowercase, strip
accents + punctuation) and fuzzy-match against the known synonyms, so "Physjcal
Damaqe" or the French "Dégâts physiques" both resolve to ``physical_damage``.
Stat defs also carry the expected type (int|percent) and a plausible max, used to
sanity-check a read in parse.py.
"""
from __future__ import annotations

import json
import re
import unicodedata
from dataclasses import dataclass
from difflib import SequenceMatcher
from functools import lru_cache
from pathlib import Path

_STATS_JSON = Path(__file__).parent.parent / "gamedata" / "character_stats.json"

# Label keys in the JSON that are NOT language synonym lists.
_META_KEYS = {"key", "type", "max", "min"}


def normalize(text: str) -> str:
    """Lowercase, strip accents (Dégâts -> degats) + punctuation, collapse spaces -
    so OCR noise, casing, accents, and stray separators don't block a match."""
    nfkd = unicodedata.normalize("NFKD", text)
    no_accents = "".join(c for c in nfkd if not unicodedata.combining(c))
    return re.sub(r"[^a-z0-9]+", " ", no_accents.lower()).strip()


@dataclass(frozen=True)
class StatDef:
    key: str
    type: str          # "int" | "percent"
    min: float
    max: float


@dataclass(frozen=True)
class _Synonym:
    key: str
    norm: str          # normalized label text


@lru_cache(maxsize=1)
def _load() -> tuple[dict[str, StatDef], tuple[_Synonym, ...]]:
    data = json.loads(_STATS_JSON.read_text(encoding="utf-8"))
    defs: dict[str, StatDef] = {}
    syns: list[_Synonym] = []
    for s in data["stats"]:
        defs[s["key"]] = StatDef(
            key=s["key"], type=s.get("type", "int"),
            min=float(s.get("min", 0)), max=float(s.get("max", 1e12)),
        )
        for field, vals in s.items():
            if field in _META_KEYS or not isinstance(vals, list):
                continue
            for label in vals:
                norm = normalize(label)
                if norm:
                    syns.append(_Synonym(key=s["key"], norm=norm))
    # Longest synonyms first so a longer, more specific label is preferred when
    # scores tie (e.g. "maximum health" over a hypothetical "health").
    syns.sort(key=lambda x: -len(x.norm))
    return defs, tuple(syns)


def stat_def(key: str) -> StatDef | None:
    return _load()[0].get(key)


def all_keys() -> list[str]:
    return list(_load()[0].keys())


def match_label(text: str, *, threshold: float = 0.82) -> tuple[str, float] | None:
    """Best ``(stat_key, score)`` for a label fragment, or ``None`` below
    ``threshold``. Exact normalized match scores 1.0; a label fully contained in
    the fragment scores 0.97; otherwise a fuzzy ratio (catches OCR typos)."""
    norm = normalize(text)
    if not norm:
        return None
    _defs, syns = _load()
    best: tuple[str, float] | None = None
    for syn in syns:
        if syn.norm == norm:
            score = 1.0
        elif syn.norm in norm:           # the whole label appears in the fragment
            score = 0.97
        else:
            score = SequenceMatcher(None, norm, syn.norm).ratio()
        if score >= threshold and (best is None or score > best[1]):
            best = (syn.key, score)
    return best
