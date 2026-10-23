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


async def call(app, method: str, url: str, headers: list, body: bytes):
    import urllib.parse
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers + [(b"content-length", str(len(body)).encode())],
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": body, "more_body": False}]
    sent = []

    async def receive():
        if pending:
            return pending.pop(0)
        await done.wait()
        return {"type": "http.disconnect"}

    async def send(message):
        sent.append(message)
        if message["type"] == "http.response.body" and not message.get("more_body"):
            done.set()

    await app(scope, receive, send)
    start = next(m for m in sent if m["type"] == "http.response.start")
    raw = b"".join(m.get("body", b"") for m in sent if m["type"] == "http.response.body")
    return start["status"], raw


B = "XyZkiwi"
MP = f"multipart/form-data; boundary={B}"


def multipart(parts) -> bytes:
    out = b""
    for name, filename, data in parts:
        disp = f'Content-Disposition: form-data; name="{name}"' + (f'; filename="{filename}"' if filename else "")
        out += f"--{B}\r\n{disp}\r\n\r\n".encode() + data + b"\r\n"
    return out + f"--{B}--\r\n".encode()


JSON = "application/json"
CAPTURE_BODIES = [
    (JSON, b'{"name": 5}'), (JSON, b"{}"), (JSON, b"[]"), (JSON, b'"x"'), (JSON, b"null"), (None, b""),
    ("text/plain", b"name=x"), (JSON, b"{bad"), (JSON, b'{"name": " none "}'), (JSON, b'{"name": "\\u3000"}'),
    (JSON, b'{"name": null}'), ("application/vnd.x+json", b'{"name": ["a"]}'), (None, b'{"name": "x"}'),
]
LUXION_BODIES = [
    (JSON, b'{"active": "maybe"}'), (JSON, b'{"active": null}'), (JSON, b"[]"), (JSON, b'"x"'),
    (JSON, b'{"active": 2}'), ("text/plain", b"x"), (JSON, b"{bad"), (JSON, b'{"active": 1.5}'),
]
UPLOADS = [
    ("", MP, multipart([("dump", None, b"x")])),
    ("", MP, multipart([("file", None, b"x")])),
    ("", MP, multipart([("file", None, b"")])),
    ("", MP, multipart([("file", "a.cfg", b"")])),
    ("?timestamp=abc", MP, multipart([("file", "a.cfg", b"x")])),
    ("?timestamp=1.5&timestamp=2", MP, multipart([("file", None, b"x")])),
    ("?timestamp=", MP, multipart([("file", "a.cfg", b"")])),
    ("", MP, b"garbage"),
    ("", "multipart/form-data", b"--x\r\n"),
    ("", JSON, b'{"file": "x"}'),
]


async def http_cases() -> dict:
    import types

    from app.core.dependencies import IngestAuth, require_master_ingest
    from app.core.features import require_market_enabled, require_store_enabled
    from app.main import app
    from app.trove import router as trove_router

    user = types.SimpleNamespace(id=ObjectId("65f0c0ffee0000000000abcd"), email="m@x", is_superuser=True)
    app.dependency_overrides[require_master_ingest] = lambda: IngestAuth(user=user, token=None)
    app.dependency_overrides[require_market_enabled] = lambda: None
    app.dependency_overrides[require_store_enabled] = lambda: None

    async def nothing(*a, **k):
        return None

    async def unreachable(*a, **k):
        raise AssertionError("only invalid requests are replayed")

    trove_router.ingest_log.record = nothing
    for name in ("publish_challenge", "publish_chaos", "publish_luxion"):
        setattr(trove_router.events_bus, name, nothing)
    for name in ("insert_challenge", "insert_chaos_chest", "insert_luxion"):
        setattr(trove_router.captures, name, unreachable)

    async def challenge(name, **k):
        name = name.strip()
        if not name or name.lower() == "none":
            raise ValueError("challenge name is empty or 'none'")
        raise AssertionError("valid")

    async def chaos(name, **k):
        if not name.strip():
            raise ValueError("name is empty")
        raise AssertionError("valid")

    trove_router.captures.insert_challenge = challenge
    trove_router.captures.insert_chaos_chest = chaos
    trove_router.market_service.insert_dump = unreachable
    trove_router.store_service.insert_dump = unreachable

    out: dict = {"capture": [], "luxion": [], "upload": []}
    n = 0

    async def go(section, url, ctype, body, extra=None):
        nonlocal n
        n += 1
        rid = f"parity-prod-{n:03d}"
        headers = [(b"x-request-id", rid.encode())] + ([(b"content-type", ctype.encode())] if ctype else [])
        try:
            status, raw = await call(app, "POST", url, headers, body)
        except AssertionError:
            return
        if status == 500:
            return
        out[section].append({"url": url, "content_type": ctype, "body": body.decode("latin-1"), "request_id": rid,
                             "status": status, "response": raw.decode("utf-8"), **(extra or {})})

    for path in ("/v1/rotations/challenge/insert", "/v1/rotations/chaos-chest/insert"):
        for ctype, body in CAPTURE_BODIES:
            await go("capture", path, ctype, body)
    for ctype, body in LUXION_BODIES:
        await go("luxion", "/v1/rotations/luxion/insert", ctype, body)
    for path in ("/v1/market/insert", "/v1/store/insert"):
        for query, ctype, body in UPLOADS:
            await go("upload", path + query, ctype, body)
    app.dependency_overrides.clear()
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
        "http": asyncio.run(http_cases()),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
