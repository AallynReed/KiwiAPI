"""Golden fixtures for kiwi-server's embed surface (src/embed) and the Dressing
Room palette, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_embed_fixtures.py
Writes native/kiwi-server/testdata/embed/golden.json: the /embed/viewer page as
the API's Jinja2Templates renders site/templates/embed_viewer.html, read_tmod's
verdict on crafted uploads, UnicodeDecodeError texts and the DressPalette body.
"""
import base64
import json
import os
import struct
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "embed" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "embed-fixture-secret")

from fastapi.templating import Jinja2Templates  # noqa: E402

from app.trove import tmod as T  # noqa: E402

TEMPLATES = Jinja2Templates(directory=str(ROOT / "site" / "templates"))
KEYS = ("release", "tmod", "game", "prefab", "dress", "path", "sound", "backdrop", "mode", "theme")


def page_cases() -> list:
    out = []
    for vals in (
        {"mode": "auto", "theme": "dark"},
        {"release": "abc", "mode": "vfx", "theme": "light", "backdrop": "hat",
         "dress": "knight:x:<script>&\"'", "path": "a/b c.pkfx", "sound": "12"},
        {"game": "Ünïcode — “quotes” & <tags>", "prefab": "prefabs/x", "mode": "audio", "theme": "dark"},
    ):
        ctx = {k: vals.get(k, "") for k in KEYS} | {"api_base": "", "app_url": "https://trove.aallyn.net"}
        html = TEMPLATES.get_template("embed_viewer.html").render(ctx)
        out.append({"vars": ctx, "html": html})
    return out


def leb(n: int) -> bytes:
    b = bytearray()
    while n >= 0x80:
        b.append((n & 0x7F) | 0x80)
        n >>= 7
    b.append(n)
    return bytes(b)


def mk(props: int, body: bytes) -> bytes:
    return struct.pack("<QHH", 12 + len(body), 1, props) + body


def tmod_cases() -> list:
    good = T.build_tmod(1, {"title": "Café mod", "author": "x"}, [("ui/a.txt", b"hello"), ("blueprints/b.blueprint", b"\0" * 9)])
    hdr = int.from_bytes(good[:8], "little")
    samples = [
        good, b"short", b"\x05" + b"\0" * 11, struct.pack("<QHH", 9999, 1, 0) + b"\0" * 4,
        struct.pack("<QHH", 20, 1, 1) + leb(3) + b"\xff\xfe\xfd" + leb(1) + b"v" + b"\0",
        struct.pack("<QHH", 30, 1, 1) + leb(5) + b"ab", struct.pack("<QHH", 14, 1, 0) + b"\x05",
        struct.pack("<QHH", 22, 1, 1) + leb(2) + b"\xe2\x82" + leb(1) + b"v" + b"\0" * 6,
        struct.pack("<QHH", 25, 1, 1) + b"\xff" * 11 + b"\x01",
        good[:hdr - 3] + good[hdr:], bytes([hdr + 2]) + good[1:],
        mk(1, leb(5) + b"ab"), mk(1, b"\xff" * 10 + b"\x01"), mk(0, b"\x05ab"),
        mk(1, leb(2) + b"\xc3\xa9" + leb(1) + b"v"), mk(0, b"\x02\xff\xfe" + leb(1) * 4),
    ]
    out = []
    for s in samples:
        try:
            T.read_tmod(s)
            err = None
        except T.TmodError as e:
            err = str(e)
        out.append({"data": base64.b64encode(s).decode(), "error": err})
    return out


def utf8_cases() -> list:
    out = []
    for b in (b"\xff", b"ab\x80", b"\xc3\x28", b"\xe2\x82\x28", b"\xe2\x82", b"\xf0\x9f\x98", b"\xe0\x80\x80",
              b"\xed\xa0\x80", b"\xf4\x90\x80\x80", b"\xc0\xaf", b"ok\xf8"):
        try:
            b.decode("utf-8")
            msg = None
        except UnicodeDecodeError as e:
            msg = str(e)
        out.append({"data": base64.b64encode(b).decode(), "error": msg})
    return out


def main() -> None:
    from app.trove.dressing import palette as pal
    from app.trove.dressing.schemas import DressPalette

    palette = DressPalette(columns=pal.COLUMNS, hair=list(pal.HAIR), eyes=list(pal.EYE)).model_dump_json()
    data = {"pages": page_cases(), "tmods": tmod_cases(), "utf8": utf8_cases(), "palette": palette}
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, ensure_ascii=False, indent=0), encoding="utf-8")
    print(OUT, OUT.stat().st_size)


if __name__ == "__main__":
    main()
