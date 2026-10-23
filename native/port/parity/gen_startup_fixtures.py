"""Golden fixtures for kiwi-server's startup (``app/main.py`` lifespan).

``app.core.database.init_db`` runs three times against an in-memory Mongo that
answers listIndexes/createIndexes/dropIndexes the way mongod 7 does: on a fresh
database, on what that run left, and on a database carrying the legacy indexes
the migrations drop. Every command it sends is recorded in canonical extended
JSON (BSON types kept), with the index catalog after each run. The bootstrap
admin, the supporters and interest-item seeds are captured as the documents
Beanie would insert (``_id`` and dates fixed), and the three Postgres
``pg_schema`` scripts verbatim. The live proof is ``sandbox.sh twin startup``.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_startup_fixtures.py
"""

from __future__ import annotations

import asyncio
import datetime as dt
import json
import logging
import os
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "startup" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))

from bson import ObjectId, json_util  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.son import SON  # noqa: E402
from pymongo import IndexModel  # noqa: E402
from pymongo.errors import OperationFailure  # noqa: E402

import app.core.database as database  # noqa: E402
from app.core.config import settings  # noqa: E402


EPOCH = dt.datetime(2026, 1, 1, tzinfo=dt.timezone.utc)


def canon(v):
    return json.loads(json_util.dumps(v, json_options=CANONICAL_JSON_OPTIONS))


def spec_key(spec: dict) -> list:
    return list(spec["key"].items())


def options(spec: dict) -> dict:
    return {k: v for k, v in spec.items() if k not in ("key", "name", "v")}


class FakeCursor:
    def __init__(self, docs):
        self.docs = list(docs)

    def __aiter__(self):
        return self._gen()

    async def _gen(self):
        for d in self.docs:
            yield d

    async def to_list(self, length=None):
        return list(self.docs)


class FakeCollection:
    def __init__(self, db: "FakeDB", name: str):
        self.database = db
        self.name = name
        self.full_name = f"{db.name}.{name}"

    @property
    def indexes(self) -> dict | None:
        return self.database.catalog.get(self.name)

    def log(self, op, **kw):
        self.database.log.append(canon({"op": op, "collection": self.name, **kw}))

    async def index_information(self, *a, **kw):
        out = {}
        for name, spec in (self.indexes or {}).items():
            info = {k: v for k, v in spec.items() if k not in ("key", "name")}
            info["key"] = spec_key(spec)
            out[name] = info
        self.log("listIndexes")
        return out

    def _create(self, docs: list[dict]) -> None:
        current = dict(self.indexes or {"_id_": SON([("v", 2), ("key", SON([("_id", 1)])), ("name", "_id_")])})
        staged = dict(current)
        for doc in docs:
            spec = SON([("v", 2), ("key", doc["key"]), ("name", doc["name"])])
            for k, v in doc.items():
                if k not in ("key", "name"):
                    spec[k] = v
            same_name = staged.get(doc["name"])
            same_key = [s for s in staged.values() if spec_key(s) == spec_key(spec)]
            if same_name is not None:
                if spec_key(same_name) == spec_key(spec) and options(same_name) == options(spec):
                    continue
                raise OperationFailure(
                    f"An existing index has the same name as the requested index: {doc['name']}", 86)
            if same_key:
                other = same_key[0]
                if options(other) == options(spec):
                    raise OperationFailure(f"Index already exists with a different name: {other['name']}", 85)
                raise OperationFailure(
                    f"An equivalent index already exists with a different name and options: {other['name']}", 85)
            staged[doc["name"]] = spec
        self.database.catalog[self.name] = staged

    async def create_indexes(self, models, *a, **kw):
        docs = [m.document for m in models]
        try:
            self._create(docs)
        except OperationFailure as e:
            self.log("createIndexes", indexes=docs, error=e.code)
            raise
        self.log("createIndexes", indexes=docs)
        return [d["name"] for d in docs]

    async def create_index(self, keys, *a, **kwargs):
        return (await self.create_indexes([IndexModel(keys, **kwargs)]))[0]

    async def drop_index(self, name, *a, **kw):
        if self.indexes is None:
            self.log("dropIndexes", index=name, error=26)
            raise OperationFailure("ns not found", 26)
        if name not in self.indexes or name == "_id_":
            self.log("dropIndexes", index=name, error=27)
            raise OperationFailure(f"index not found with name [{name}]", 27)
        del self.indexes[name]
        self.log("dropIndexes", index=name)

    async def update_many(self, flt, update, *a, **kw):
        self.log("update_many", filter=flt, update=update)

    async def update_one(self, flt, update, *a, **kw):
        self.log("update_one", filter=flt, update=update)

    async def count_documents(self, flt, *a, **kw):
        self.log("count_documents", filter=flt, **kw)
        return 0

    def find(self, flt=None, projection=None, *a, **kw):
        self.log("find", filter=flt or {}, projection=projection)
        return FakeCursor([])

    async def find_one(self, *a, **kw):
        return None

    def aggregate(self, pipeline, *a, **kw):
        self.log("aggregate", pipeline=pipeline)
        return FakeCursor([])

    async def distinct(self, key, *a, **kw):
        self.log("distinct", key=key)
        return []

    def with_options(self, *a, **kw):
        return self


class FakeClient:
    def __init__(self, *a, **kw):
        self.dbs: dict[str, FakeDB] = {}

    def __getitem__(self, name):
        return self.dbs.setdefault(name, FakeDB(self, name))

    def append_metadata(self, *a, **kw):
        pass

    async def close(self):
        pass


class FakeDB:
    def __init__(self, client, name):
        self.client = client
        self.name = name
        self.catalog: dict[str, dict] = {}
        self.log: list = []

    def __getitem__(self, name):
        return FakeCollection(self, name)

    def get_collection(self, name, *a, **kw):
        return FakeCollection(self, name)

    async def command(self, cmd, *a, **kw):
        if "buildInfo" in cmd:
            return {"version": "7.0.28"}
        raise AssertionError(cmd)

    async def list_collection_names(self, *a, **kw):
        return list(self.catalog)


CLIENT = FakeClient()
database.AsyncMongoClient = lambda *a, **kw: CLIENT
settings.mongo_db = "kiwi"


def catalog(db: FakeDB) -> dict:
    return {name: [canon(s) for s in specs.values()] for name, specs in db.catalog.items()}


async def run(pre: dict[str, list[dict]] | None = None) -> dict:
    db = CLIENT["kiwi"]
    if pre is not None:
        db.catalog = {}
        for coll, specs in pre.items():
            db.catalog[coll] = {"_id_": SON([("v", 2), ("key", SON([("_id", 1)])), ("name", "_id_")])}
            for s in specs:
                db.catalog[coll][s["name"]] = SON([("v", 2)] + list(s.items()))
    db.log = []
    await database.init_db()
    return {"commands": db.log, "catalog": catalog(db)}


LEGACY = {
    "mod_projects": [
        SON([("key", SON([("source", 1), ("source_id", 1)])), ("name", "source_1_source_id_1"),
             ("unique", True), ("sparse", True)]),
        SON([("key", SON([("slug", 1)])), ("name", "slug_1"), ("unique", True)]),
    ],
    "mod_releases": [SON([("key", SON([("project_id", 1), ("tag", 1)])), ("name", "project_id_1_tag_1"),
                          ("unique", True)])],
    "site_users": [SON([("key", SON([("email", 1)])), ("name", "email_1"), ("unique", True)])],
    "usage_events": [SON([("key", SON([("created_at", 1)])), ("name", "created_at_ttl"),
                          ("expireAfterSeconds", 86400)])],
}


def seed_docs() -> dict:
    from app.auth.models import User
    from app.supporters.models import Supporter
    from app.supporters.service import DEFAULT_SUPPORTERS
    from app.trove.market.models import MarketInterestItem, load_interest_items_from_file

    from beanie.odm.utils.dump import get_dict

    def beanie_doc(d):
        for name, value in list(d):
            if isinstance(value, dt.datetime):
                setattr(d, name, EPOCH)
        out = SON([("_id", ObjectId("000000000000000000000000"))])
        out.update(get_dict(d, to_db=True))
        return canon(out)

    user = User(email="parity@aallyn.net", hashed_password="$argon2id$x", display_name="Administrator",
                is_superuser=True, is_verified=True)
    return {
        "admin_user": beanie_doc(user),
        "supporters": [beanie_doc(Supporter(name=n, added_by=None)) for n in DEFAULT_SUPPORTERS],
        "interest_items_file": load_interest_items_from_file(),
        "interest_item": beanie_doc(MarketInterestItem(name="Glim")),
    }


def pg_schemas() -> list[str]:
    from app.trove.codexes import pg_schema as codexes
    from app.trove.leaderboards import pg_schema as leaderboards
    from app.trove.market import pg_schema as market

    return [leaderboards._SCHEMA, market._SCHEMA, codexes._SCHEMA]


async def main() -> None:
    models = [m.Settings.name for m in database.DOCUMENT_MODELS]
    fresh = await run({})
    again = await run()
    legacy = await run(LEGACY)
    out = {
        "settings": {
            "usage_retention_days": settings.usage_retention_days,
            "pageview_retention_days": settings.pageview_retention_days,
            "email_outbox_retention_days": settings.email_outbox_retention_days,
        },
        "models": models,
        "fresh": fresh,
        "again": again,
        "legacy_pre": {k: [canon(s) for s in v] for k, v in LEGACY.items()},
        "legacy": legacy,
        "seeds": seed_docs(),
        "pg_schemas": pg_schemas(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(out, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    n = sum(len(v) - 1 for v in fresh["catalog"].values())
    print(f"wrote {OUT.relative_to(ROOT)}: {len(fresh['catalog'])} collections, {n} indexes, "
          f"{len(fresh['commands'])}/{len(again['commands'])}/{len(legacy['commands'])} commands")


asyncio.run(main())
