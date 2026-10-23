"""Subset Noto faces for the localized OG card fixtures (gen_og_fixtures.py).

Production loads Debian's NotoSansCJK-{Regular,Bold}.ttc (static CFF) and
NotoSansThai-{Regular,Bold}.ttf. These test faces keep the same FreeType paths
in a few hundred KB, cut to the characters the snapshot catalogs and fixtures
use: a static-CFF CJK face (Pillow's Tests/fonts/NotoSansJP-Regular.otf) as
"Regular" and the pan-CJK CFF2 face 0 of Android Studio's NotoSansCJK-Regular.ttc
as "Bold", each in a one-font TTC, plus Android's unhinted Noto Sans Thai
(FreeType auto-hints it). All are SIL Open Font License 1.1.

Run after `gen_og_fixtures.py catalogs`, with any Python that has fontTools:
    python native/port/parity/make_og_test_fonts.py PILLOW_TESTS_FONTS_DIR ANDROID_LAYOUTLIB_FONTS_DIR
"""

from __future__ import annotations

import json
import sys
from pathlib import Path

from fontTools import subset
from fontTools.ttLib import TTFont
from fontTools.ttLib.ttCollection import TTCollection

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-pil" / "testdata" / "fonts"
LOCALES = ROOT / "native" / "kiwi-server" / "testdata" / "og" / "locales"


def unicodes() -> set[int]:
    chars = set(map(chr, range(0x20, 0x7F))) | set("…—·−~%№ü")
    for f in LOCALES.glob("*/*.json"):
        for k, v in json.loads(f.read_text(encoding="utf-8")).items():
            chars |= set(k) | (set(v) if isinstance(v, str) else set())
    return {ord(c) for c in chars}


def cut(src: Path, dst: Path, index: int | None) -> None:
    opts = subset.Options()
    opts.name_IDs = ["*"]
    opts.name_languages = ["*"]
    opts.layout_features = ["*"]
    opts.notdef_outline = True
    font = TTFont(src, fontNumber=index) if index is not None else TTFont(src)
    sub = subset.Subsetter(opts)
    sub.populate(unicodes=unicodes())
    sub.subset(font)
    if dst.suffix == ".ttc":
        coll = TTCollection()
        coll.fonts.append(font)
        coll.save(dst)
    else:
        font.save(dst)
    print(f"{dst.name}: {dst.stat().st_size} bytes")


def main() -> None:
    pillow, android = Path(sys.argv[1]), Path(sys.argv[2])
    cut(pillow / "NotoSansJP-Regular.otf", OUT / "NotoSansCJK-subset-Regular.ttc", None)
    cut(android / "NotoSansCJK-Regular.ttc", OUT / "NotoSansCJK-subset-Bold.ttc", 0)
    cut(android / "NotoSansThai-Regular.ttf", OUT / "NotoSansThai-subset-Regular.ttf", None)
    cut(android / "NotoSansThai-Bold.ttf", OUT / "NotoSansThai-subset-Bold.ttf", None)


if __name__ == "__main__":
    main()
