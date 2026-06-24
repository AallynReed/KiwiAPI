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

# rjsmin's regex lexer doesn't track ES6 template-literal nesting perfectly: on
# deeply nested templates it can desync and treat template *text* as code, which
# strips the *significant* space in `${a} ${b}` -> `${a}${b}` (this is what made
# "Joined Jun 2026" render as "JoinedJun 2026"). It's intermittent - only some
# `} ${` collapse - so it's the worst kind of silent corruption. We can't make
# rjsmin parse templates, so instead: detect any lost `} ${` and refuse to ship
# the broken minified file (fall back to the unminified source for that file).
# The real fix lives in the source - put the space inside the interpolation as a
# string literal: `${esc(t('Joined')) + ' ' + fmtDate(d)}` - which no minifier
# touches. This net just guarantees a regression can never reach users silently.
_TMPL_SPACE = re.compile(r"\} \$\{")


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
            mini = csscompressor.compress(raw)
        else:
            mini = rjsmin.jsmin(raw, keep_bang_comments=False)
            # Guard: rjsmin must not eat a `${a} ${b}` template space. If it did,
            # the minified file would silently render words mashed together, so we
            # keep the source verbatim for that file instead (slightly larger, but
            # correct) and flag it loudly so the source can be fixed with `+ ' '`.
            if _TMPL_SPACE.findall(mini).__len__() < _TMPL_SPACE.findall(raw).__len__():
                corrupted.append(src.name)
                mini = raw
        dst.write_text(mini, encoding="utf-8")
        before = len(raw.encode("utf-8"))
        after = len(mini.encode("utf-8"))
        pct = 100 * (1 - after / before) if before else 0
        flag = "  !! kept source (template space lost)" if src.name in corrupted else ""
        print(f"{src.name:<24}{fmt_size(before):>12}{fmt_size(after):>12}{pct:>8.1f}%{flag}")
    print("-" * 58)
    print(f"{len(pairs)} files minified")
    if corrupted:
        print(
            "\n!! WARNING: rjsmin dropped template-literal spaces in: "
            + ", ".join(corrupted)
            + "\n   Those files were shipped UNMINIFIED to avoid 'JoinedJun'-style"
            "\n   corruption. Fix the source by moving the space inside the"
            "\n   interpolation, e.g. `${a + ' ' + b}`, then re-deploy."
        )


if __name__ == "__main__":
    main()
