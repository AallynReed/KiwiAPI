"""Golden fixtures for kiwi-server's OG cards (src/og/tests.rs), from Python.

Runs the real app/site/og_image.py drawing functions (_draw, _draw_status) on
fixed inputs, with the font lists pointed at the Debian DejaVu files in
kiwi-pil/testdata/fonts and time.time() pinned. Writes each card's PNG plus
cases.json (inputs) to native/kiwi-server/testdata/og/, and Python's
math.hypot and f"{x:.2f}" results for the numeric helpers.

The PNG bytes are Pillow's Windows wheel output; the Rust tests compare the
decoded pixels (what production must match) and the bytes too (zlib-rs
reproduces this zlib-ng 2.3.3 stream). Production's Linux wheel deflates the
same pixels into a different stream; see the area notes.

The localized cards (l10n.json, l10n_*.png) render every site language from
snapshot catalogs (testdata/og/locales: the site and bot entries for the card
strings, refreshed by the `catalogs` step) with the subset Noto CJK/Thai faces
from make_og_test_fonts.py. i18n.json holds str.format and catalog edge cases
for app/i18n.py's t().

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_og_fixtures.py catalogs
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_og_fixtures.py
"""

from __future__ import annotations

import json
import math
import random
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "og"
FONTS = ROOT / "native" / "kiwi-pil" / "testdata" / "fonts"
PARITY = Path(__file__).resolve().parent

from PIL import features  # noqa: E402

from app import i18n  # noqa: E402
from app.site import og_image as og  # noqa: E402

assert not features.check("raqm")
og._REG[:] = [str(FONTS / "DejaVuSans.ttf")]
og._BOLD[:] = [str(FONTS / "DejaVuSans-Bold.ttf")]
NOW = 1792708167


class _Clock:
    @staticmethod
    def time() -> float:
        return float(NOW)


og.time = _Clock


class _DT(og.datetime):
    @classmethod
    def now(cls, tz=None):
        return og.datetime.fromtimestamp(NOW, tz)


og.datetime = _DT


def _merchant(active: bool, state, start: int, end: int) -> dict:
    out = {"active": active, "starts_at": start, "ends_at": end, "seconds_remaining": max(0, end - NOW),
           "schedule": []}
    if state is not None:
        out["state"] = state
    return out


def _slot(start: int, end: int, names: list) -> dict:
    return {"starts_at": start, "ends_at": end,
            "biomes": [{"name": n, "final_name": f, "icon": "x"} for n, f in names]}


SOURCES = {
    "a": {
        "challenge": {"starts_at": NOW - 600, "ends_at": NOW + 1300, "active": True, "is_friday_window": False,
                      "seconds_remaining": 1300, "name": "Rampage in the Sundered Uplands of Infinite Peril Forever",
                      "type": "rampage", "captured_at": None},
        "chaos": {"active": True, "starts_at": NOW - 86400, "ends_at": NOW + 5 * 86400 + 7300,
                  "item": {"name": "Ultimate Shadow Hunter Style", "blueprint": None}},
        "daily": {"current": {"name": "Gem Day", "normal_buffs": ["+50% Gem Dust", "+50% Gem Boxes", "x"]}},
        "server": {"daily_reset_at": NOW + 3 * 3600 + 125, "weekly_reset_at": NOW + 4 * 86400 + 3 * 3600},
        "biomes": {"current": _slot(NOW - 100, NOW + 2 * 3600 + 59, [("Desert", "Desert Frontier"),
                                                                       ("Jungle", ""), ("Tundra", "Permafrost")]),
                   "upcoming": []},
        "wild_mana": {"current": _slot(NOW - 100, NOW + 6 * 86400, [("Fae Forest", "Fae Forest"),
                                                                      ("Lunar Lagoon", "Lunar Lagoon")])},
        "stampy": {"current": _slot(NOW - 100, NOW + 30 * 3600, [("Candoria", "Candoria")])},
        "corruxion": _merchant(True, None, NOW - 3600, NOW + 2 * 3600 + 30),
        "fluxion": _merchant(True, "Flux", NOW - 3600, NOW + 45 * 60),
        "luxion": {"active": True, "current_window": {"starts_at": NOW - 60, "ends_at": NOW + 7000},
                   "next_window": {"starts_at": NOW + 99999, "ends_at": NOW + 200000}, "ends_at": NOW + 10 ** 6},
        "status": {"overall": "down", "checked_at": NOW - 30,
                   "environments": {"eu": {"status": "online"}, "us": {"status": "down"}, "pts": None}},
    },
    "b": {
        "challenge": {"starts_at": NOW - 4000, "ends_at": NOW - 400, "active": False, "is_friday_window": True,
                      "seconds_remaining": 0, "name": None, "type": None, "captured_at": None},
        "chaos": {"active": True, "starts_at": NOW - 86400, "ends_at": NOW + 86400, "item": None},
        "daily": {"current": {}},
        "server": {"daily_reset_at": NOW + 59, "weekly_reset_at": NOW - 5},
        "biomes": {"current": _slot(NOW - 100, NOW + 2 * 60, []), "upcoming": []},
        "wild_mana": {"current": None},
        "stampy": {"current": None},
        "corruxion": _merchant(False, None, NOW + 86400 * 3 + 7200, NOW + 86400 * 4),
        "fluxion": _merchant(True, "away", NOW - 60, NOW + 60),
        "luxion": {"active": False, "current_window": None, "next_window": None, "ends_at": None},
        "status": None,
    },
    "c": {
        "challenge": {"starts_at": NOW - 10, "ends_at": NOW + 1790, "active": True, "is_friday_window": True,
                      "seconds_remaining": 1790, "name": "", "type": "collection"},
        "chaos": {"active": True, "starts_at": NOW - 86400, "ends_at": None, "item": {"name": "Box"}},
        "daily": {"current": {"name": "", "normal_buffs": []}},
        "server": {"daily_reset_at": NOW + 86400 * 2, "weekly_reset_at": NOW + 1},
        "biomes": {"current": None},
        "wild_mana": {"current": _slot(NOW, NOW + 3600 * 23, [("", "")])},
        "stampy": {"current": {"biomes": [], "ends_at": NOW + 10}},
        "corruxion": _merchant(False, None, 0, 0),
        "fluxion": _merchant(False, "away", NOW + 3599, NOW + 9000),
        "luxion": {"active": True, "current_window": None, "next_window": {"starts_at": NOW + 3600 * 5},
                   "ends_at": NOW + 10},
        "status": {"overall": "online", "environments": {"eu": {"status": "online"}, "us": {"status": "online"},
                                                          "pts": {"status": "maintenance"}}},
    },
}
SOURCES["d"] = dict(SOURCES["c"], luxion={"active": True, "current_window": None, "next_window": None,
                                          "ends_at": NOW + 86400 * 9}, status={"overall": "unknown"})
SOURCES["a"]["game_update"] = {"version_tag": "live-us 2026.10.21 (#1187)", "files_added": 12,
                               "files_modified": 3456, "files_removed": 0}
SOURCES["b"]["game_update"] = None
SOURCES["c"]["game_update"] = {"version_tag": "0", "files_added": 0, "files_modified": 1, "files_removed": 1234567}
SOURCES["d"]["game_update"] = None

KINDS = ["hourly_challenge", "challenge_collection", "challenge_rampage", "challenge_racing", "challenge_target",
         "challenge_dungeon", "chaos_chest", "daily_bonuses", "longshade", "wild_mana", "stampy", "corruxion",
         "fluxion", "luxion", "server_status", "game_update", "weird_new_kind", "x", "ünïcode_straße"]


def _patch_sources(src: dict) -> None:
    import app.trove.captures as captures
    import app.trove.chaos as chaos
    import app.trove.luxion as luxion
    import app.trove.rotations as rotations
    import app.trove.server_time as server_time
    import app.trove.status as status

    async def _challenge():
        return src["challenge"]

    async def _chaos():
        return src["chaos"]

    async def _luxion():
        return src["luxion"]

    captures.get_current_challenge = _challenge
    chaos.get_chaos_chest = _chaos
    luxion.get_luxion = _luxion
    server_time.daily_buffs = lambda: src["daily"]
    server_time.server_time = lambda: src["server"]
    server_time.corruxion = lambda: src["corruxion"]
    server_time.fluxion = lambda: src["fluxion"]
    rotations.biome_rotation = lambda: src["biomes"]
    rotations.wild_mana = lambda: src["wild_mana"]
    rotations.stampy = lambda: src["stampy"]
    status.get_status = lambda: src["status"] if src["status"] is not None else {}

    import types

    import app.trove.updates.read as updates_read

    async def _latest(branch):
        assert branch == "live-us"
        v = src["game_update"]
        return types.SimpleNamespace(**v) if v is not None else None

    updates_read.latest_version = _latest


def board_and_announce() -> tuple[list, list]:
    import asyncio

    boards = []
    for name, src in SOURCES.items():
        data = {"challenge": src["challenge"], "chaos": src["chaos"],
                "biomes": (src["biomes"].get("current") or {}), "corruxion": src["corruxion"],
                "fluxion": src["fluxion"], "server": src["server"], "status": src["status"]}
        (OUT / f"board_{name}.png").write_bytes(og._draw_board(data))
        boards.append({"name": name, "data": data})
    anns = []
    for name, src in SOURCES.items():
        _patch_sources(src)
        for kind in KINDS:
            title, accent, lines = asyncio.run(og._announcement_content(kind))
            row = {"set": name, "kind": kind, "title": title, "accent": list(accent), "lines": lines}
            if kind in ("challenge_rampage", "luxion", "daily_bonuses", "weird_new_kind", "server_status", "game_update"):
                row["png"] = f"announce_{name}_{kind}.png"
                (OUT / row["png"]).write_bytes(og._draw_announcement(title, accent, lines))
            anns.append(row)
    return boards, anns


def _points(rng: random.Random, start: int, n: int, step: int, base: float, jitter: float) -> list[dict]:
    out = []
    t = start
    for _ in range(n):
        v = float(round(base + rng.uniform(-jitter, jitter)))
        out.append({"t": t, "active": v, "avg": v, "peak": v, "samples": 1})
        t += step + rng.randrange(-120, 120)
    return out


def activity_cases() -> list[dict]:
    rng = random.Random(42)
    cases = []
    prod = PARITY / "data" / "og"
    for period in ("7d", "1m"):
        series = json.loads((prod / f"series_{period}.json").read_text(encoding="utf-8"))
        live = json.loads((prod / "current.json").read_text(encoding="utf-8"))
        live = {k: live.get(k) for k in ("estimate_24h", "estimate_7d")}
        cases.append({"name": f"prod_{period}", "period": period, "series": series, "live": live})

    ws = 1792103367
    pts = _points(rng, ws + 1200, 28, 21600, 5000, 900)
    gappy = pts[:5] + pts[9:10] + pts[14:22] + pts[25:]
    cases.append({"name": "gaps_7d", "period": "7d",
                  "series": {"bucket_seconds": 21600, "window_start": ws, "window_end": ws + 7 * 86400,
                             "points": gappy, "latest": 5123.4},
                  "live": {"estimate_24h": None, "estimate_7d": None}})
    cases.append({"name": "inferred_bucket_1m", "period": "1m",
                  "series": {"bucket_seconds": None, "window_start": ws - 23 * 86400, "window_end": ws + 7 * 86400,
                             "points": _points(rng, ws - 23 * 86400 + 600, 40, 64800, 12000, 4000)[:17]
                             + _points(rng, ws - 3 * 86400, 12, 64800, 9000, 3000), "latest": 0},
                  "live": {"estimate_24h": 1234567, "estimate_7d": 98765432}})
    cases.append({"name": "flat_zero_7d", "period": "7d",
                  "series": {"bucket_seconds": 21600, "window_start": ws, "window_end": ws + 7 * 86400,
                             "points": [{"t": ws + i * 21600, "active": 0.0} for i in range(28)], "latest": None},
                  "live": {"estimate_24h": 0, "estimate_7d": 0}})
    cases.append({"name": "warming_up", "period": "7d",
                  "series": {"bucket_seconds": 21600, "window_start": ws, "window_end": ws + 7 * 86400,
                             "points": [{"t": ws + 600, "active": 4000.0}], "latest": 4000.5},
                  "live": {"estimate_24h": None, "estimate_7d": 7}})
    cases.append({"name": "two_points_outside_window", "period": "7d",
                  "series": {"bucket_seconds": 3600, "window_start": ws, "window_end": ws + 7 * 86400,
                             "points": [{"t": ws - 7200, "active": 10.0}, {"t": ws + 8 * 86400, "active": 99999.0}],
                             "latest": 2.5},
                  "live": {"estimate_24h": None}})
    return cases


def status_cases() -> list[dict]:
    env = lambda s: {"status": s, "online": s == "online", "game": None}  # noqa: E731
    return [
        {"name": "online", "payload": {"overall": "online", "auth": None, "checked_at": NOW - 12,
                                       "environments": {"eu": env("online"), "us": env("online"), "pts": env("online")}},
         "uptimes": [0.99871, 1.0, 0.951234]},
        {"name": "partial", "payload": {"overall": "down", "checked_at": NOW - 125,
                                        "environments": {"eu": env("online"), "us": env("down"), "pts": env("down")}},
         "uptimes": [None, 0.5, 0.00125]},
        {"name": "down_hours", "payload": {"overall": "down", "checked_at": NOW - 7300,
                                           "environments": {"eu": env("maintenance"), "us": None, "pts": {}}},
         "uptimes": [0.0, None, 0.99995]},
        {"name": "unknown_shell", "payload": {"overall": "unknown", "auth": None, "checked_at": None,
                                              "environments": {k: env("unknown") for k in ("eu", "us", "pts")}},
         "uptimes": [None, None, None]},
        {"name": "maintenance_future", "payload": {"overall": "maintenance", "checked_at": NOW + 50,
                                                   "environments": {"eu": {"status": None}, "us": env("weird")}},
         "uptimes": [0.12345, 0.98765, 0.5]},
    ]


LOCALES = OUT / "locales"
EDGE = OUT / "locales_edge"
TEST_FONTS = {
    "cjk_regular": FONTS / "NotoSansCJK-subset-Regular.ttc",
    "cjk_bold": FONTS / "NotoSansCJK-subset-Bold.ttc",
    "thai_regular": FONTS / "NotoSansThai-subset-Regular.ttf",
    "thai_bold": FONTS / "NotoSansThai-subset-Bold.ttf",
}
L10N_LANGS = [c for c, _ in i18n.LANGS if c != "en"]
BOARD_SET = {"fr": "b", "de": "c", "pt-PT": "d", "es": "b", "ru": "c", "ja": "a", "ko": "a", "zh-CN": "a", "th": "a"}


def og_keys() -> set[str]:
    """Every English string the cards pass to t()."""
    import ast

    tree = ast.parse((ROOT / "app" / "site" / "og_image.py").read_text(encoding="utf-8"))
    keys = {n.args[0].value for n in ast.walk(tree)
            if isinstance(n, ast.Call) and getattr(n.func, "id", None) == "t" and n.args
            and isinstance(n.args[0], ast.Constant) and isinstance(n.args[0].value, str)}
    keys |= {title for title, _ in og._BOARD_CARDS}
    keys |= set(og._ST_LABEL.values()) | set(og._ST_OVERALL.values()) | set(og._ANN_TITLE.values())
    keys |= set(og._PERIOD_LABEL.values()) | {k.replace("_", " ").title() for k in KINDS}
    return keys


def write_catalogs() -> None:
    """Snapshot the site and bot catalog entries the cards look up."""
    keys = {i18n._norm(k) for k in og_keys()}
    for sub, src in (("site", ROOT / "site" / "static" / "locales"), ("app", ROOT / "app" / "locales")):
        (LOCALES / sub).mkdir(parents=True, exist_ok=True)
        for lang in L10N_LANGS:
            data = json.loads((src / f"{lang}.json").read_text(encoding="utf-8"))
            kept = {k: v for k, v in data.items() if isinstance(k, str) and i18n._norm(k) in keys}
            (LOCALES / sub / f"{lang}.json").write_text(json.dumps(kept, ensure_ascii=False, indent=1) + "\n",
                                                        encoding="utf-8")


def _board_data(src: dict) -> dict:
    return {"challenge": src["challenge"], "chaos": src["chaos"], "biomes": (src["biomes"].get("current") or {}),
            "corruxion": src["corruxion"], "fluxion": src["fluxion"], "server": src["server"],
            "status": src["status"]}


def localized() -> list[dict]:
    """Status, board and banner cards in every other language."""
    import asyncio

    for p in TEST_FONTS.values():
        assert p.exists(), f"{p} missing: run make_og_test_fonts.py"
    i18n._SITE_LOCALES = LOCALES / "site"
    i18n._BOT_LOCALES = LOCALES / "app"
    i18n._catalog.cache_clear()
    og._CJK_REG[:] = [str(TEST_FONTS["cjk_regular"])]
    og._CJK_BOLD[:] = [str(TEST_FONTS["cjk_bold"])]
    og._THAI_REG[:] = [str(TEST_FONTS["thai_regular"])]
    og._THAI_BOLD[:] = [str(TEST_FONTS["thai_bold"])]
    stats = status_cases()
    sets = list(SOURCES)
    rows = []
    for li, lang in enumerate(L10N_LANGS):
        i18n.set_current_language(lang)
        st = stats[li % len(stats)]
        png = f"l10n_{lang}_status.png"
        (OUT / png).write_bytes(og._draw_status(st["payload"], dict(zip(("eu", "us", "pts"), st["uptimes"]))))
        rows.append({"lang": lang, "card": "status", "case": st["name"], "png": png})
        png = f"l10n_{lang}_board.png"
        (OUT / png).write_bytes(og._draw_board(_board_data(SOURCES[BOARD_SET[lang]])))
        rows.append({"lang": lang, "card": "board", "set": BOARD_SET[lang], "png": png})
        drawn = {(sets[(li + j) % len(sets)], KINDS[(3 * li + j) % len(KINDS)]) for j in range(3)}
        if lang in ("ja", "th"):
            drawn |= {("a", "game_update"), ("a", "server_status")}
        for set_name, src in SOURCES.items():
            _patch_sources(src)
            for kind in KINDS:
                title, accent, lines = asyncio.run(og._announcement_content(kind))
                row = {"lang": lang, "card": "announce", "set": set_name, "kind": kind, "title": title,
                       "accent": list(accent), "lines": lines}
                if (set_name, kind) in drawn:
                    slug = kind if kind.isascii() else f"kind{KINDS.index(kind)}"
                    row["png"] = f"l10n_{lang}_{set_name}_{slug}.png"
                    (OUT / row["png"]).write_bytes(og._draw_announcement(title, accent, lines))
                rows.append(row)
    i18n.set_current_language("en")
    return rows


FORMAT_ARGS = {"when": "in 2h", "d": 3, "h": 14, "n": 42, "pct": "99.87", "s": "héllo wörld ünï", "neg": -1234567,
               "z": 0, "big": 1234567890123, "nb": "a b", "q": "it's \"x\"", "nl": "a\nb\t\x07", "e": ""}
FORMAT_HAND = [
    "", "plain", "{when}", "Ends {when}", "{{when}}", "{{", "}}", "{", "}", "a}b", "a{b", "{when", "{when!", "{when!r",
    "{when!r}", "{when!s}", "{when!a}", "{s!a}", "{q!r}", "{nl!r}", "{nb!r}", "{nb!a}", "{when!x}", "{when!rr}",
    "{when:}", "{when:>12}", "{when:*^13}", "{when:.3}", "{when:<5.1}", "{when:s}", "{when:d}", "{when:,}",
    "{when:=5}", "{when:+}", "{when: }", "{when:#}", "{when:z}", "{when:05}", "{d:05}", "{neg:010,}", "{big:_}",
    "{big:,}", "{neg:+}", "{d: }", "{d:x}", "{d:#X}", "{neg:#b}", "{big:_x}", "{big:,x}", "{d:c}", "{neg:c}",
    "{d:n}", "{big:n}", "{d:.2}", "{d:e}", "{neg:.3e}", "{big:g}", "{d:%}", "{z:.0%}", "{d:#.0f}", "{big:,.2f}",
    "{big:020,.1f}", "{neg:G}", "{d:#g}", "{z:g}", "{z:#g}", "{d:=+8}", "{d:^9}", "{neg:<9}", "{d:s}", "{d:z}",
    "{d:,_}", "{d:_,}", "{d:,,}", "{d:xx}", "{d:.}", "{0}", "{}", "{1}", "{d}{}", "{}{0}", "{d[0]}", "{s[0]}",
    "{s[1]}{s[99]}", "{s[x]}", "{s[]}", "{s[0}", "{s[0]x}", "{s.}", "{s.x}", "{d.real}", "{d.imag}{d.numerator}",
    "{neg.denominator}", "{d.x}", "{when:{d}}", "{when:>{d}}", "{when:{s}}", "{when:{when:{d}}}", "{when:{}}",
    "{when:{{}}", "{missing}", "x {missing!r:>3} y", "{ when}", "{when }", "{é}", "{d:٣}", "{d:５}", "{d!r:>5}",
    "{neg!s:>12}", "{e:>3}", "{e!r}", "}{when}", "{when}}", "{when}{", "{d:{d}{d}}",
]


def format_cases() -> list:
    """str.format(**FORMAT_ARGS) outcomes: hand-written and random templates."""
    import re

    rng = random.Random(11)
    alphabet = list("{}{}{}{}[]:!._,  0123456789") + ["when", "d", "n", "s", "neg", "z", "big", "pct", "q",
                                                     "nb", "e", "x"] + list("rsa<>=^+-#%xXobcdefgnEFGz") + ["é", "٣"]
    templates = list(FORMAT_HAND)
    while len(templates) < 4000:
        t = "".join(rng.choice(alphabet) for _ in range(rng.randint(1, 12)))
        if not re.search(r"\d{4}", t):
            templates.append(t)
    out = []
    for tpl in templates:
        try:
            res = ["ok", tpl.format(**FORMAT_ARGS)]
        except (KeyError, IndexError, ValueError):
            res = ["lookup"]
        except Exception as e:  # noqa: BLE001 - the class is the expectation
            res = ["other", type(e).__name__]
        if res[0] == "ok" and " at 0x" in res[1]:
            continue
        out.append([tpl, res])
    return out


EDGE_FILES = {
    ("site", "fr"): json.dumps({
        "Ends {when}": "Fin {quand}", "  Leaves   {when} ": "Part {when}", "Away": "", "Online": 5,
        "Here now": "Ici", "Arrives {when}": "{0}", "Rotates {when}": "{when.x}", "Daily {when}": "Quotidien {when:>12}",
        "now": "maintenant", "in {m}m": "dans {m:03d} min", "Status unknown": "Inconnu !",
        "Next {when}": "Suivant {{when}}", "Live\tstatus": "En direct", "Updated {time} UTC": "MAJ {time:.2} UTC",
    }, ensure_ascii=False).encode(),
    ("app", "fr"): json.dumps({"Here now": "Là", "Away": "Absent", "Leaves {when}": "Sort {when}"},
                              ensure_ascii=False).encode(),
    ("site", "de"): "﻿".encode() + b'{"Away": "Weg"}',
    ("app", "de"): b'{"Away ": "Fort", "Online": "On"}',
    ("site", "es"): b'{"Away": ',
    ("site", "ru"): b"[1, 2]",
    ("site", "ja"): '{"Away": "1", "Away ": "2", "Away": "3", "Online": "", "Down": null, "Unknown": ["x"], '
                    '"now": "今"}'.encode(),
    ("site", "ko"): b'{"Away": "\xff"}',
    ("app", "ko"): json.dumps({"Away": "자리 비움"}, ensure_ascii=False).encode(),
    ("site", "zh-CN"): b'"just a string"',
    ("site", "th"): '{\r\n "Away": "ไม่อยู่",\r\n "Online":\r\n "ออนไลน์"\r\n}\r\n'.encode(),
}
EDGE_PROBES = [
    ("Ends {when}", {"when": "in 2h"}), ("Leaves {when}", {"when": "in 5m"}), ("Away", {}), ("Online", {}),
    ("Here now", {}), ("Arrives {when}", {"when": "now"}), ("Rotates {when}", {"when": "x"}),
    ("Daily {when}", {"when": "in 1d"}), ("now", {}), ("in {m}m", {"m": 7}), ("Status unknown", {}),
    ("Next {when}", {"when": "z"}), ("Live status", {}), ("Live  status ", {}), ("Down", {}), ("Unknown", {}),
    ("Updated {time} UTC", {"time": "17:49"}), ("Unknown key", {}), ("Unknown {x}", {"x": 1}),
    ("Bad {x", {"x": 1}), ("Bad {x.y}", {"x": 1}), ("Bad {y}", {"x": 1}),
]


def edge_cases() -> list[dict]:
    """t() over catalogs that are missing, malformed, colliding or not objects."""
    import shutil

    shutil.rmtree(EDGE, ignore_errors=True)
    for (sub, lang), raw in EDGE_FILES.items():
        (EDGE / sub).mkdir(parents=True, exist_ok=True)
        (EDGE / sub / f"{lang}.json").write_bytes(raw)
    i18n._SITE_LOCALES = EDGE / "site"
    i18n._BOT_LOCALES = EDGE / "app"
    i18n._catalog.cache_clear()
    out = []
    for lang in L10N_LANGS + ["en", "xx"]:
        for text, fmt in EDGE_PROBES:
            try:
                res = ["ok", i18n.t(text, lang, **fmt)]
            except Exception as e:  # noqa: BLE001 - the class is the expectation
                res = ["error", type(e).__name__]
            out.append({"lang": lang, "text": text, "fmt": fmt, "res": res})
    return out


def fallback_cases() -> dict:
    """Cards drawn when no _REG / _BOLD face opens, so _font ends on
    ImageFont.load_default() (Aileron at size 10)."""
    import hashlib

    from PIL import ImageFont

    reg, bold = list(og._REG), list(og._BOLD)
    rows = []
    try:
        for name, (r, b) in {"none": ([], []), "no_bold": (reg, [])}.items():
            og._REG[:] = r + ["/nonexistent/DejaVuSans.ttf"]
            og._BOLD[:] = b + ["/nonexistent/DejaVuSans-Bold.ttf"]
            st, act = status_cases()[1], activity_cases()[2]
            pngs = {
                "status": og._draw_status(st["payload"], dict(zip(("eu", "us", "pts"), st["uptimes"]))),
                "board": og._draw_board(_board_data(SOURCES["a"])),
                "activity": og._draw(act["series"], act["live"], act["period"]),
            }
            _patch_sources(SOURCES["a"])
            import asyncio

            title, accent, lines = asyncio.run(og._announcement_content("luxion"))
            pngs["announce"] = og._draw_announcement(title, accent, lines)
            for card, png in pngs.items():
                (OUT / f"fallback_{name}_{card}.png").write_bytes(png)
            rows.append({"name": name, "regular": bool(r), "bold": bool(b), "status": st["name"],
                         "activity": act["name"], "board": "a", "announce": ["a", "luxion"]})
    finally:
        og._REG[:], og._BOLD[:] = reg, bold
    return {"default_font_sha256": hashlib.sha256(ImageFont.load_default().font_bytes).hexdigest(), "cases": rows}


def main() -> None:
    if sys.argv[1:] == ["catalogs"]:
        write_catalogs()
        print(f"catalog snapshots written to {LOCALES}")
        return
    OUT.mkdir(parents=True, exist_ok=True)
    acts = activity_cases()
    for c in acts:
        png = og._draw(c["series"], c["live"], c["period"])
        (OUT / f"activity_{c['name']}.png").write_bytes(png)
    stats = status_cases()
    for c in stats:
        png = og._draw_status(c["payload"], dict(zip(("eu", "us", "pts"), c["uptimes"])))
        (OUT / f"status_{c['name']}.png").write_bytes(png)

    rng = random.Random(7)
    hyp = []
    for _ in range(400):
        x, y = rng.uniform(-1200, 1200), rng.uniform(-700, 700)
        if rng.random() < 0.3:
            x, y = float(rng.randrange(-1200, 1200)), float(rng.randrange(-700, 700))
        hyp.append([repr(x), repr(y), repr(math.hypot(x, y))])
    fmts = [[repr(v), f"{v:.2f}"] for v in (0.125, 0.375, 2.675, 1.005, 99.125, 99.995, 12.5, 0.0, 100.0, 98.7654321,
                                       0.00125 * 100, 0.99995 * 100, 0.951234 * 100, 0.12345 * 100)]
    fmts += [[repr(x), f"{x:.2f}"] for x in (rng.uniform(0, 100) for _ in range(200))]
    from app.site.router import _badge_svg
    badges = [[v, c, _badge_svg("trove", v, c)] for v, c in
              (("online", "#3fb950"), ("offline", "#f85149"), ("unknown", "#9aa4b2"))]
    boards, anns = board_and_announce()
    cases = {"now": NOW, "hhmm": _DT.now(og.timezone.utc).strftime("%H:%M"), "activity": acts, "status": stats,
             "hypot": hyp, "fixed2": fmts, "badges": badges, "sources": SOURCES, "boards": boards,
             "announcements": anns, "fallback": fallback_cases()}
    (OUT / "cases.json").write_text(json.dumps(cases, ensure_ascii=False) + "\n", encoding="utf-8")
    l10n = localized()
    (OUT / "l10n.json").write_text(json.dumps(l10n, ensure_ascii=False) + "\n", encoding="utf-8")
    i18n_cases = {"format_args": FORMAT_ARGS, "format": format_cases(), "edge": edge_cases()}
    (OUT / "i18n.json").write_text(json.dumps(i18n_cases, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(acts)} activity + {len(stats)} status cards, {len(l10n)} localized rows written to {OUT}")


if __name__ == "__main__":
    main()
