"""Report which translatable site strings are missing from each locale.

English IS the key: a string is marked in a template with `data-i18n` (or one of
the attribute markers) and in JS by passing it to `t()`, and the locale JSONs map
the normalised English straight to its translation. So the set of keys is whatever
those two sources mention, and the gap is set-difference per locale.

    python scripts/i18n_audit.py            # counts per locale
    python scripts/i18n_audit.py --missing de     # the untranslated strings
    python scripts/i18n_audit.py --stale          # keys no source mentions
"""
from __future__ import annotations

import argparse
import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
SITE = ROOT / "site"
TEMPLATES = SITE / "templates"
STATIC = SITE / "static"
LOCALES = STATIC / "locales"


def norm(s: str) -> str:
    """The same normalisation i18n.js applies before a lookup."""
    return re.sub(r"\s+", " ", s).strip()


# The OPENING tag only. The inner HTML is then walked out by hand, because
# markers DO nest - `<li data-i18n><a data-i18n>…</a></li>` marks both the row and
# the link inside it, and one regex with a non-greedy body swallows the outer
# element whole and never sees the inner one. That silently dropped 485 real keys
# and reported the locales as complete while they were not.
_OPEN = re.compile(
    r"<(?P<tag>[a-zA-Z][\w-]*)\b(?P<attrs>[^>]*\bdata-i18n\b(?![-\w])[^>]*?)(?P<selfclose>/?)>"
)
# Tags that never carry a closing partner, so they never open a depth level.
_VOID = {"area", "base", "br", "col", "embed", "hr", "img", "input", "link",
         "meta", "param", "source", "track", "wbr"}


def _inner_html(text: str, tag: str, body_at: int) -> str | None:
    """The content of the element whose body starts at ``body_at``.

    Counts opens and closes of the SAME tag name, so an element nested inside one
    of its own kind ends at the right place rather than at the first close."""
    if tag.lower() in _VOID:
        return ""
    depth = 1
    pos = body_at
    pattern = re.compile(rf"<(/?){re.escape(tag)}\b[^>]*?(/?)>", re.I | re.S)
    while depth:
        m = pattern.search(text, pos)
        if not m:
            return None                    # unbalanced - report nothing, guess nothing
        if m.group(1):
            depth -= 1
            if not depth:
                return text[body_at:m.start()]
        elif not m.group(2):
            depth += 1
        pos = m.end()
    return None
_ATTR = re.compile(
    r"""data-i18n-(?:placeholder|title|aria-label)\s*=\s*["']([^"']+)["']"""
)
# The JS side. Every page wraps `window.BTTi18n.t` in a local one-liner, and the
# local is NOT always called `t` - `classes.js` and `status.js` call theirs `tr`,
# and scanning for `t(` alone reported 468 live strings as dead. Match the wrappers
# this codebase actually uses, and `BTTi18n.t(...)` for anything calling it direct.
# Only literals; a t(variable) has no key to collect and is reported separately so
# it cannot hide.
_T_NAMES = r"(?:t|tr|_t|i18n|translate|BTTi18n\.t)"
_T_CALL = re.compile(rf"""(?<![\w.]){_T_NAMES}\(\s*(["'])((?:\\.|(?!\1).)*)\1""")
_T_DYNAMIC = re.compile(rf"(?<![\w.]){_T_NAMES}\(\s*[^\"'\s)]")


def _unescape_js(s: str) -> str:
    return s.replace("\\'", "'").replace('\\"', '"').replace("\\\\", "\\")


def strip_js_comments(src: str) -> str:
    """Blank out comments, keeping length and line structure.

    Comments discuss the code, and this codebase's discuss it in its own terms - one
    of them names ``t('results in')`` to explain why that fragment was REPLACED by a
    single placeholder key. Scanned naively, the explanation of a deleted string
    reads as a live use of it, and the string gets translated into eight locales
    nobody will ever look up. A char scan rather than a regex, because a `//` inside
    a string literal or a regex is not a comment.

    REGEX LITERALS have to be recognised or this eats live code: ``/^\\/modpacks\\//``
    ends with an escaped slash followed by its own terminator, which reads exactly
    like the start of a line comment - blanking from there ran to the end of the file
    and took 44 real strings with it. Whether a `/` opens a regex or divides is
    decided by what precedes it, the heuristic every syntax highlighter uses: after a
    value (identifier, number, closing bracket) it divides; after an operator, an
    opening bracket, or nothing, it opens a regex.
    """
    out = list(src)
    i, n = 0, len(src)
    quote = None          # the string delimiter we are inside, or None
    prev = ""             # last significant character seen outside a comment

    def regex_can_start() -> bool:
        if not prev:
            return True
        if prev in ")]}":
            return False
        if prev.isalnum() or prev in "_$":
            head = src[:i].rstrip()
            return bool(_REGEX_KEYWORD.search(head))
        return True

    while i < n:
        c = src[i]
        if quote:
            if c == "\\":
                i += 2
                continue
            if c == quote:
                quote = None
            i += 1
            continue
        if c in "\"'`":
            quote = c
            prev = c
            i += 1
            continue
        if c == "/" and i + 1 < n and src[i + 1] not in "/*" and regex_can_start():
            # Skip the literal whole. A `/` inside a [...] class does not end it, and
            # a newline means this was never a regex - bail rather than run on.
            i += 1
            in_class = False
            while i < n:
                d = src[i]
                if d == "\\":
                    i += 2
                    continue
                if d == "[":
                    in_class = True
                elif d == "]":
                    in_class = False
                elif d == "/" and not in_class:
                    i += 1
                    break
                elif d == "\n":
                    break
                i += 1
            prev = "/"
            continue
        if c == "/" and i + 1 < n:
            nxt = src[i + 1]
            if nxt == "/":
                while i < n and src[i] != "\n":
                    out[i] = " "
                    i += 1
                continue
            if nxt == "*":
                while i < n and not (src[i] == "*" and i + 1 < n and src[i + 1] == "/"):
                    if src[i] != "\n":
                        out[i] = " "
                    i += 1
                # An unterminated block runs to end of file; stop at the end rather
                # than reading past it.
                for k in range(i, min(i + 2, n)):
                    out[k] = " "
                i += 2
                continue
        if not c.isspace():
            prev = c
        i += 1
    return "".join(out)


_REGEX_KEYWORD = re.compile(r"\b(return|typeof|case|in|of|delete|void|do|else|yield)$")


_HTML_COMMENT = re.compile(r"<!--.*?-->", re.S)


def keys_from_templates() -> dict[str, set[str]]:
    found: dict[str, set[str]] = {}
    for path in sorted(TEMPLATES.rglob("*.html")):
        text = _HTML_COMMENT.sub("", path.read_text(encoding="utf-8"))
        keys = set()
        for m in _OPEN.finditer(text):
            if m.group("selfclose"):
                continue
            inner = _inner_html(text, m.group("tag"), m.end())
            if inner is not None:
                keys.add(norm(inner))
        keys |= {norm(m.group(1)) for m in _ATTR.finditer(text)}
        keys = {k for k in keys if k and not k.startswith("{")}
        if keys:
            found[str(path.relative_to(ROOT))] = keys
    return found


def keys_from_js() -> tuple[dict[str, set[str]], dict[str, int]]:
    found: dict[str, set[str]] = {}
    dynamic: dict[str, int] = {}
    for path in sorted(STATIC.rglob("*.js")):
        if ".min." in path.name or "vendor" in path.parts:
            continue
        text = strip_js_comments(path.read_text(encoding="utf-8"))
        keys = {norm(_unescape_js(m.group(2))) for m in _T_CALL.finditer(text)}
        keys = {k for k in keys if k}
        if keys:
            found[str(path.relative_to(ROOT))] = keys
        n = len(_T_DYNAMIC.findall(text))
        if n:
            dynamic[str(path.relative_to(ROOT))] = n
    return found, dynamic


def all_keys() -> tuple[set[str], dict[str, set[str]], dict[str, int]]:
    per_file = {}
    per_file.update(keys_from_templates())
    js, dynamic = keys_from_js()
    per_file.update(js)
    every: set[str] = set()
    for keys in per_file.values():
        every |= keys
    return every, per_file, dynamic


def locales() -> dict[str, dict[str, str]]:
    return {
        p.stem: json.loads(p.read_text(encoding="utf-8"))
        for p in sorted(LOCALES.glob("*.json"))
    }


def main() -> None:
    ap = argparse.ArgumentParser()
    ap.add_argument("--missing", metavar="LOCALE", help="list what this locale lacks")
    ap.add_argument("--by-file", action="store_true", help="group missing by source file")
    ap.add_argument("--stale", action="store_true", help="keys no source mentions")
    ap.add_argument("--json", action="store_true", help="machine-readable output")
    args = ap.parse_args()

    every, per_file, dynamic = all_keys()
    locs = locales()

    if args.missing:
        have = set(locs[args.missing])
        missing = sorted(every - have)
        if args.by_file:
            for path, keys in sorted(per_file.items()):
                gap = sorted(set(keys) - have)
                if gap:
                    print(f"\n## {path}  ({len(gap)})")
                    for k in gap:
                        print(f"  {k}")
            return
        if args.json:
            print(json.dumps(missing, ensure_ascii=False, indent=1))
            return
        for k in missing:
            print(k)
        return

    if args.stale:
        for name, d in locs.items():
            extra = sorted(set(d) - every)
            print(f"{name}: {len(extra)} key(s) no source mentions")
            for k in extra[:40]:
                print(f"  {k}")
        return

    print(f"{len(every)} translatable strings across {len(per_file)} files\n")
    for name, d in sorted(locs.items()):
        have = len(every & set(d))
        pct = 100 * have / len(every) if every else 0
        print(f"  {name:>6}  {have:>5} / {len(every)}  ({pct:5.1f}%)"
              f"   missing {len(every - set(d))}")
    if dynamic:
        total = sum(dynamic.values())
        print(f"\n{total} t() call(s) with a non-literal argument "
              f"(no key to collect) in {len(dynamic)} file(s):")
        for path, n in sorted(dynamic.items(), key=lambda kv: -kv[1]):
            print(f"  {n:>4}  {path}")


if __name__ == "__main__":
    main()
