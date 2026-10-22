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
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_og_fixtures.py
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
    cases = {"now": NOW, "activity": acts, "status": stats, "hypot": hyp, "fixed2": fmts, "badges": badges}
    (OUT / "cases.json").write_text(json.dumps(cases, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"{len(acts)} activity + {len(stats)} status cards written to {OUT}")


if __name__ == "__main__":
    main()
