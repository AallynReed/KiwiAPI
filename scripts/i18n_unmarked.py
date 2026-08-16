"""Find user-visible text in a template that nothing has marked for translation.

The audit answers "are the marked strings translated?". This answers the other
half: "is everything that should be marked, marked?" - a page can sit at 100%
coverage and still render half in English because the strings were never wired.

    python scripts/i18n_unmarked.py site/templates/mods.html
    python scripts/i18n_unmarked.py --all
"""
from __future__ import annotations

import argparse
import re
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import i18n_audit as audit  # noqa: E402

# Text inside these never reaches the reader as prose.
_SKIP_TAGS = {"script", "style", "code", "kbd", "svg", "path", "option"}
# A Jinja expression, an entity on its own, punctuation, a number, an icon glyph:
# none of these are a sentence somebody reads.
_NOT_PROSE = re.compile(r"^(?:[\W\d_]+|&[a-z]+;|\{\{.*\}\}|\{%.*%\})$", re.S)


def _blocked_spans(text: str) -> list[tuple[int, int]]:
    """Ranges already covered by a marker, a comment, or a non-prose tag."""
    spans = []
    for m in audit._HTML_COMMENT.finditer(text):
        spans.append((m.start(), m.end()))
    for tag in _SKIP_TAGS:
        for m in re.finditer(rf"<{tag}\b.*?</{tag}>", text, re.S | re.I):
            spans.append((m.start(), m.end()))
    for m in audit._OPEN.finditer(text):
        if m.group("selfclose"):
            continue
        inner = audit._inner_html(text, m.group("tag"), m.end())
        if inner is not None:
            spans.append((m.start(), m.end() + len(inner)))
    return spans


def unmarked(path: Path) -> list[str]:
    text = path.read_text(encoding="utf-8")
    blocked = _blocked_spans(text)

    def covered(i: int) -> bool:
        return any(a <= i < b for a, b in blocked)

    out = []
    # Text between tags. Attributes are handled by the audit's own attribute markers.
    for m in re.finditer(r">([^<>]+)<", text):
        chunk = m.group(1)
        body = chunk.strip()
        if not body or _NOT_PROSE.match(body):
            continue
        if covered(m.start(1)):
            continue
        if not re.search(r"[A-Za-z]{2}", body):
            continue
        out.append(audit.norm(body))
    return out


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("paths", nargs="*", type=Path)
    ap.add_argument("--all", action="store_true", help="every site template")
    args = ap.parse_args()

    paths = sorted(audit.TEMPLATES.rglob("*.html")) if args.all else args.paths
    total = 0
    for path in paths:
        found = unmarked(path)
        if not found:
            continue
        total += len(found)
        print(f"\n### {path}  ({len(found)})")
        for s in found:
            print(f"  {s[:120]}")
    print(f"\n{total} unmarked string(s)")


if __name__ == "__main__":
    main()
