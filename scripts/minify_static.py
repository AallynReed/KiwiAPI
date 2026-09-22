"""Generate minified copies of every static CSS + JS asset the showcase
site ships. Source files keep their original names (so editing is easy);
the minified output lives alongside as `*.min.css` / `*.min.js`, which is
what the templates reference (served `no-cache`, so a redeploy refreshes
browsers).

DYNAMIC: every top-level `site/static/*.css` and `*.js` source file is
discovered and minified automatically - add a new page asset and it's
picked up on the next run, no list to maintain. Already-minified outputs
(`*.min.css` / `*.min.js`) are skipped as inputs. Generating a `.min` for a
file a template doesn't reference is harmless (just an unused sibling).

Tools:
  - csscompressor for CSS (handles `color-mix(...)`, `@keyframes`,
    `calc()`, modern syntax. Safer than naive regex strip-and-collapse.)
  - rjsmin for JS (Eric Sasse's port of Douglas Crockford's JSMin,
    handles string/regex literals + automatic semicolon insertion. Pure
    Python, no Node required.)

Re-runnable: existing `.min.*` files are overwritten in-place.

Run:  .venv/Scripts/python scripts/minify_static.py
"""
import re
from pathlib import Path

import csscompressor
import rjsmin

ROOT = Path(__file__).resolve().parent.parent
STATIC = ROOT / "site" / "static"

# rjsmin predates ES6 template literals: it lexes their text as code and strips
# whitespace it thinks is insignificant, so `</i> ${name}` shipped as
# `</i>${name}` and `class="a" title="b"` as `class="a"title="b"` in 17 files.
# So every template literal is swapped for a placeholder string before rjsmin
# runs and restored verbatim after - `${}` code inside a template stays
# unminified, which costs a little size and nothing else.
_REGEX_AFTER = set("(,=:[!&|?{};+-*%<>~^") | {"", "return", "typeof", "case", "do", "else",
                                             "in", "of", "new", "delete", "void", "throw",
                                             "yield", "await"}


def _skip_string(src: str, i: int) -> int:
    quote, i = src[i], i + 1
    while src[i] != quote:
        i += 2 if src[i] == "\\" else 1
    return i + 1


def _skip_regex(src: str, i: int) -> int:
    i, in_class = i + 1, False
    while True:
        c = src[i]
        if c == "\\":
            i += 2
            continue
        if c == "[":
            in_class = True
        elif c == "]":
            in_class = False
        elif c == "/" and not in_class:
            break
        i += 1
    i += 1
    while i < len(src) and src[i].isalpha():
        i += 1
    return i


def _skip_template(src: str, i: int) -> int:
    """`i` is just past an opening backtick; returns the index past its close."""
    while True:
        c = src[i]
        if c == "\\":
            i += 2
        elif c == "`":
            return i + 1
        elif src.startswith("${", i):
            i = _scan_code(src, i + 2, None)
        else:
            i += 1


def _scan_code(src: str, i: int, spans: list[tuple[int, int]] | None) -> int:
    """Walk JS code from `i`. With `spans` (top level) record every template
    literal and run to the end; without (inside `${}`) stop past the closing
    brace and return that index."""
    n, depth, prev = len(src), 0, ""
    while i < n:
        c = src[i]
        if c.isspace():
            i += 1
        elif src.startswith("//", i):
            j = src.find("\n", i)
            i = n if j < 0 else j
        elif src.startswith("/*", i):
            i = src.index("*/", i + 2) + 2
        elif c in "'\"":
            i, prev = _skip_string(src, i), "str"
        elif c == "`":
            j = _skip_template(src, i + 1)
            if spans is not None:
                spans.append((i, j))
            i, prev = j, "str"
        elif c == "/" and prev in _REGEX_AFTER:
            i, prev = _skip_regex(src, i), "re"
        elif c == "{":
            depth, i, prev = depth + 1, i + 1, c
        elif c == "}":
            if spans is None and depth == 0:
                return i + 1
            depth, i, prev = depth - 1, i + 1, c
        elif c.isalnum() or c in "_$":
            j = i
            while j < n and (src[j].isalnum() or src[j] in "_$"):
                j += 1
            prev, i = (src[i:j] if src[i:j] in _REGEX_AFTER else "id"), j
        else:
            i, prev = i + 1, c
    if spans is None:
        raise ValueError("unterminated ${ } in a template literal")
    return i


def minify_js(src: str) -> str:
    spans: list[tuple[int, int]] = []
    _scan_code(src, 0, spans)
    tag = "__btt_tpl_%d__"
    if "__btt_tpl_" in src:
        raise ValueError("placeholder collides with the source")
    parts, last = [], 0
    for n, (a, b) in enumerate(spans):
        parts.append(src[last:a] + f'"{tag % n}"')
        last = b
    mini = rjsmin.jsmin("".join(parts) + src[last:], keep_bang_comments=False)
    for n, (a, b) in enumerate(spans):
        placeholder = f'"{tag % n}"'
        if mini.count(placeholder) != 1:
            raise ValueError(f"template {n} did not survive minification")
        mini = mini.replace(placeholder, src[a:b])
    return mini

# csscompressor protects calc() but strips the spaces around `+` everywhere else
# (it's a selector combinator to it), so `clamp(2rem, 4vw + 0.8rem, 3.6rem)` came
# out as `4vw+0.8rem` - invalid, since math functions need whitespace around a
# binary +. The whole value then dropped and headings fell back to body size.
_MATH_FN = re.compile(r"\b(?:clamp|min|max|calc)\(", re.I)


def _respace_math_plus(css: str) -> str:
    out, i = [], 0
    for m in _MATH_FN.finditer(css):
        if m.start() < i:
            continue    # nested inside a function already handled
        depth, j = 1, m.end()
        while j < len(css) and depth:
            depth += {"(": 1, ")": -1}.get(css[j], 0)
            j += 1
        body = re.sub(r"(?<=[\w%)])\s*\+\s*(?=[\w.(])", " + ", css[m.end():j])
        out.append(css[i:m.end()] + body)
        i = j
    return "".join(out) + css[i:]


def discover_pairs() -> list[tuple[Path, Path]]:
    """Every top-level source `*.css` / `*.js` -> its `*.min.*` sibling.

    Skips files that are already minified outputs so re-runs don't try to
    minify `foo.min.css` into `foo.min.min.css`. Sorted for stable output."""
    pairs: list[tuple[Path, Path]] = []
    for src in sorted(STATIC.glob("*.css")) + sorted(STATIC.glob("*.js")):
        name = src.name
        if name.endswith(".min.css") or name.endswith(".min.js"):
            continue
        dst = src.with_name(f"{src.stem}.min{src.suffix}")
        pairs.append((src, dst))
    return pairs


def fmt_size(n: int) -> str:
    return f"{n / 1024:6.1f} KB"


def main() -> None:
    pairs = discover_pairs()
    print(f"{'file':<24}{'original':>12}{'minified':>12}{'saved':>10}")
    print("-" * 58)
    corrupted: list[str] = []
    for src, dst in pairs:
        raw = src.read_text(encoding="utf-8")
        if src.suffix == ".css":
            mini = _respace_math_plus(csscompressor.compress(raw))
        else:
            # A file the scanner can't follow ships as-is rather than risk it.
            try:
                mini = minify_js(raw)
            except (ValueError, IndexError) as e:
                corrupted.append(f"{src.name} ({e})")
                mini = raw
        dst.write_text(mini, encoding="utf-8")
        before = len(raw.encode("utf-8"))
        after = len(mini.encode("utf-8"))
        pct = 100 * (1 - after / before) if before else 0
        flag = "  !! kept source" if mini is raw and src.suffix == ".js" else ""
        print(f"{src.name:<24}{fmt_size(before):>12}{fmt_size(after):>12}{pct:>8.1f}%{flag}")
    print("-" * 58)
    print(f"{len(pairs)} files minified")
    if corrupted:
        print("\n!! WARNING: shipped unminified, the template scan failed on: " + ", ".join(corrupted))


if __name__ == "__main__":
    main()
