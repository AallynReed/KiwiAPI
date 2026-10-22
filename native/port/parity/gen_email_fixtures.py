"""Golden fixtures for the email outbox, its template and the daily maintenance
mails, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_email_fixtures.py
Writes native/kiwi-server/testdata/email/golden.json.

Runs the real template, `app/auth/emails.py`, the outbox model and the
maintenance sweeps; Mongo reads are swapped for fixed fakes and every queued
mail is captured instead of stored.
"""
import asyncio
import datetime as dt
import json
import logging
import os
import sys
import types
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

ENV = {
    "APP_NAME": 'Kiwi <API> "é" & co',
    "DEV_URL": "https://dev.example.test/?a=1&b=<2>",
    "DOCS_URL": "https://docs.example.test",
    "SECRET_KEY": "email-fixture-secret",
    "SMTP_HOST": "smtp.example.test",
    "RATE_LIMIT_ALERT_EMAIL": "Alerts@Example.test",
    "TOKEN_INACTIVITY_REVOKE_DAYS": "182",
    "EMAIL_RETRY_BASE_SECONDS": "60",
}
os.environ.update(ENV)

from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS, dumps as bson_dumps  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

import app.auth.emails as emails  # noqa: E402
import app.core.email_outbox as outbox  # noqa: E402
import app.core.maintenance as maint  # noqa: E402
from app.core.email_render import render_email  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "email" / "golden.json"
NOW = dt.datetime(2026, 10, 22, 11, 0, 0, 123456, tzinfo=dt.timezone.utc)


def ext(v):
    return json.loads(bson_dumps(v, json_options=CANONICAL_JSON_OPTIONS))


RENDER_CASES = [
    ("plain", "Hello", ["One paragraph."], None, None),
    ("empty", "", [], None, None),
    ("button_note", 'Head <&> "q"', ["p1 'single' & <b>", "p2 ✓ 🐉"],
     {"label": "Go <now>", "url": "https://x.test/?a=1&b=\"2\""}, "Note & <stuff>"),
    ("empty_note", "H", ["x"], {"label": "L", "url": "u"}, ""),
]


def render_cases():
    out = []
    for name, heading, paragraphs, button, note in RENDER_CASES:
        out.append({"name": name, "heading": heading, "paragraphs": paragraphs, "button": button,
                    "note": note, "html": render_email(heading, paragraphs, button=button, note=note)})
    return out


QUEUED = []


async def _capture(to, subject, text, html=None):
    QUEUED.append({"to": to, "subject": subject, "text": text, "html": html})


def notices():
    emails.queue_email = _capture
    user = types.SimpleNamespace(email="owner@example.test")
    out = []
    for days in (-1, 0, 1, 6):
        QUEUED.clear()
        asyncio.run(emails.send_token_expiring_email(user, 'My "bot" <token>', "kiwi_ab12cd34", days))
        out.append({"days_left": days, "token_name": 'My "bot" <token>', "prefix": "kiwi_ab12cd34",
                    **QUEUED[0]})
    return out


class FakeQuery:
    def __init__(self, rows):
        self.rows = rows

    async def to_list(self):
        return self.rows


def digest_case(hours, threshold, rows, users, tokens, last):
    QUEUED.clear()
    maint.queue_email = _capture
    maint.utcnow = lambda: NOW
    state = {}

    async def get_state(key):
        return last

    async def set_state(key, value):
        state[key] = value

    maint._get_state_time = get_state
    maint._set_state_time = set_state
    pipelines = []

    def aggregate(pipeline):
        pipelines.append(pipeline)
        return FakeQuery(rows)

    maint.UsageEvent.aggregate = staticmethod(aggregate)
    maint.User.find = staticmethod(lambda q: FakeQuery([types.SimpleNamespace(id=k, email=v) for k, v in users.items()]))
    maint.ApiToken.find = staticmethod(lambda q: FakeQuery([types.SimpleNamespace(id=k, name=n, prefix=p)
                                                            for k, (n, p) in tokens.items()]))
    rc._CACHE["rate_limit_digest_window_hours"] = (hours, float("inf"))
    rc._CACHE["rate_limit_alert_threshold"] = (threshold, float("inf"))
    sent = asyncio.run(maint.daily_rate_limit_digest())
    return {
        "hours": hours, "threshold": threshold, "last": last.isoformat() if last else None,
        "rows": [{"_id": str(r["_id"]) if r["_id"] else None, "count": r["count"],
                  "user_id": str(r["user_id"]) if r["user_id"] else None} for r in rows],
        "users": {str(k): v for k, v in users.items()},
        "tokens": {str(k): list(v) for k, v in tokens.items()},
        "sent": sent, "marked": "rate_limit_digest" in state,
        "pipeline": ext(pipelines[0]) if pipelines else None,
        "mail": QUEUED[0] if QUEUED else None,
    }


U1, U2 = ObjectId("65a000000000000000000001"), ObjectId("65a000000000000000000002")
T1, T2, T3 = (ObjectId("65b000000000000000000001"), ObjectId("65b000000000000000000002"),
              ObjectId("65b000000000000000000003"))


def digests():
    rows = [
        {"_id": T1, "count": 1234567, "user_id": U1},
        {"_id": T2, "count": 15, "user_id": U2},
        {"_id": T3, "count": 3, "user_id": U1},
        {"_id": None, "count": 2, "user_id": None},
    ]
    users = {U1: "a&b@example.test", U2: "<x>@example.test"}
    tokens = {T1: ('Bot "one" & <two>', "kiwi_aaaa'1"), T2: ("Second", "kiwi_bbbb")}
    return [
        digest_case(24, 20, rows, users, tokens, None),
        digest_case(24, 2_000_000, rows, users, tokens, None),
        digest_case(6, 1, rows[:1], users, tokens, NOW - dt.timedelta(hours=5, minutes=24)),
        digest_case(6, 1, rows[:1], users, tokens, NOW - dt.timedelta(hours=5, minutes=23, seconds=59)),
        digest_case(1, 0, [], {}, {}, None),
    ]


def outbox_doc():
    from beanie import init_beanie
    from beanie.odm.utils.dump import get_dict
    from pymongo import AsyncMongoClient
    from pymongo.asynchronous.database import AsyncDatabase

    async def _cmd(self, *a, **k):
        return {"version": "7.0.0"}

    async def _lcn(self, *a, **k):
        return []

    AsyncDatabase.command = _cmd
    AsyncDatabase.list_collection_names = _lcn

    async def init():
        client = AsyncMongoClient("mongodb://127.0.0.1:1", connect=False, serverSelectionTimeoutMS=200)
        await init_beanie(database=client["kiwi"], skip_indexes=True, document_models=[outbox.OutboxEmail])

    asyncio.run(init())
    msg = outbox.OutboxEmail(to="x@example.test", subject="S", text="T")
    return list(get_dict(msg, to_db=True).keys())


def backoffs():
    return [[n, outbox._backoff(n).total_seconds()] for n in range(1, 6)]


def times09():
    hours = [1, 6, 24, 168, 720]
    return [[h * 3600 * 10**6, int((dt.timedelta(hours=h) * 0.9) / dt.timedelta(microseconds=1))] for h in hours]


def main():
    data = {
        "env": ENV,
        "render": render_cases(),
        "notices": notices(),
        "digests": digests(),
        "outbox_keys": outbox_doc(),
        "backoff": backoffs(),
        "times09": times09(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
