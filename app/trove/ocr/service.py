"""OCR orchestration: image bytes -> recognized character stats.

Thin layer over engine + parser. The engine call is blocking + CPU-bound, so it
runs in a worker thread to keep the event loop responsive under concurrent
requests. After the main parse, a box-aware recovery pass re-OCRs the row of any
known label whose value the full-image detector missed (RapidOCR occasionally
drops a lone glyph - e.g. a stat's '0' - even though the label box is found).
"""
from __future__ import annotations

import asyncio

from app.trove.ocr import engine, parse
from app.trove.ocr import vocabulary as vocab


def _process(data: bytes) -> dict:
    arr, elems = engine.recognize(data)
    lines = [e.text for e in elems]
    result = parse.extract_character_stats(lines)
    stats = result["stats"]

    # Recovery: for each KNOWN stat the parse didn't find, see if its label was
    # nonetheless detected (as a box) and re-OCR that row to recover the value the
    # detector dropped. Bounded + safe: only fires for missing stats, requires a
    # type match, and the recovered read is marked lower-confidence.
    missing = set(vocab.all_keys()) - set(stats)
    if missing:
        for el in elems:
            if not el.box or not missing:
                continue
            m = vocab.match_label(el.text)
            if m is None or m[0] not in missing:
                continue
            key = m[0]
            sd = vocab.stat_def(key)
            for txt in engine.ocr_row(arr, el.box):
                parsed = parse.parse_number(txt)
                if parsed is None:
                    continue
                value, is_pct = parsed
                if sd is not None and (sd.type == "percent") != is_pct:
                    continue  # type mismatch - not this stat's value
                in_range = sd is None or (sd.min <= value <= sd.max)
                if not in_range:
                    continue
                stats[key] = {
                    "value": int(round(value)) if (sd and sd.type == "int") else value,
                    "unit": "percent" if (sd and sd.type == "percent") else "count",
                    "raw": txt.strip(),
                    "confidence": 0.7,                  # recovered via targeted re-OCR
                    "in_range": True,
                    "type_match": True,
                    "derived": False,
                }
                missing.discard(key)
                break

    result["matched"] = len(stats)
    result["lines"] = lines
    return result


async def extract_from_image(data: bytes) -> dict:
    """Recognize Trove character stats from a screenshot's bytes.

    Returns parse.extract_character_stats()'s dict plus the raw ``lines`` the OCR
    produced (transparency - lets a caller see what the recognizer actually read).
    Propagates ``engine.OcrUnavailable`` (engine not installed) and ``ValueError``
    (unreadable image) for the endpoint to translate into HTTP status codes."""
    return await asyncio.to_thread(_process, data)
