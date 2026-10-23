"""Golden fixtures for the event producers port (capture inserts, market and
store inserts, the news upsert), from the repo root:

    TZ=UTC0 S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_producers_fixtures.py
        -> native/kiwi-server/testdata/producers/golden.json

Documents are the real Beanie models encoded the way save()/insert() send
them (get_dict(to_db=True)), as canonical extended JSON so int32/int64/double
and key order are kept. The market ingest runs the real service with the
Postgres upsert, the interest list, the clock and the DM check swapped for
recorders.
"""

from __future__ import annotations

import asyncio
import json
import logging
import os
import sys
import warnings
from datetime import UTC, datetime, timedelta
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "producers-fixture-secret")

from bson import ObjectId, json_util  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from beanie.odm.settings.document import DocumentSettings  # noqa: E402
from beanie.odm.utils.dump import get_dict  # noqa: E402

from app.admin.ingest_log import IngestLogEntry  # noqa: E402
from app.trove import luxion, server_time  # noqa: E402
from app.trove.market import service as market_service  # noqa: E402
from app.trove.models import ChallengeCapture, ChaosChestCapture, LuxionAppearance, TroveNews  # noqa: E402

OUT = ROOT / "native" / "kiwi-server" / "testdata" / "producers" / "golden.json"
T0 = datetime(2026, 10, 22, 10, 59, 59, 999_999, tzinfo=UTC)

for model in (ChallengeCapture, ChaosChestCapture, LuxionAppearance, TroveNews, IngestLogEntry):
    model._document_settings = DocumentSettings(name=model.__name__)


def ext(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def encoded(doc) -> dict:
    return ext(get_dict(doc, to_db=True))


def daily_resets() -> list:
    instants = [T0, T0 + timedelta(microseconds=1), T0 + timedelta(hours=13), datetime(2026, 1, 1, tzinfo=UTC),
                datetime(1969, 12, 31, 11, 0, tzinfo=UTC), datetime(1969, 12, 31, 10, 59, tzinfo=UTC)]
    return [[int(t.timestamp() * 1_000_000), server_time.current_daily_reset(t)] for t in instants]


def luxion_live() -> list:
    out = []
    first = datetime(2026, 10, 20, 12, 30, 15, 250_000, tzinfo=UTC)
    started = 1_760_958_000
    for ended in (None, first + timedelta(days=1)):
        for now_ts in (started, started + 7 * 86400 - 1, started + 7 * 86400, started - 10):
            doc = LuxionAppearance(started_at=started, first_seen_at=first, last_seen_at=first, ended_at=ended)
            live = doc.ended_at is None and now_ts < luxion.run_bounds(doc.started_at, doc.first_seen_at)[1]
            out.append({"started_at": started, "first_seen_ms": int(first.timestamp() * 1000),
                        "ended": ended is not None, "now": now_ts, "live": live})
    return out


def documents() -> dict:
    at = datetime(2026, 10, 22, 11, 0, 0, 123_456, tzinfo=UTC)
    oid = ObjectId("65f0c0ffee0000000000abcd")
    news = TroveNews(url="https://trovegame.com/x", title="T", author="Team Trove", summary="S", category="News",
                     categories=["News", "Events"], image=None, published_at=at, first_seen_at=at, updated_at=at)
    return {
        "chaos": encoded(ChaosChestCapture(week_anchor=1_792_576_800, name="Chest", captured_at=at)),
        "chaos_big": encoded(ChaosChestCapture(week_anchor=3_000_000_000, name="Chest", captured_at=at)),
        "challenge": encoded(ChallengeCapture(window_anchor=1_792_576_800, window_ends_at=1_792_578_000,
                                              name="Cursed Vale", is_friday_window=True, captured_at=at)),
        "luxion_open": encoded(LuxionAppearance(started_at=1_792_576_800, first_seen_at=at, last_seen_at=at)),
        "luxion_closed": encoded(LuxionAppearance(id=oid, started_at=1_792_576_800, first_seen_at=at,
                                                  last_seen_at=at, ended_at=at)),
        "news": encoded(news),
        "log_market": ext(IngestLogEntry(endpoint="e", timestamp=at, user_id=oid, user_email="m@x", success=True,
                                         summary={"parsed": 0, "imported": 0, "ignored_not_in_list": 0,
                                                  "last_seen": None, "bytes": 12}).summary),
        "log_luxion": ext(IngestLogEntry(endpoint="e", timestamp=at, user_id=oid, user_email="m@x", success=True,
                                         summary={"started_at": 0, "active": False, "refreshed": True}).summary),
    }


DUMP = "\n".join([
    "bad line",
    "6f1c1b2e-3c4d-11ef-9a7b-0242ac120002;Glim;;999;1500000",
    "6f1c1b2e-3c4d-11ef-9a7b-0242ac120002;Glim;;1;1",
    "7a2d2c3f-3c4d-11ef-9a7b-0242ac120002; Plasmium ;Material;3;10",
    "8b3e3d4a-3c4d-11ef-9a7b-0242ac120002;Glim;;7;100",
    "9c4f4e5b-3c4d-11ef-9a7b-0242ac120002;Untracked;;1;5",
    "ad505f6c-3c4d-11ef-9a7b-0242ac120002;Plasmium;;2;9",
    "be616a7d-3c4d-11ef-9a7b-0242ac120002;Glim;;0;100",
    "cf727b8e-3c4d-11ef-9a7b-0242ac120002;Glim;;5;60000000",
])


def market() -> list:
    out = []
    calls: dict = {}

    async def upsert(rows):
        calls["rows"] = [[str(r[0]), r[1], r[2], r[3], r[4], r[5], r[6], r[7]] for r in rows]
        return len(rows)

    async def interest():
        return frozenset({"Glim", "Plasmium"})

    async def check_market(cheapest):
        calls["cheapest"] = [[k, v] for k, v in cheapest.items()]

    from app.dm_subs import delivery as dm
    market_service.pg_store.upsert_listings = upsert
    market_service._interest_items_set = interest
    market_service._now = lambda: 1_792_576_800
    dm.check_market = check_market
    for ts in (None, 0, -5, 1_700_000_000, 1_792_577_100, 1_792_577_101):
        for text in (DUMP, "nothing here"):
            calls.clear()
            summary = asyncio.run(market_service.insert_dump(text, timestamp=ts))
            out.append({"timestamp": ts, "text": text, "summary": summary,
                        "rows": calls.get("rows"), "cheapest": calls.get("cheapest")})
    return out


def main() -> None:
    probe = datetime(2026, 1, 1)
    assert probe.astimezone(UTC).replace(tzinfo=None) == probe, "run with TZ=UTC0 (the container's zone)"
    data = {
        "python": sys.version.split()[0],
        "daily_reset": daily_resets(),
        "luxion_live": luxion_live(),
        "documents": documents(),
        "market": market(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
