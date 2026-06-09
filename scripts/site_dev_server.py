"""Tiny local dev server for previewing the BetterTroveTools showcase site
WITHOUT spinning up the full FastAPI stack (which needs MongoDB + Redis +
env config).

Serves:
- /                        → site/templates/index.html
- /commands                → site/templates/commands.html
- /leaderboards            → site/templates/leaderboards.html
- /static/*                → site/static/*
- /site/leaderboards/*     → stub JSON responses with realistic shapes

Run via the "site" config in .claude/launch.json; access at
http://localhost:8913/leaderboards.
"""
from __future__ import annotations

import json
from http.server import HTTPServer, SimpleHTTPRequestHandler
from pathlib import Path
from urllib.parse import urlparse, parse_qs

ROOT = Path(__file__).resolve().parent.parent
SITE_DIR = ROOT / "site"
TEMPLATES = SITE_DIR / "templates"
STATIC = SITE_DIR / "static"

# Stub data shaped to match what app/trove/leaderboards/service.py returns,
# so the page renders end-to-end without a backend.
#
# Anchors are computed at module-import time relative to "now" so the
# 7-day picker on the page shows recent trove-days. We seed FIVE of the
# last 7 days so two slots render as "No data" — exercising the empty
# state. Each anchor is real UTC 11:00 of its trove-day (matches the
# current API's daily-anchor model).
import time as _time
_DAY = 86400
_TROVE_OFFSET = 11 * 3600
_now = int(_time.time())
_today_trove_key = (_now - _TROVE_OFFSET) // _DAY
# Days 0 (today), 1 (yesterday), 2, 4, 6 ago — skip 3 and 5 so the
# picker visibly shows two "No data" gaps.
_seeded_days = {0, 1, 2, 4, 6}
_anchors = sorted(
    [(_today_trove_key - d) * _DAY + _TROVE_OFFSET for d in _seeded_days],
    reverse=True,
)
STUB_ANCHOR = _anchors[0]
STUB_TIMESTAMPS = _anchors

STUB_BOARDS = [
    {"uuid": 1012, "name_id": "Leaderboard_GlyphKicker", "name": "GLYPH KICKER",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "daily", "reset_kind": "daily", "player_board": True},
    {"uuid": 20, "name_id": "Leaderboard_GeodeMetaExperience", "name": "GEODE MASTERY POINTS",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "default", "player_board": True},
    {"uuid": 33001, "name_id": "Leaderboard_HartsReceived", "name": "HART-A-PHONES RECEIVED",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 2004, "name_id": "Leaderboard_DelveDepth_Challenge", "name": "CHALLENGE: Deepest (WEEKLY)",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 5001, "name_id": "Leaderboard_ClubExp", "name": "CLUB MASTERY",
     "category_id": "Leaderboard_Category_Clubs", "category": "CLUBS",
     "contest_type": None, "reset_kind": "default", "player_board": False},
]
STUB_ENTRIES = [
    {"player_name": "Skill", "rank": 1, "score": 59731.0},
    {"player_name": "noa00__00", "rank": 1, "score": 59731.0},
    {"player_name": "MaxOG", "rank": 3, "score": 59629.0},
    {"player_name": "Bae", "rank": 4, "score": 59560.0},
    {"player_name": "VatsanT", "rank": 5, "score": 59511.0},
    {"player_name": "Aallyn", "rank": 6, "score": 58032.0},
    {"player_name": "TestPlayer", "rank": 7, "score": 57000.0},
]


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        path = url.path

        # Page routes.
        if path == "/":
            return self._send_file(TEMPLATES / "index.html", "text/html")
        if path == "/commands":
            return self._send_file(TEMPLATES / "commands.html", "text/html")
        if path == "/leaderboards":
            return self._send_file(TEMPLATES / "leaderboards.html", "text/html")
        if path == "/updates":
            return self._send_file(TEMPLATES / "updates.html", "text/html")
        if path == "/status":
            return self._send_file(TEMPLATES / "status.html", "text/html")

        # Static.
        if path.startswith("/static/"):
            return self._send_file(STATIC / path[len("/static/"):], None)

        # Stub JSON endpoints.
        if path == "/site/leaderboards/config":
            # Return a non-3 value so the subtitle change is visible in
            # the local preview (prod would normally serve 3 from the
            # runtime_config default).
            return self._send_json({"hot_retention_days": 5})
        if path == "/site/trove-status":
            # Multi-env stub: Live in maintenance, PTS online (matches the
            # real-world state we observed). Auth up.
            import time as _t
            return self._send_json({
                "overall": "maintenance",
                "auth": {"online": True, "http_status": 405, "latency_ms": 120.0, "error": None},
                "environments": {
                    "live": {"status": "maintenance", "online": False,
                             "game": {"online": False, "host": "trove-pc-live-us-game-1.trovegame.com", "port": 6560, "latency_ms": 3000.0, "error": "TimeoutError"}},
                    "pts": {"status": "online", "online": True,
                            "game": {"online": True, "host": "trove-pc-pts-us-game-1.trovegame.com", "port": 6560, "latency_ms": 95.0, "error": None}},
                },
                "checked_at": int(_t.time()),
            })
        if path == "/site/trove-status/history":
            # Synthetic 30-day timeline with a couple of outages so the
            # graphic + outage log render in preview.
            import time as _t
            now = int(_t.time()); day = 86400
            qs = parse_qs(url.query); env = (qs.get("env", ["live"])[0])
            start = now - 30 * day
            segs = [
                {"status": "online", "online": True, "started_at": start, "ended_at": now - 9*day, "duration_seconds": 21*day},
                {"status": "maintenance", "online": False, "started_at": now - 9*day, "ended_at": now - 9*day + 7200, "duration_seconds": 7200},
                {"status": "online", "online": True, "started_at": now - 9*day + 7200, "ended_at": now - 2*day, "duration_seconds": 7*day - 7200},
                {"status": "down", "online": False, "started_at": now - 2*day, "ended_at": now - 2*day + 1800, "duration_seconds": 1800},
            ]
            if env == "live":
                segs.append({"status": "maintenance", "online": False, "started_at": now - 4*3600, "ended_at": None, "duration_seconds": 4*3600})
            else:
                segs.append({"status": "online", "online": True, "started_at": now - 2*day + 1800, "ended_at": None, "duration_seconds": 2*day - 1800})
            covered = sum(s["duration_seconds"] for s in segs)
            up = sum(s["duration_seconds"] for s in segs if s["status"] == "online")
            outages = [{"status": s["status"], "started_at": s["started_at"], "ended_at": s["ended_at"], "duration_seconds": s["duration_seconds"]} for s in segs if s["status"] != "online"]
            return self._send_json({
                "env": env, "days": 30, "window_start": start, "window_end": now,
                "uptime": round(up/covered, 5), "covered_seconds": covered,
                "segments": segs, "outages": outages,
            })
        if path == "/site/leaderboards/activity":
            # 1.0h window with a realistic count so the hero pill renders.
            return self._send_json({
                "window_start": STUB_ANCHOR - 3600,
                "window_end": STUB_ANCHOR,
                "duration_hours": 1.0,
                "estimate": 4231,
                "by_board": [
                    {"uuid": 10, "name": "FLUX EARNED",      "category": "STATS", "active_players": 2847},
                    {"uuid": 15, "name": "LOOT COLLECTED",   "category": "STATS", "active_players": 2143},
                    {"uuid": 3,  "name": "ENEMIES DEFEATED", "category": "STATS", "active_players": 1908},
                ],
                "boards_analyzed": 11,
                "methodology": "Distinct top-5000 leaderboard players whose score increased on at least one lifetime-accumulating board between the two most recent captures.",
                "computed_at": 1780890000,
            })
        if path == "/site/leaderboards/cheaters":
            # Three synthetic flagged players spanning the confidence
            # range — so the page's filter slider has something to
            # show + hide as the threshold moves. Confidence values
            # mirror what the real detection module would compute.
            return self._send_json({
                "players": [
                    {
                        "player_name": "Cheater1",
                        "confidence": 0.998,  # 2 strong boards, noisy-OR
                        "leaderboards": [
                            {
                                "uuid": 1012, "name": "GLYPH KICKER",
                                "category": "CONTESTS", "contest_type": "daily",
                                "rank": 1, "score": 99999, "confidence": 0.99,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 99,999 is 42.0 robust z-scores above this board's median of 50,000 (threshold: 3.5). The MAD-based check is resistant to cheaters inflating their own baseline.",
                                        "measurements": {
                                            "player_score": 99999, "peer_median": 50000,
                                            "peer_mad": 1200, "modified_z_score": 42.0,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 5000,
                                        },
                                        "confidence": 0.99,
                                    },
                                    {
                                        "type": "rank_gap",
                                        "summary": "Rank-1 score 99,999 is 27x the typical between-rank gap on this board (49% vs typical 1.8%). Next-rank score: 51,000.",
                                        "measurements": {
                                            "player_rank": 1, "player_score": 99999,
                                            "next_rank": 2, "next_rank_score": 51000,
                                            "gap_fraction": 0.49, "typical_gap_fraction": 0.018,
                                            "gap_multiplier": 27.2,
                                            "threshold_multiplier": 10.0,
                                        },
                                        "confidence": 0.94,
                                    },
                                ],
                            },
                            {
                                "uuid": 20, "name": "GEODE MASTERY POINTS",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 1, "score": 875000, "confidence": 0.99,
                                "evidence": [
                                    {
                                        "type": "velocity_outlier",
                                        "summary": "Score gained 800,000 in 1.0h (rate 800,000/h). This board's peer p95 rate is 500/h — this player is 1,600x faster.",
                                        "measurements": {
                                            "score_delta": 800000, "duration_hours": 1.0,
                                            "rate_per_hour": 800000, "peer_p95_rate_per_hour": 500,
                                            "rate_multiplier": 1600,
                                            "threshold_multiplier": 10.0,
                                            "previous_anchor": 1780743600,
                                            "previous_score": 75000,
                                        },
                                        "confidence": 0.99,
                                    },
                                ],
                            },
                        ],
                    },
                    {
                        "player_name": "noa00__00",
                        "confidence": 0.81,  # one decent board flag
                        "leaderboards": [
                            {
                                "uuid": 2004, "name": "CHALLENGE: Deepest (WEEKLY)",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 1, "score": 9999, "confidence": 0.81,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 9,999 is 7.0 robust z-scores above this board's median of 142 (threshold: 3.5).",
                                        "measurements": {
                                            "player_score": 9999, "peer_median": 142,
                                            "peer_mad": 18, "modified_z_score": 7.0,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 250,
                                        },
                                        "confidence": 0.816,
                                    },
                                ],
                            },
                        ],
                    },
                    {
                        # Borderline player — flagged but right at the
                        # threshold. Confidence 0.5; default filter
                        # (0.9) should hide this one.
                        "player_name": "BorderlineBob",
                        "confidence": 0.5,
                        "leaderboards": [
                            {
                                "uuid": 33001, "name": "HART-A-PHONES RECEIVED",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 4, "score": 800, "confidence": 0.5,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 800 is 3.5 robust z-scores above this board's median of 220 (threshold: 3.5).",
                                        "measurements": {
                                            "player_score": 800, "peer_median": 220,
                                            "peer_mad": 110, "modified_z_score": 3.55,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 80,
                                        },
                                        "confidence": 0.5,
                                    },
                                ],
                            },
                        ],
                    },
                ],
                "computed_at": 1780870000,
                "anchor": STUB_ANCHOR,
                "method": "Three independent statistical checks: Modified Z-score (MAD-based, Iglewicz & Hoaglin 1993), rank-gap ratio, and velocity vs peer p95.",
                "config": {
                    "z_threshold": 3.5,
                    "velocity_multiplier": 10.0,
                    "min_board_size": 20,
                },
                "total_flagged": 3,
                "boards_analyzed": 25,
            })
        if path == "/site/leaderboards/timestamps":
            return self._send_json({"items": STUB_TIMESTAMPS, "count": len(STUB_TIMESTAMPS)})
        if path == "/site/leaderboards/boards":
            qs = parse_qs(url.query)
            created_at = int(qs.get("created_at", [str(STUB_ANCHOR)])[0])
            return self._send_json({
                "created_at": created_at, "items": STUB_BOARDS,
                "count": len(STUB_BOARDS),
            })
        if path.startswith("/site/leaderboards/") and path.endswith("/entries"):
            parts = path.split("/")
            uuid = int(parts[3])
            qs = parse_qs(url.query)
            created_at = int(qs.get("created_at", [str(STUB_ANCHOR)])[0])
            limit = int(qs.get("limit", ["100"])[0])
            offset = int(qs.get("offset", ["0"])[0])
            page = STUB_ENTRIES[offset:offset + limit]
            return self._send_json({
                "uuid": uuid, "created_at": created_at,
                "items": page, "count": len(page), "total": len(STUB_ENTRIES),
            })
        if path.startswith("/site/leaderboards/players/") and path.endswith("/history"):
            from urllib.parse import unquote
            queried = unquote(path.split("/")[4])
            # Mirror the prod case-insensitive match (service.py uses
            # an anchored ``$regex`` with ``i``). Return rows ONLY for
            # the small set of stub players, regardless of input casing.
            known = {e["player_name"].lower(): e["player_name"] for e in STUB_ENTRIES}
            canonical = known.get(queried.lower())
            if canonical is None:
                return self._send_json({
                    "player_name": queried, "items": [], "count": 0,
                })
            return self._send_json({
                "player_name": canonical,
                "items": [
                    {"player_name": canonical, "rank": 1, "score": 59731.0,
                     "leaderboard": 1012, "created_at": STUB_ANCHOR},
                    {"player_name": canonical, "rank": 4, "score": 12500.0,
                     "leaderboard": 20, "created_at": STUB_ANCHOR},
                ],
                "count": 2,
            })

        self.send_error(404)

    def _send_file(self, p: Path, content_type: str | None):
        if not p.exists():
            return self.send_error(404)
        if content_type is None:
            suffix = p.suffix.lower()
            content_type = {
                ".html": "text/html", ".css": "text/css", ".js": "application/javascript",
                ".json": "application/json", ".png": "image/png", ".jpg": "image/jpeg",
                ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp",
                ".svg": "image/svg+xml", ".ico": "image/x-icon",
            }.get(suffix, "application/octet-stream")
        data = p.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _send_json(self, obj):
        data = json.dumps(obj).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        print(f"[{self.address_string()}] {fmt % args}")


if __name__ == "__main__":
    print("[site-dev] http://localhost:8913/leaderboards")
    HTTPServer(("127.0.0.1", 8913), Handler).serve_forever()
