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
# last 7 days so two slots render as "No data" - exercising the empty
# state. Each anchor is real UTC 11:00 of its trove-day (matches the
# current API's daily-anchor model).
import time as _time
_DAY = 86400
_TROVE_OFFSET = 11 * 3600
_now = int(_time.time())
_today_trove_key = (_now - _TROVE_OFFSET) // _DAY
# Days 0 (today), 1 (yesterday), 2, 4, 6 ago - skip 3 and 5 so the
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

# The 18 Trove classes in classes.json order (= Effort/Paragon board offset
# order), for the Class Activity page stubs.
_STUB_CLASSES = [
    "Bard", "Boomeranger", "Candy Barbarian", "Chloromancer", "Dino Tamer",
    "Dracolyte", "Fae Trickster", "Gunslinger", "Ice Sage", "Knight",
    "Lunar Lancer", "Neon Ninja", "Pirate Captain", "Revenant", "Shadow Hunter",
    "Solarion", "Tomb Raiser", "Vanguardian",
]
# qualified_name per class (classes.json order) → self-hosted icon path.
_STUB_CLASS_QN = [
    "bard", "adventurer", "candybarbarian", "chloromancer", "dinotamer",
    "dracolyte", "faetrickster", "gunslinger", "icemage", "knight",
    "lunarlancer", "neonninja", "piratelord", "spirittank", "shadowhunter",
    "solarion", "tombraiser", "crimefighter",
]
def _stub_icon(ci):
    return f"/static/class-icons/{_STUB_CLASS_QN[ci]}.png"


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
        if path == "/giveaways":
            return self._send_file(TEMPLATES / "giveaways.html", "text/html")
        if path == "/clubs":
            return self._send_file(TEMPLATES / "clubs.html", "text/html")
        if path == "/activity":
            return self._send_file(TEMPLATES / "activity.html", "text/html")
        if path == "/market":
            return self._send_file(TEMPLATES / "market.html", "text/html")
        if path == "/support":
            return self._send_file(TEMPLATES / "support.html", "text/html")
        if path == "/documentation":
            return self._send_file(TEMPLATES / "docs.html", "text/html")
        if path == "/terms":
            return self._send_file(TEMPLATES / "terms.html", "text/html")
        if path == "/privacy":
            return self._send_file(TEMPLATES / "privacy.html", "text/html")
        if path == "/class-activity":
            return self._send_file(TEMPLATES / "class-activity.html", "text/html")

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
            # Multi-env stub (eu/us/pts, binary online/down): EU down, US + PTS
            # online - a partial outage, matching the real-world state observed.
            import time as _t
            return self._send_json({
                "overall": "down",
                "auth": {"online": True, "http_status": 405, "latency_ms": 120.0, "error": None},
                "environments": {
                    "eu": {"status": "down", "online": False,
                           "game": {"online": False, "host": "ams-c12-b05.ams.triongames.com", "port": 6560, "latency_ms": 3000.0, "error": "glsserver_dropped"}},
                    "us": {"status": "online", "online": True,
                           "game": {"online": True, "host": "dal-c35-b05.dal.triongames.com", "port": 6560, "latency_ms": 88.0, "error": None}},
                    "pts": {"status": "online", "online": True,
                            "game": {"online": True, "host": "auth-pcpts01.trovegame.com", "port": 6560, "latency_ms": 95.0, "error": None}},
                },
                "checked_at": int(_t.time()),
            })
        if path == "/site/trove-status/history":
            # Synthetic 7-day timeline with a couple of outages so the
            # graphic + outage log render in preview.
            import time as _t
            now = int(_t.time()); day = 86400
            qs = parse_qs(url.query); env = (qs.get("env", ["live"])[0])
            start = now - 7 * day
            segs = [
                {"status": "online", "online": True, "started_at": start, "ended_at": now - 3*day, "duration_seconds": 4*day},
                {"status": "down", "online": False, "started_at": now - 3*day, "ended_at": now - 3*day + 5400, "duration_seconds": 5400},
                {"status": "online", "online": True, "started_at": now - 3*day + 5400, "ended_at": now - 18*3600, "duration_seconds": 3*day - 5400 - 18*3600},
            ]
            if env == "live":
                segs.append({"status": "down", "online": False, "started_at": now - 18*3600, "ended_at": None, "duration_seconds": 18*3600})
            else:
                segs.append({"status": "online", "online": True, "started_at": now - 18*3600, "ended_at": None, "duration_seconds": 18*3600})
            covered = sum(s["duration_seconds"] for s in segs)
            up = sum(s["duration_seconds"] for s in segs if s["status"] == "online")
            outages = [{"status": s["status"], "started_at": s["started_at"], "ended_at": s["ended_at"], "duration_seconds": s["duration_seconds"]} for s in segs if s["status"] != "online"]
            return self._send_json({
                "env": env, "days": 7, "window_start": start, "window_end": now,
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
                "estimate_24h": 18764,
                "estimate_7d": 52310,
                "window_24h_start": STUB_ANCHOR - 86400,
                "window_7d_start": STUB_ANCHOR - 7 * 86400,
                "span_24h_hours": 24.0,
                "span_7d_hours": 168.0,
                "by_board": [
                    {"uuid": 10, "name": "FLUX EARNED",      "category": "STATS", "active_players": 2847},
                    {"uuid": 15, "name": "LOOT COLLECTED",   "category": "STATS", "active_players": 2143},
                    {"uuid": 3,  "name": "ENEMIES DEFEATED", "category": "STATS", "active_players": 1908},
                ],
                "boards_analyzed": 11,
                "methodology": "Distinct top-5000 leaderboard players whose score increased on at least one lifetime-accumulating board between the two most recent captures.",
                "computed_at": 1780890000,
            })
        if path == "/site/leaderboards/activity/series":
            # Synthetic bucketed series so the /activity charts render. The
            # period sets bucket size + count; a diurnal sine + slow drift
            # gives a believable shape with a clear peak / quiet trough.
            import math
            qs = parse_qs(url.query)
            period = (qs.get("period", ["7d"])[0]).lower()
            spec = {
                "1d":  (3600, 24), "7d": (3 * 3600, 56), "1m": (86400, 30),
                "3m":  (86400, 90), "6m": (2 * 86400, 90), "1y": (7 * 86400, 52),
                "all": (7 * 86400, 80),
            }.get(period, (3 * 3600, 56))
            bucket, count = spec
            end = STUB_ANCHOR
            start = end - bucket * count
            points = []
            for i in range(count):
                ti = start + i * bucket
                base = 3500 + 1500 * math.sin(i / 3.0) + (i * 12)
                active = max(180, round(base))
                points.append({"t": ti, "active": float(active),
                               "peak": float(active + 420), "samples": 1})
            peak = max(points, key=lambda p: p["active"])
            avg = round(sum(p["active"] for p in points) / len(points), 1)
            return self._send_json({
                "period": period, "bucket_seconds": bucket,
                "window_start": start, "window_end": end,
                "points": points,
                "peak": {"t": peak["t"], "active": peak["active"]},
                "average": avg, "latest": points[-1]["active"],
                "methodology": "stub series",
            })
        if path == "/site/leaderboards/class-activity/series":
            # Synthetic per-class multi-line series: shared buckets + one values[]
            # per class (a diurnal sine, phase-shifted per class so the lines
            # spread out), with occasional nulls to mimic the weekly-reset gap.
            import math
            qs = parse_qs(url.query)
            period = (qs.get("period", ["7d"])[0]).lower()
            spec = {
                "1d":  (3600, 24), "7d": (3 * 3600, 56), "1m": (86400, 30),
                "3m":  (86400, 90), "6m": (2 * 86400, 90), "1y": (7 * 86400, 52),
                "all": (7 * 86400, 80),
            }.get(period, (3 * 3600, 56))
            bucket, count = spec
            end = STUB_ANCHOR
            start = end - bucket * count
            buckets = [start + i * bucket for i in range(count)]
            classes = []
            for ci, name in enumerate(_STUB_CLASSES):
                vals = []
                for i in range(count):
                    base = 120 + 70 * math.sin(i / 3.0 + ci * 0.5) + (18 - ci) * 6
                    # a couple of synthetic gaps (weekly reset look)
                    vals.append(None if (i % 28 == 13) else max(2.0, round(base, 1)))
                classes.append({"class_index": ci, "name": name, "icon": _stub_icon(ci), "values": vals})
            return self._send_json({
                "period": period, "bucket_seconds": bucket,
                "window_start": start, "window_end": end,
                "buckets": buckets, "classes": classes,
                "methodology": "stub class series",
            })
        if path == "/site/leaderboards/class-activity/current":
            # Synthetic per-class counts + sum-normalized share for the donut.
            counts = [max(3, 320 - ci * 15 - (ci % 3) * 8) for ci in range(len(_STUB_CLASSES))]
            total = sum(counts)
            classes = [
                {"class_index": ci, "name": _STUB_CLASSES[ci], "icon": _stub_icon(ci),
                 "active_players": counts[ci], "share": round(counts[ci] / total, 4)}
                for ci in range(len(_STUB_CLASSES))
            ]
            classes.sort(key=lambda c: -c["active_players"])
            return self._send_json({
                "window_start": STUB_ANCHOR - 3600, "window_end": STUB_ANCHOR,
                "duration_hours": 1.0, "total_active": total, "classes": classes,
                "methodology": "stub class current", "computed_at": STUB_ANCHOR,
            })
        if path == "/site/leaderboards/cheaters":
            # Three synthetic flagged players spanning the confidence
            # range - so the page's filter slider has something to
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
                                        "summary": "Score gained 800,000 in 1.0h (rate 800,000/h). This board's peer p95 rate is 500/h - this player is 1,600x faster.",
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
                        # Borderline player - flagged but right at the
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
        if path == "/site/giveaways":
            import time as _t
            from datetime import datetime, timezone
            def _iso(off):
                return datetime.fromtimestamp(int(_t.time()) + off, tz=timezone.utc).isoformat()
            return self._send_json({"items": [
                {"id": "1", "title": "Holiday code drop", "description": "A shiny Trove mount code for one lucky winner.",
                 "prize_name": "Trove Mount Code", "status": "open",
                 "starts_at": _iso(-86400), "ends_at": _iso(3 * 86400), "entry_count": 42, "winner_username": None},
                {"id": "2", "title": "Next week's giveaway", "description": "Coming soon - a mystery Trove prize.",
                 "prize_name": "Mystery Prize", "status": "scheduled",
                 "starts_at": _iso(2 * 86400), "ends_at": _iso(5 * 86400), "entry_count": 0, "winner_username": None},
                {"id": "3", "title": "Last month's draw", "description": "Already drawn + the code is on its way.",
                 "prize_name": "Glim Pack", "status": "drawn",
                 "starts_at": _iso(-10 * 86400), "ends_at": _iso(-3 * 86400), "entry_count": 118, "winner_username": "Aallyn"},
                {"id": "4", "title": "Quiet one", "description": None,
                 "prize_name": "Costume Code", "status": "closed",
                 "starts_at": _iso(-20 * 86400), "ends_at": _iso(-15 * 86400), "entry_count": 0, "winner_username": None},
            ]})
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
