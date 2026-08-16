"""Rebuild the served Font Awesome subset from the pristine vendor drop.

Font Awesome ships ~2,000 icons: 100 KB of render-blocking CSS and 280 KB of
woff2 for the ~200 icons the site actually uses. This walks the source tree for
``fa-*`` class names, keeps only those rules, and re-subsets the fonts to the
codepoints that survive.

Vendor originals live in ``vendor/fontawesome/`` and are never served; the
generated files land in ``site/static/fonts/``. Re-run after adding an icon, or
the new glyph renders as an empty box::

    pip install fonttools brotli
    python scripts/subset_fontawesome.py

Icon names built at runtime (``"fa-" + platform``) can't be found by scanning,
so they are listed in EXTRA below.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

from fontTools.subset import Options, Subsetter
from fontTools.ttLib import TTFont

ROOT = Path(__file__).resolve().parent.parent
VENDOR = ROOT / "vendor" / "fontawesome"
OUT_CSS = ROOT / "site" / "static" / "fonts" / "fontawesome.min.css"
OUT_FONTS = ROOT / "site" / "static" / "fonts" / "fa"

# Scanned for `fa-*` tokens. The vendor CSS is excluded (it names every icon).
SCAN_DIRS = ("site/templates", "site/static", "app")
SCAN_EXTS = {".html", ".js", ".css", ".py", ".json", ".txt"}

# Names assembled at runtime, so no literal `fa-<name>` exists to grep for.
EXTRA = {
    "fa-youtube",  # home.js: "fa-brands fa-" + platform
    "fa-twitch",
}

# One `.fa-a:before,.fa-b:before{content:"\f000"}` rule, aliases included.
ICON_RULE = re.compile(r'((?:\.fa-[a-z0-9-]+:before,)*\.fa-[a-z0-9-]+:before)\{content:"([^"]*)"\}')
TOKEN = re.compile(r"\bfa-[a-z0-9-]+")
CSS_ESCAPE = re.compile(r"\\([0-9a-fA-F]{1,6}) ?")


def used_names() -> set[str]:
    names = set(EXTRA)
    for d in SCAN_DIRS:
        for path in (ROOT / d).rglob("*"):
            if path.suffix not in SCAN_EXTS or not path.is_file():
                continue
            if ".min." in path.name or VENDOR in path.parents:
                continue
            names |= set(TOKEN.findall(path.read_text(encoding="utf-8", errors="ignore")))
    return names


def subset_css(css: str, names: set[str]) -> tuple[str, set[int]]:
    """Drop unused icon rules; return the CSS and the codepoints left in it."""
    codepoints: set[int] = set()
    kept = dropped = 0

    def rewrite(m: re.Match[str]) -> str:
        nonlocal kept, dropped
        selectors = [s for s in m.group(1).split(",") if s[1:-7] in names]
        if not selectors:
            dropped += 1
            return ""
        kept += 1
        codepoints.update(int(h, 16) for h in CSS_ESCAPE.findall(m.group(2)))
        return ",".join(selectors) + '{content:"' + m.group(2) + '"}'

    css = ICON_RULE.sub(rewrite, css)
    # Every browser that runs this site takes the woff2; the ttf fallbacks are
    # 650 KB we would otherwise have to subset and ship for nobody.
    css = re.sub(r',url\([^)]*\.ttf\) format\("truetype"\)', "", css)
    print(f"css: kept {kept} icon rules, dropped {dropped}")
    return css, codepoints


def subset_font(src: Path, dst: Path, codepoints: set[int]) -> None:
    font = TTFont(src)
    have = codepoints & set(font.getBestCmap())
    options = Options()
    options.flavor = "woff2"
    options.desubroutinize = True
    options.layout_features = []
    options.name_IDs = ["*"]
    options.notdef_outline = True
    subsetter = Subsetter(options=options)
    subsetter.populate(unicodes=have)
    subsetter.subset(font)
    font.flavor = "woff2"
    font.save(dst)
    before, after = src.stat().st_size, dst.stat().st_size
    print(f"{dst.name}: {len(have)} glyphs, {before:,} -> {after:,} bytes")


def main() -> int:
    if not VENDOR.is_dir():
        print(f"missing vendor drop at {VENDOR}", file=sys.stderr)
        return 1

    names = used_names()
    css, codepoints = subset_css(
        (VENDOR / "fontawesome.min.css").read_text(encoding="utf-8"), names
    )
    OUT_CSS.write_text(css, encoding="utf-8")
    OUT_FONTS.mkdir(parents=True, exist_ok=True)
    # Subset from the vendor .ttf: fontTools chokes decoding some of Font
    # Awesome's shipped woff2, and the two carry identical outlines anyway.
    for name in ("fa-solid-900", "fa-regular-400", "fa-brands-400", "fa-v4compatibility"):
        subset_font(VENDOR / "fa" / f"{name}.ttf", OUT_FONTS / f"{name}.woff2", codepoints)
    print(f"\ncss: {OUT_CSS.stat().st_size:,} bytes")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
