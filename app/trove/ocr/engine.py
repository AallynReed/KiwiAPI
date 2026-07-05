"""Local OCR engine wrapper: image bytes -> ordered text lines.

Self-hosted via RapidOCR (ONNX-runtime PaddleOCR) - accurate on stylized game
fonts, multilingual, pip-only (no system OCR binary, no torch). The import is
GUARDED so the app still boots if the optional dependency (or its libGL system
lib) isn't present - the endpoint then returns a clear 503 instead of crashing
import. The model is loaded lazily + once (it's heavy) and the blocking call is
run off the event loop by the service layer.

Engine-swappable: any backend that turns image bytes into ordered text lines
works here; the accuracy logic lives in parse.py and only consumes lines.
"""
from __future__ import annotations

import io
import logging
import threading
from dataclasses import dataclass

logger = logging.getLogger(__name__)

try:  # optional dependency - guarded so a missing wheel/libGL doesn't break boot
    from rapidocr import RapidOCR
    _IMPORT_ERROR: Exception | None = None
except Exception as exc:  # noqa: BLE001 - ImportError or libGL OSError both land here
    RapidOCR = None  # type: ignore[assignment,misc]
    _IMPORT_ERROR = exc

_engine = None
_engine_lock = threading.Lock()


class OcrUnavailable(RuntimeError):
    """The OCR engine (or a system lib it needs) isn't installed in this image."""


def available() -> bool:
    return RapidOCR is not None


def _get_engine():
    global _engine
    if RapidOCR is None:
        raise OcrUnavailable(f"OCR engine unavailable: {_IMPORT_ERROR}")
    if _engine is None:
        with _engine_lock:                # first call loads the ONNX models once
            if _engine is None:
                _engine = RapidOCR()
    return _engine


@dataclass
class OcrElement:
    """One recognized text box. ``box`` is 4 (x, y) corner points; empty if the
    engine returned no geometry."""
    text: str
    box: tuple = ()


def _decode(data: bytes):
    import numpy as np
    from PIL import Image, UnidentifiedImageError
    try:
        img = Image.open(io.BytesIO(data)).convert("RGB")
    except (UnidentifiedImageError, OSError) as exc:
        raise ValueError(f"unreadable image: {exc}") from exc
    return np.asarray(img)


def _run(arr) -> list[OcrElement]:
    """OCR a decoded RGB array into ordered (text, box) elements (top-to-bottom,
    left-to-right)."""
    res = _get_engine()(arr)
    txts = getattr(res, "txts", None) if res is not None else None
    if not txts:
        return []
    boxes = getattr(res, "boxes", None)
    if boxes is None:
        return [OcrElement(t) for t in txts if t and t.strip()]
    elems = [OcrElement(t, tuple((float(p[0]), float(p[1])) for p in b))
             for b, t in zip(boxes, txts, strict=False) if t and t.strip()]
    elems.sort(key=lambda e: (min(p[1] for p in e.box), min(p[0] for p in e.box)))
    return elems


def recognize(data: bytes) -> tuple[object, list[OcrElement]]:
    """OCR image bytes -> ``(decoded_array, elements)``. The array is returned so
    callers can re-OCR sub-regions without decoding again (see ocr_row)."""
    arr = _decode(data)
    return arr, _run(arr)


def image_to_lines(data: bytes) -> list[str]:
    """OCR ``data`` (raw image bytes) into text lines ordered top-to-bottom,
    left-to-right - the input parse.extract_character_stats() expects.

    BLOCKING + CPU-bound; call from a worker thread (see service.py). Raises
    ``OcrUnavailable`` if the engine isn't installed, ``ValueError`` on an
    unreadable image."""
    return [e.text for e in _run(_decode(data))]


def ocr_row(arr, box, *, scale: int = 4, right_frac: float = 0.30) -> list[str]:
    """Re-OCR a tight horizontal strip around ``box``'s row, upscaled, and return
    the texts. Used to recover a value the full-image detector missed next to a
    label it DID find (RapidOCR occasionally drops a lone glyph like a '0'). The
    strip spans the label's y-band and extends right (where the value sits)."""
    import numpy as np
    from PIL import Image

    h, w = arr.shape[:2]
    xs = [p[0] for p in box]
    ys = [p[1] for p in box]
    y0 = max(0, int(min(ys)) - 2)
    y1 = min(h, int(max(ys)) + 2)
    x0 = max(0, int(min(xs) - 0.05 * w))
    x1 = min(w, int(max(xs) + right_frac * w))
    strip = arr[y0:y1, x0:x1]
    if strip.size == 0:
        return []
    up = Image.fromarray(strip).resize(
        (max(1, (x1 - x0) * scale), max(1, (y1 - y0) * scale)), Image.LANCZOS)
    return [e.text for e in _run(np.asarray(up))]
