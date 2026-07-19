"""Server-side render model for /commands.

The command reference is served pre-rendered so the page is complete without JS
(crawlers + no-JS visitors) and paints instantly; ``commands.js`` then hydrates -
wiring search + scroll-spy onto the existing DOM and re-rendering only when the
user switches language. The canonical server render is English - the crawlable
default and ``t()``'s fallback; non-English visitors get a client re-render.

This mirrors ``commands.js`` renderIntro/renderChips/renderList exactly (same
classes, ids, ``data-search``) so the pre-rendered DOM is byte-for-byte what the
JS would have built, and the filter/observer keep working untouched.
"""
import re
from typing import Any

from app.site.static_data import load_static_json

_COMMANDS_JSON = "commands.json"
SSR_LANG = "en"
# Matches a "<placeholder>" token (no nested angle brackets) - same test as
# commands.js renderRow, gating the yellow usage-caution block.
_PLACEHOLDER_RE = re.compile(r"<[^<>]+>")


def _t(field: dict | None, lang: str = SSR_LANG) -> str:
    """Resolve a ``{lang: text}`` field, falling back to English then empty -
    identical to commands.js ``t()``."""
    if not field:
        return ""
    return field.get(lang) or field.get("en") or ""


def commands_view(lang: str = SSR_LANG) -> dict[str, Any]:
    """Build the language-resolved view model the template loops over."""
    data = load_static_json(_COMMANDS_JSON)
    intro_raw = data.get("intro", {})
    rules_raw = intro_raw.get("rules") or {}
    intro = {
        "title": _t(intro_raw.get("title"), lang),
        "subtitle": _t(intro_raw.get("subtitle"), lang),
        "search_placeholder": _t(intro_raw.get("search_placeholder"), lang),
        "source_note": _t(intro_raw.get("source_note"), lang),
        "placeholder_warning": _t(intro_raw.get("placeholder_warning"), lang),
        "rules": rules_raw.get(lang) or rules_raw.get("en") or [],
    }

    commands = data.get("commands", [])
    categories = data.get("categories", [])

    # Chips list every category with its command count (matches renderChips,
    # which iterates all categories regardless of whether any commands land in
    # them). Sections below only include non-empty categories (matches
    # renderList's ``if (!items.length) continue``).
    counts: dict[str, int] = {}
    for cmd in commands:
        counts[cmd["category"]] = counts.get(cmd["category"], 0) + 1
    chips = [
        {"key": cat["key"], "name": _t(cat.get("name"), lang), "count": counts.get(cat["key"], 0)}
        for cat in categories
    ]

    sections = []
    total = 0
    for cat in categories:
        items = [c for c in commands if c["category"] == cat["key"]]
        if not items:
            continue
        total += len(items)
        rows = []
        for cmd in items:
            aliases = cmd.get("aliases") or []
            desc = _t(cmd.get("description"), lang)
            note = _t(cmd.get("note"), lang) if cmd.get("note") else ""
            # Precomputed lowercased haystack the JS filter reads from
            # ``row.dataset.search`` - same concatenation order as renderRow.
            search = f"{cmd['syntax']} {' '.join(aliases)} {desc} {note}".lower()
            rows.append({
                "syntax": cmd["syntax"],
                "aliases": aliases,
                "desc": desc,
                "note": note,
                "has_placeholder": bool(_PLACEHOLDER_RE.search(cmd["syntax"])),
                "search": search,
            })
        sections.append({
            "key": cat["key"],
            "name": _t(cat.get("name"), lang),
            "count": len(items),
            "rows": rows,
        })

    return {"intro": intro, "chips": chips, "sections": sections,
            "total": total, "ssr_lang": lang}
