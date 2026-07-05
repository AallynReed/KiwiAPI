"""Render an ``ImageDesign`` to a PNG (Pillow).

Unlike the embed renderer, timestamp styles render as human-readable text
(``{ends_at:R}`` → "in 12 minutes") because an image can't hold a live Discord
``<t:>`` tag - it's baked at render time (the render URL is cache-busted by the data
signature, mirroring the existing announce.png banners).
"""

from __future__ import annotations

import re
import time
from datetime import datetime, timezone
from io import BytesIO

from PIL import Image, ImageDraw

from app.images.models import MAX_DIM, MAX_LAYERS, MIN_DIM, ImageDesign, Layer
from app.site.og_image import _font

_VAR_RE = re.compile(r"\{([a-zA-Z0-9_]+)(?::([a-zA-Z]))?\}")
_TS_STYLES = set("tTdDfFR")


# ── variable substitution (image-friendly) ──────────────────────────────────

def _human_time(unix: int, style: str, now: int) -> str:
    d = datetime.fromtimestamp(unix, tz=timezone.utc)
    if style == "R":
        diff = unix - now
        a = abs(diff)
        if a < 60:
            n, unit = a, "second"
        elif a < 3600:
            n, unit = round(a / 60), "minute"
        elif a < 86400:
            n, unit = round(a / 3600), "hour"
        else:
            n, unit = round(a / 86400), "day"
        s = "" if n == 1 else "s"
        return f"in {n} {unit}{s}" if diff >= 0 else f"{n} {unit}{s} ago"
    fmt = {"t": "%H:%M", "T": "%H:%M:%S", "d": "%Y-%m-%d", "D": "%d %B %Y",
           "f": "%d %B %Y %H:%M", "F": "%A, %d %B %Y %H:%M"}.get(style, "%Y-%m-%d %H:%M")
    return d.strftime(fmt)


def substitute(text: str | None, ctx: dict, now: int) -> str:
    if not text:
        return ""

    def _repl(m: re.Match) -> str:
        key, style = m.group(1), m.group(2)
        if key not in ctx:
            return ""
        val = ctx[key]
        if style and style in _TS_STYLES:
            try:
                return _human_time(int(val), style, now)
            except (TypeError, ValueError):
                return "" if val is None else str(val)
        return "" if val is None else str(val)

    return _VAR_RE.sub(_repl, text)


_MD_LINK = re.compile(r"\[([^\]]+)\]\((?:https?:)?[^)]+\)")


def _strip_md(s: str) -> str:
    """Drop Discord markup an image can't render (bold/italic/code, link syntax) so a
    composite variable like ``current_line`` reads cleanly as plain text."""
    s = _MD_LINK.sub(r"\1", s)
    return s.replace("**", "").replace("`", "").replace("*", "")


# ── colour helpers ───────────────────────────────────────────────────────────

def _rgba(c: str | None, opacity: float = 1.0) -> tuple[int, int, int, int]:
    s = (str(c or "")).strip().lstrip("#")
    try:
        if len(s) == 3:
            s = "".join(ch * 2 for ch in s)
        r, g, b = int(s[0:2], 16), int(s[2:4], 16), int(s[4:6], 16)
    except (ValueError, IndexError):
        r, g, b = 255, 255, 255
    return (r, g, b, max(0, min(255, int(opacity * 255))))


# ── background ───────────────────────────────────────────────────────────────

def _gradient(w: int, h: int, c1: str, c2: str, angle: int) -> Image.Image:
    """A linear gradient ``c1``→``c2`` at ``angle`` degrees (90 = top→bottom)."""
    diag = int((w * w + h * h) ** 0.5) + 2
    mask = Image.linear_gradient("L").resize((diag, diag))        # 0 top → 255 bottom
    mask = mask.rotate(angle - 90, resample=Image.BILINEAR, expand=False)
    left, top = (diag - w) // 2, (diag - h) // 2
    mask = mask.crop((left, top, left + w, top + h))
    a = Image.new("RGBA", (w, h), _rgba(c1))
    b = Image.new("RGBA", (w, h), _rgba(c2))
    return Image.composite(b, a, mask)


def _fit_image(src: Image.Image, w: int, h: int, fit: str) -> Image.Image:
    if fit == "stretch":
        return src.resize((w, h))
    sw, sh = src.size
    scale = (max(w / sw, h / sh) if fit == "cover" else min(w / sw, h / sh))
    rw, rh = max(1, int(sw * scale)), max(1, int(sh * scale))
    src = src.resize((rw, rh))
    canvas = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    canvas.paste(src, ((w - rw) // 2, (h - rh) // 2))
    return canvas


def _draw_background(img: Image.Image, bg, blob_get) -> None:
    w, h = img.size
    if bg.type == "gradient":
        img.alpha_composite(_gradient(w, h, bg.color1, bg.color2, bg.angle))
    elif bg.type == "image" and bg.image_sha:
        data = blob_get(bg.image_sha)
        if data:
            try:
                src = Image.open(BytesIO(data)).convert("RGBA")
                img.alpha_composite(_fit_image(src, w, h, bg.fit))
                return
            except Exception:
                pass
        img.alpha_composite(Image.new("RGBA", (w, h), _rgba(bg.color1)))
    else:
        img.alpha_composite(Image.new("RGBA", (w, h), _rgba(bg.color1)))


# ── layers ───────────────────────────────────────────────────────────────────

def _wrap(draw, text: str, font, max_w: int) -> list[str]:
    lines: list[str] = []
    for para in text.split("\n"):
        words = para.split(" ")
        cur = ""
        for word in words:
            trial = (cur + " " + word).strip()
            if cur and draw.textbbox((0, 0), trial, font=font)[2] > max_w:
                lines.append(cur)
                cur = word
            else:
                cur = trial
        lines.append(cur)
    return lines


def _draw_text(img: Image.Image, draw, layer: Layer, ctx: dict, now: int) -> None:
    text = _strip_md(substitute(layer.text, ctx, now))
    if not text:
        return
    font = _font(max(6, min(300, layer.font_size)), layer.bold)
    fill = _rgba(layer.color, layer.opacity)
    lines = _wrap(draw, text, font, layer.max_width) if layer.max_width else text.split("\n")
    line_h = (draw.textbbox((0, 0), "Ag", font=font)[3]) + 4
    for i, line in enumerate(lines):
        y = layer.y + i * line_h
        w = draw.textbbox((0, 0), line, font=font)[2]
        x = layer.x
        if layer.align == "center":
            x = layer.x - w / 2
        elif layer.align == "right":
            x = layer.x - w
        draw.text((x, y), line, font=font, fill=fill)


def _draw_rect(img: Image.Image, draw, layer: Layer) -> None:
    x0, y0 = layer.x, layer.y
    x1, y1 = layer.x + layer.w, layer.y + layer.h
    fill = _rgba(layer.color, layer.opacity)
    if layer.radius > 0:
        draw.rounded_rectangle([x0, y0, x1, y1], radius=min(layer.radius, int(min(layer.w, layer.h) / 2)), fill=fill)
    else:
        draw.rectangle([x0, y0, x1, y1], fill=fill)


def _draw_image_layer(img: Image.Image, layer: Layer, blob_get) -> None:
    if not layer.image_sha:
        return
    data = blob_get(layer.image_sha)
    if not data:
        return
    try:
        src = Image.open(BytesIO(data)).convert("RGBA")
    except Exception:
        return
    w, h = max(1, int(layer.w)), max(1, int(layer.h))
    src = src.resize((w, h))
    if layer.opacity < 1.0:
        alpha = src.getchannel("A").point(lambda a: int(a * layer.opacity))
        src.putalpha(alpha)
    if layer.radius > 0:
        mask = Image.new("L", (w, h), 0)
        ImageDraw.Draw(mask).rounded_rectangle([0, 0, w - 1, h - 1],
                                               radius=min(layer.radius, int(min(w, h) / 2)), fill=255)
        if layer.opacity < 1.0:
            mask = mask.point(lambda a: int(a * layer.opacity))
        img.paste(src, (int(layer.x), int(layer.y)), mask)
    else:
        img.alpha_composite(src, (int(layer.x), int(layer.y)))


# ── entry point ──────────────────────────────────────────────────────────────

def render(design: ImageDesign, ctx: dict, blob_get, *, now: int | None = None) -> bytes:
    """Render ``design`` to PNG bytes. ``ctx`` supplies template variables;
    ``blob_get(sha) -> bytes | None`` fetches uploaded images from the blob store."""
    now = now if now is not None else int(time.time())
    w = max(MIN_DIM, min(MAX_DIM, int(design.width)))
    h = max(MIN_DIM, min(MAX_DIM, int(design.height)))
    img = Image.new("RGBA", (w, h), (0, 0, 0, 0))
    _draw_background(img, design.background, blob_get)
    draw = ImageDraw.Draw(img, "RGBA")
    for layer in (design.layers or [])[:MAX_LAYERS]:
        try:
            if layer.type == "text":
                _draw_text(img, draw, layer, ctx, now)
            elif layer.type == "rect":
                _draw_rect(img, draw, layer)
            elif layer.type == "image":
                _draw_image_layer(img, layer, blob_get)
        except Exception:
            continue                          # one bad layer never breaks the render
    out = BytesIO()
    img.save(out, "PNG")
    return out.getvalue()
