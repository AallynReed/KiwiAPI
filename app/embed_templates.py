"""Shared embed templating.

A user-editable **template** + a flat **variable context** -> a Discord embed
(`{embeds:[...], content}`). Used by outbound webhooks (`app/webhooks`) and the
bot's per-guild announcements (`app/bot`), so both menus offer the same
"customize the embed" capability with one renderer.

Design:
- A *type* (a webhook event, or a bot announcement type) exposes a **context**: a
  flat ``{name: value}`` dict of variables, including pre-formatted convenience
  vars so conditional formatting stays in code (e.g. ``when_line``).
- A **template** holds the labels/structure with ``{var}`` placeholders. The
  *default* template is built per-render (so the bot can localize it via ``t()``);
  a user's *custom* template is static text in whatever language they wrote.
- ``render_template(template, context)`` substitutes and returns a Discord embed,
  clamped to Discord's documented field limits.

Placeholders: ``{name}`` (string substitution) and ``{unix_var:R}`` for a unix
timestamp variable -> a Discord ``<t:unix:R>`` tag (styles t T d D f F R). Unknown
variables render empty.
"""

from __future__ import annotations

import re

from pydantic import BaseModel, Field

# Discord's documented per-field maximums (we clamp to stay valid).
LIMITS = {
    "title": 256, "description": 4096, "field_name": 256, "field_value": 1024,
    "footer": 2048, "content": 2000,
}
MAX_FIELDS = 25

# When a template references a saved Image Studio design, the delivery layer renders
# that design fresh and uploads it as an attachment under this filename (the embed's
# image points at attachment://<this>). Uploading per-post sidesteps Discord's
# aggressive URL-image caching, so live data in the image is always current.
EMBED_IMAGE_ATTACHMENT = "embed-image.png"

_VAR_RE = re.compile(r"\{([a-zA-Z0-9_]+)(?::([a-zA-Z]))?\}")
_TS_STYLES = set("tTdDfFR")


class EmbedField(BaseModel):
    name: str = ""
    value: str = ""
    inline: bool = True


class EmbedTemplate(BaseModel):
    """A customizable embed. ``enabled=False`` means "use the type's default";
    callers only render this when it's enabled."""

    enabled: bool = False
    content: str | None = None       # plain text above the embed (mentions go here)
    title: str | None = None
    url: str | None = None           # makes the title a clickable link
    description: str | None = None
    color: str | None = None         # "#RRGGBB", "RRGGBB", or a decimal string
    fields: list[EmbedField] = Field(default_factory=list)
    footer: str | None = None
    show_image: bool = True          # include an image (default banner, or the design below)
    image_design_id: str | None = None  # an Image Studio design - rendered + uploaded per post
    image_url: str | None = None     # advanced: a static image URL (set via raw JSON)


def substitute(text: str | None, ctx: dict) -> str:
    """Replace ``{var}`` / ``{unix:R}`` placeholders from ``ctx``. Unknown vars -> ''."""
    if not text:
        return ""

    def _repl(m: re.Match) -> str:
        key, style = m.group(1), m.group(2)
        if key not in ctx:
            return ""
        val = ctx[key]
        if style and style in _TS_STYLES:
            try:
                return f"<t:{int(val)}:{style}>"
            except (TypeError, ValueError):
                return "" if val is None else str(val)
        return "" if val is None else str(val)

    return _VAR_RE.sub(_repl, text)


def parse_color(c) -> int | None:
    """Accept '#RRGGBB', 'RRGGBB', or a decimal string/int -> int, else None."""
    if c is None or c == "":
        return None
    if isinstance(c, int):
        return c
    s = str(c).strip().lstrip("#")
    try:
        if re.fullmatch(r"[0-9a-fA-F]{6}", s):
            return int(s, 16)
        return int(s)
    except ValueError:
        return None


def render_template(
    tmpl: EmbedTemplate, ctx: dict, *, default_image_url: str | None = None,
) -> tuple[dict, str | None]:
    """Render ``(embed_dict, content)`` from a template + context.

    ``embed_dict`` is a Discord embed object (drop it into ``{"embeds": [embed]}``);
    ``content`` is the optional message text (or None)."""
    embed: dict = {}

    title = substitute(tmpl.title, ctx).strip()
    if title:
        embed["title"] = title[: LIMITS["title"]]
    url = substitute(tmpl.url, ctx).strip()
    if url.startswith("http"):
        embed["url"] = url
    desc = substitute(tmpl.description, ctx).strip()
    if desc:
        embed["description"] = desc[: LIMITS["description"]]
    color = parse_color(tmpl.color)
    if color is not None:
        embed["color"] = color

    fields = []
    for f in tmpl.fields[:MAX_FIELDS]:
        n = substitute(f.name, ctx).strip()[: LIMITS["field_name"]]
        v = substitute(f.value, ctx).strip()[: LIMITS["field_value"]]
        if n or v:
            # Discord rejects an empty name or value; use a zero-width space.
            fields.append({"name": n or "​", "value": v or "​",
                           "inline": bool(f.inline)})
    if fields:
        embed["fields"] = fields

    footer = substitute(tmpl.footer, ctx).strip()
    if footer:
        embed["footer"] = {"text": footer[: LIMITS["footer"]]}

    if tmpl.show_image:
        if tmpl.image_design_id:
            # Delivery renders the design + uploads it under this name (see
            # EMBED_IMAGE_ATTACHMENT); referencing the attachment avoids URL caching.
            embed["image"] = {"url": f"attachment://{EMBED_IMAGE_ATTACHMENT}"}
        else:
            img = (tmpl.image_url or "").strip() or default_image_url
            if img:
                embed["image"] = {"url": img}

    content = substitute(tmpl.content, ctx).strip() or None
    if content:
        content = content[: LIMITS["content"]]
    return embed, content


def template_to_dict(tmpl: EmbedTemplate) -> dict:
    """The template as a plain dict for the editor's "raw JSON" view + API responses."""
    return tmpl.model_dump()
