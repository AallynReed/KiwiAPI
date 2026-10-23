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

Run from the repo root with the production-pinned venv:
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

KINDS = ["hourly_challenge", "challenge_collection", "challenge_rampage", "challenge_racing", "challenge_target",
         "challenge_dungeon", "chaos_chest", "daily_bonuses", "longshade", "wild_mana", "stampy", "corruxion",
         "fluxion", "luxion", "server_status", "weird_new_kind", "x", "ünïcode_straße"]


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
            if kind in ("challenge_rampage", "luxion", "daily_bonuses", "weird_new_kind", "server_status"):
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


def main() -> None:
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
             "announcements": anns}
    (OUT / "cases.json").write_text(json.dumps(cases, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(acts)} activity + {len(stats)} status cards written to {OUT}")


if __name__ == "__main__":
    main()
