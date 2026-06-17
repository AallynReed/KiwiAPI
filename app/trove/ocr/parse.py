"""Turn OCR'd text lines into ``{stat_key: value}`` with confidence.

Engine-agnostic: input is a list of ordered text lines (any local OCR engine
produces these); no bounding boxes required. Per line we split out the numbers,
match the text fragments around them against the closed vocabulary, and pair each
label with the adjacent number - handling BOTH layouts seen in-game:

  "14,690 Physical Damage"                 (value before label, single column)
  "Critical Hit 114.8%"                    (value after label)
  "Physical Damage 799,894 Critical Hit 114.8%"  (two columns merged onto one line)

Pairing is type-aware: when a label sits between two numbers, the one whose kind
(percent vs integer) matches the stat wins - so a percent value never gets
attached to an integer stat. Each value is then sanity-checked against the stat's
expected type + plausible range; a failure lowers confidence (and flags the field)
rather than silently returning a wrong number.

The game (even its French client) uses commas for thousands and '.' for decimals,
so a space is always a separator and never part of a number - which sidesteps the
"is '4 256' one number or two" ambiguity.
"""
from __future__ import annotations

import re
from dataclasses import dataclass

from app.trove import stats as game_stats
from app.trove.ocr import vocabulary as vocab

# A number is a digit run with interspersed group/decimal separators (',' or '.'),
# optional trailing %. We allow BOTH separators inside one token (e.g. "2.716,754")
# because OCR routinely confuses ',' and '.'; _parse_number sorts out which is the
# decimal. Spaces never belong to a number (see module docstring).
_NUMBER_RE = re.compile(r"\d[\d.,]*\d\s*%?|\d\s*%?")


@dataclass
class _Elem:
    kind: str           # "num" | "label"
    start: int
    value: float = 0.0
    is_percent: bool = False
    raw: str = ""
    key: str = ""
    score: float = 0.0


def _parse_number(token: str) -> tuple[float, bool] | None:
    """Parse an OCR number token to ``(value, is_percent)``.

    The game uses ',' for thousands and '.' for the decimal, but OCR frequently
    misreads one as the other (so "636,088" comes back as "636.088"). A thousands
    group is ALWAYS 3 digits, so we treat the final separator as the decimal point
    only when it's followed by 1-2 digits; every other ',' or '.' is a thousands
    separator. This recovers "636.088"->636088, "2.716,754"->2716754, and keeps
    "114.8"->114.8 / "3,438.3"->3438.3 / "799,894"->799894."""
    t = token.strip().replace(" ", "")
    is_pct = t.endswith("%")
    if is_pct:
        t = t[:-1]
    if not t:
        return None
    seps = [i for i, c in enumerate(t) if c in ".,"]
    if seps:
        last, frac = seps[-1], t[seps[-1] + 1:]
        if frac.isdigit() and 1 <= len(frac) <= 2:        # trailing 1-2 digits = decimal
            num_str = t[:last].replace(",", "").replace(".", "") + "." + frac
        else:                                              # all separators are grouping
            num_str = t.replace(",", "").replace(".", "")
    else:
        num_str = t
    try:
        return float(num_str), is_pct
    except ValueError:
        return None


def parse_number(token: str) -> tuple[float, bool] | None:
    """Public wrapper around the number parser (used by the OCR recovery pass)."""
    return _parse_number(token)


def _line_elements(line: str) -> list[_Elem]:
    """Ordered numbers + matched labels on one line. Label candidates are the text
    fragments between/around the numbers (the numbers delimit them)."""
    elems: list[_Elem] = []
    last = 0

    def _add_label(chunk: str, start: int) -> None:
        if chunk.strip():
            m = vocab.match_label(chunk)
            if m:
                elems.append(_Elem("label", start, key=m[0], score=m[1]))

    for m in _NUMBER_RE.finditer(line):
        parsed = _parse_number(m.group())
        if parsed is None:
            continue  # leave this text for the surrounding label chunks
        _add_label(line[last:m.start()], last)
        val, pct = parsed
        elems.append(_Elem("num", m.start(), value=val, is_percent=pct, raw=m.group().strip()))
        last = m.end()
    _add_label(line[last:], last)

    elems.sort(key=lambda e: e.start)
    return elems


# Cross-line pairing: some labels render with their value on a SEPARATE OCR line,
# sometimes with other text between (Coefficient, and "Rg de pouvoir"/Power Rank in
# the French equipment view). We bridge only a small window and require a type
# match, and penalise the confidence so a heuristic read is distinguishable.
_CROSSLINE_WINDOW = 4
_CROSSLINE_PENALTY = 0.85


def _record(key: str, value: float, is_percent: bool, raw: str, base_score: float) -> dict:
    """Build one stat record, validating type + range and folding both into the
    confidence."""
    sd = vocab.stat_def(key)
    type_match = sd is None or ((sd.type == "percent") == is_percent)
    in_range = sd is None or (sd.min <= value <= sd.max)
    conf = base_score
    if not in_range:
        conf *= 0.4
    if not type_match:
        conf *= 0.6
    val = int(round(value)) if (sd and sd.type == "int") else value
    return {
        "value": val,
        "unit": "percent" if (sd and sd.type == "percent") else "count",
        "raw": raw,
        "confidence": round(conf, 3),
        "in_range": in_range,
        "type_match": type_match,
        "derived": False,
    }


def _insert(out: dict, key: str, rec: dict) -> None:
    """Keep the highest-confidence record per stat across duplicate sightings."""
    prev = out.get(key)
    if prev is None or rec["confidence"] > prev["confidence"]:
        out[key] = rec


def _reconcile_coefficient(out: dict) -> None:
    """Coefficient is a DERIVED stat and often isn't shown on the sheet at all.
    The game computes it as ``floor(D * (1 + CritDamage/100))`` where ``D`` is the
    HIGHER of Physical / Magic Damage. So instead of trusting an OCR read of that
    long 8+ digit number, we compute it from the (near-always-present) damage +
    crit-damage reads:

      - not read but derivable  -> fill it in (``derived: true``)
      - read AND derivable, agree -> keep the exact derived value, mark confirmed
      - read AND derivable, disagree -> keep the read value but flag low confidence
        (the read and its inputs are inconsistent - something was misread)
      - not derivable -> leave whatever was read (or nothing)
    """
    crit = out.get("critical_damage")
    phys = out.get("physical_damage")
    magic = out.get("magic_damage")
    result = game_stats.compute_coefficient(
        phys["value"] if phys else None,
        magic["value"] if magic else None,
        crit["value"] if crit else None,
    )
    if result is None:
        return
    derived, _which = result
    sd = vocab.stat_def("coefficient")
    in_range = sd is None or (sd.min <= derived <= sd.max)
    inputs = [out[k] for k in ("physical_damage", "magic_damage", "critical_damage") if k in out]
    input_conf = min(s["confidence"] for s in inputs)
    read = out.get("coefficient")
    if read is None:
        out["coefficient"] = {
            "value": derived, "unit": "count", "raw": "",
            "confidence": round(input_conf, 3), "in_range": in_range,
            "type_match": True, "derived": True,
        }
        return
    tol = max(round(derived * 0.005), 5)
    if abs(read["value"] - derived) <= tol:
        read["value"] = derived                          # exact; fixes a last-digit slip
        read["confidence"] = round(max(read["confidence"], input_conf), 3)
    else:
        read["confidence"] = round(read["confidence"] * 0.5, 3)   # read vs inputs disagree


def extract_character_stats(lines: list[str]) -> dict:
    """Extract the known character stats from OCR text lines.

    Returns ``{"stats": {key: {value, unit, raw, confidence, in_range, type_match,
    derived}}, "matched": n, "total_known": m}``. ``confidence`` (0..1) folds the
    label-match quality with the validation result; ``in_range`` / ``type_match``
    flag a read that parsed but looks implausible; ``derived`` is true for a value
    computed rather than read (Coefficient when it isn't on the sheet)."""
    out: dict[str, dict] = {}
    unpaired: list[tuple[int, str, float]] = []          # (line_idx, key, score)
    orphans: list[tuple[int, float, bool, str]] = []     # (line_idx, value, is_percent, raw)

    for li, line in enumerate(lines):
        elems = _line_elements(line)
        consumed: set[int] = set()
        for i, el in enumerate(elems):
            if el.kind != "label":
                continue
            sd = vocab.stat_def(el.key)
            # Adjacent numbers (immediately before / after this label).
            cands = [j for j in (i - 1, i + 1)
                     if 0 <= j < len(elems) and elems[j].kind == "num" and j not in consumed]
            if not cands:
                unpaired.append((li, el.key, el.score))   # may pair across lines
                continue
            # Prefer a number whose percent-ness matches the stat type, then the
            # nearer one (ties -> the preceding number, the single-column default).
            # Loop vars bound as defaults so the closure captures THIS iteration's.
            def _key(j: int, _i=i, _sd=sd, _elems=elems):
                num = _elems[j]
                type_ok = _sd is None or ((_sd.type == "percent") == num.is_percent)
                return (type_ok, -abs(j - _i))
            best_j = max(cands, key=_key)
            num = elems[best_j]
            consumed.add(best_j)
            _insert(out, el.key, _record(el.key, num.value, num.is_percent, num.raw, el.score))

        # Numbers left unconsumed on this line are orphans - their label may be on
        # another line (claimed by the cross-line pass below).
        for idx, el in enumerate(elems):
            if el.kind == "num" and idx not in consumed:
                orphans.append((li, el.value, el.is_percent, el.raw))

    # Cross-line pass: a known label with no number on its own line claims the
    # nearest type-matching orphan within a small window (e.g. Power Rank in the
    # French equipment view, or a Coefficient rendered apart from its value).
    used: set[int] = set()
    for li, key, score in unpaired:
        if key in out:
            continue
        sd = vocab.stat_def(key)
        best: tuple[int, int] | None = None              # (distance, orphan_idx)
        for oi, (oli, _oval, opct, _oraw) in enumerate(orphans):
            if oi in used or abs(oli - li) > _CROSSLINE_WINDOW:
                continue
            if sd is not None and (sd.type == "percent") != opct:
                continue
            dist = abs(oli - li)
            if best is None or dist < best[0]:
                best = (dist, oi)
        if best is None:
            continue
        used.add(best[1])
        _oli, oval, opct, oraw = orphans[best[1]]
        _insert(out, key, _record(key, oval, opct, oraw, score * _CROSSLINE_PENALTY))

    _reconcile_coefficient(out)
    return {"stats": out, "matched": len(out), "total_known": len(vocab.all_keys())}
