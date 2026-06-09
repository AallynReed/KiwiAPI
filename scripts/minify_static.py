"""Generate minified copies of every static CSS + JS asset the landing
page ships. Source files keep their original names (so editing is easy);
the minified output lives alongside as `*.min.css` / `*.min.js` and is
what nginx serves in production via the `?v=...`-suffixed paths in
index.html.

Tools:
  - csscompressor for CSS (handles `color-mix(...)`, `@keyframes`,
    `calc()`, modern syntax. Safer than naive regex strip-and-collapse.)
  - rjsmin for JS (Eric Sasse's port of Douglas Crockford's JSMin,
    handles string/regex literals + automatic semicolon insertion. Pure
    Python, no Node required.)

Re-runnable: existing `.min.*` files are overwritten in-place.

Run:  .venv/Scripts/python scripts/minify_static.py
"""
from pathlib import Path

import csscompressor
import rjsmin

ROOT = Path(__file__).resolve().parent.parent
STATIC = ROOT / "site" / "static"

# Files to minify: (source, minified output).
PAIRS = [
    (STATIC / "style.css",         STATIC / "style.min.css"),
    (STATIC / "app.js",            STATIC / "app.min.js"),
    (STATIC / "landing.js",        STATIC / "landing.min.js"),
    (STATIC / "i18n.js",           STATIC / "i18n.min.js"),
    (STATIC / "leaderboards.css",  STATIC / "leaderboards.min.css"),
    (STATIC / "leaderboards.js",   STATIC / "leaderboards.min.js"),
    (STATIC / "updates.css",       STATIC / "updates.min.css"),
    (STATIC / "updates.js",        STATIC / "updates.min.js"),
    (STATIC / "support.css",       STATIC / "support.min.css"),
    (STATIC / "account.css",       STATIC / "account.min.css"),
    (STATIC / "site_auth.js",      STATIC / "site_auth.min.js"),
    (STATIC / "dashboard.js",      STATIC / "dashboard.min.js"),
    (STATIC / "market.css",        STATIC / "market.min.css"),
    (STATIC / "market.js",         STATIC / "market.min.js"),
    (STATIC / "status.css",        STATIC / "status.min.css"),
    (STATIC / "status.js",         STATIC / "status.min.js"),
    (STATIC / "password_reset.js", STATIC / "password_reset.min.js"),
]


def fmt_size(n: int) -> str:
    return f"{n / 1024:6.1f} KB"


def main() -> None:
    print(f"{'file':<24}{'original':>12}{'minified':>12}{'saved':>10}")
    print("-" * 58)
    for src, dst in PAIRS:
        raw = src.read_text(encoding="utf-8")
        if src.suffix == ".css":
            mini = csscompressor.compress(raw)
        else:
            mini = rjsmin.jsmin(raw, keep_bang_comments=False)
        dst.write_text(mini, encoding="utf-8")
        before = len(raw.encode("utf-8"))
        after = len(mini.encode("utf-8"))
        pct = 100 * (1 - after / before) if before else 0
        print(f"{src.name:<24}{fmt_size(before):>12}{fmt_size(after):>12}{pct:>8.1f}%")


if __name__ == "__main__":
    main()
