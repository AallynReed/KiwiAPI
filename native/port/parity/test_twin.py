"""Tests for run.py and statediff.py: python native/port/parity/test_twin.py

Local HTTP servers stand in for the twin's two stacks; small fake Mongo
(OP_MSG), Postgres (simple query) and Redis (RESP2) servers stand in for the
stores. The BSON checks compare against pymongo's encoder when it is installed.
"""

import contextlib
import fnmatch
import io
import json
import os
import socketserver
import struct
import sys
import tempfile
import threading
import unittest
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))

import run  # noqa: E402
import statediff  # noqa: E402

try:
    import bson
    from bson import Decimal128, Int64, MaxKey, MinKey, ObjectId, Regex, Timestamp
    from bson.binary import Binary
except ImportError:  # pragma: no cover
    bson = None


def serve(server_cls, handler):
    srv = server_cls(("127.0.0.1", 0), handler)
    srv.daemon_threads = True
    threading.Thread(target=srv.serve_forever, daemon=True).start()
    return srv


def quiet(fn, *args):
    out = io.StringIO()
    with contextlib.redirect_stdout(out), contextlib.redirect_stderr(out):
        rc = fn(*args)
    return rc, out.getvalue()


# ------------------------------------------------------------------- run.py

def make_app(label):
    seen = []

    class Handler(BaseHTTPRequestHandler):
        def log_message(self, *a):
            pass

        def _reply(self):
            body = self.rfile.read(int(self.headers.get("Content-Length") or 0))
            seen.append({"method": self.command, "path": self.path, "headers": dict(self.headers), "body": body})
            if self.path.startswith("/items/"):
                out = {"got": self.path.rsplit("/", 1)[1].startswith(label)}
            elif self.path == "/differ":
                out = {"who": label}
            elif self.path == "/nothing":
                out = {}
            else:
                out = {"id": f"{label}-1", "same": True}
            data = json.dumps(out).encode()
            self.send_response(201 if self.command == "POST" else 200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(data)))
            self.end_headers()
            self.wfile.write(data)

        do_GET = do_POST = do_DELETE = _reply

    return Handler, seen


class RunTests(unittest.TestCase):
    def setUp(self):
        ha, self.seen_a = make_app("A")
        hb, self.seen_b = make_app("B")
        self.srv_a = serve(ThreadingHTTPServer, ha)
        self.srv_b = serve(ThreadingHTTPServer, hb)
        self.tmp = tempfile.TemporaryDirectory()
        self.dir = self.tmp.name

    def tearDown(self):
        for s in (self.srv_a, self.srv_b):
            s.shutdown()
            s.server_close()
        self.tmp.cleanup()

    def write(self, name, content):
        path = os.path.join(self.dir, name)
        os.makedirs(os.path.dirname(path), exist_ok=True)
        with open(path, "w" if isinstance(content, str) else "wb") as f:
            f.write(content)
        return path

    def run_spec(self, specs, *extra):
        spec = self.write("area.writes.json", json.dumps(specs))
        base_a = f"http://127.0.0.1:{self.srv_a.server_address[1]}"
        base_b = f"http://127.0.0.1:{self.srv_b.server_address[1]}"
        return quiet(run.main, [spec, base_a, base_b, "--allow-writes", *extra])

    def test_env_file_parsing(self):
        path = self.write("a.env", "# c\nexport TOKEN=abc=def\nJWT='x y'\nbad line\nexport  Q=\"q\"\n")
        self.assertEqual(run.load_env_file(path), {"TOKEN": "abc=def", "JWT": "x y", "Q": "q"})

    def test_per_target_env_capture_body_file_and_default_headers(self):
        env_a = self.write("a.env", "export TOKEN=tok-a\n")
        env_b = self.write("b.env", "export TOKEN=tok-b\n")
        self.write("data/dump.bin", b"\x1f\x8braw\x00bytes")
        specs = [
            {"name": "create", "method": "POST", "path": "/things", "body": {"x": "${TOKEN}"},
             "headers": {"Authorization": "Bearer ${TOKEN}"}, "mask": ["id"], "capture": {"ID": "id"}},
            {"name": "use", "path": "/items/${ID}", "headers": {"X-Forwarded-For": "10.0.0.9"}},
            {"name": "upload", "method": "POST", "path": "/up", "body_file": "data/dump.bin",
             "headers": {"Content-Type": "application/octet-stream"}, "mask": ["id"]},
        ]
        rc, out = self.run_spec(specs, "--env-a", env_a, "--env-b", env_b,
                                "--header", "X-Forwarded-For: 203.0.113.7", "--header", "Host: kiwi-parity")
        self.assertEqual(rc, 0, out)
        a, b = self.seen_a, self.seen_b
        self.assertEqual(a[0]["headers"]["Authorization"], "Bearer tok-a")
        self.assertEqual(b[0]["headers"]["Authorization"], "Bearer tok-b")
        self.assertEqual(json.loads(a[0]["body"]), {"x": "tok-a"})
        self.assertEqual(json.loads(b[0]["body"]), {"x": "tok-b"})
        self.assertEqual(a[0]["headers"]["X-Forwarded-For"], "203.0.113.7")
        self.assertEqual(a[0]["headers"]["Host"], "kiwi-parity")
        self.assertEqual((a[1]["path"], b[1]["path"]), ("/items/A-1", "/items/B-1"))
        self.assertEqual(a[1]["headers"]["X-Forwarded-For"], "10.0.0.9")
        self.assertEqual(a[2]["body"], b"\x1f\x8braw\x00bytes")
        self.assertEqual(b[2]["body"], b"\x1f\x8braw\x00bytes")
        self.assertEqual(a[2]["headers"]["Content-Type"], "application/octet-stream")

    def test_environment_fallback_without_env_files(self):
        os.environ["PARITY_TEST_TOKEN"] = "from-env"
        try:
            rc, out = self.run_spec([{"path": "/nothing", "headers": {"Authorization": "Bearer ${PARITY_TEST_TOKEN}"}}])
        finally:
            del os.environ["PARITY_TEST_TOKEN"]
        self.assertEqual(rc, 0, out)
        self.assertEqual(self.seen_a[0]["headers"]["Authorization"], "Bearer from-env")
        self.assertEqual(self.seen_b[0]["headers"]["Authorization"], "Bearer from-env")

    def test_differences_and_missing_capture(self):
        rc, out = self.run_spec([{"name": "d", "path": "/differ"}])
        self.assertEqual(rc, 1)
        self.assertIn("$.who", out)
        rc, out = self.run_spec([{"name": "c", "path": "/nothing", "capture": {"X": "id"}}])
        self.assertEqual(rc, 1)
        self.assertIn("capture X: no id in A's reply", out)
        rc, out = self.run_spec([{"name": "c", "path": "/nothing", "capture": {"X": "id"},
                                  "allow_diff": ["capture"], "reason": "test"}])
        self.assertEqual(rc, 0, out)

    def test_missing_body_file_is_usage_error(self):
        rc, out = self.run_spec([{"method": "POST", "path": "/x", "body_file": "nope.bin"}])
        self.assertEqual(rc, 2)
        self.assertIn("body_file not found", out)


# ------------------------------------------------------------------- BSON

@unittest.skipIf(bson is None, "pymongo not installed")
class BsonTests(unittest.TestCase):
    def test_decode_matches_pymongo_encoding(self):
        import datetime as dt
        doc = {
            "_id": ObjectId("65a0000000000000000000b5"), "i32": 7, "i64": Int64(7), "big": 2**40,
            "dbl": 7.0, "s": "héllo", "none": None, "t": True, "f": False,
            "d": dt.datetime(2026, 10, 22, 11, 0, 0, 123000, tzinfo=dt.timezone.utc),
            "arr": [1, "x", {"n": Int64(-1)}], "sub": {"z": 1, "a": 2},
            "bin": Binary(b"\x00\x01", 0), "re": Regex("^a", "i"), "ts": Timestamp(1700000000, 3),
            "dec": Decimal128("1.5"), "min": MinKey(), "max": MaxKey(),
        }
        got = statediff.bson_decode(bson.encode(doc))
        self.assertEqual(list(got), list(doc))
        self.assertEqual(got["_id"], {"$oid": "65a0000000000000000000b5"})
        self.assertEqual(got["i32"], 7)
        self.assertEqual(got["i64"], {"$numberLong": "7"})
        self.assertEqual(got["big"], {"$numberLong": str(2**40)})
        self.assertIsInstance(got["dbl"], float)
        self.assertEqual(got["s"], "héllo")
        self.assertEqual(got["d"], {"$date": "2026-10-22T11:00:00.123Z"})
        self.assertEqual(got["arr"], [1, "x", {"n": {"$numberLong": "-1"}}])
        self.assertEqual(list(got["sub"]), ["z", "a"])
        self.assertEqual(got["bin"], {"$binary": {"base64": "AAE=", "subType": "00"}})
        self.assertEqual(got["re"], {"$regularExpression": {"pattern": "^a", "options": "i"}})
        self.assertEqual(got["ts"], {"$timestamp": {"t": 1700000000, "i": 3}})
        self.assertIn("$numberDecimalBits", got["dec"])
        self.assertEqual((got["min"], got["max"]), ({"$minKey": 1}, {"$maxKey": 1}))
        self.assertEqual(json.dumps(got["dbl"]), "7.0")

    def test_encode_round_trips_through_pymongo(self):
        import datetime as dt
        cmd = {"find": "c", "filter": {"_id": {"$oid": "65a0000000000000000000b5"},
                                       "at": {"$gte": {"$date": "2026-01-01T00:00:00Z"}},
                                       "n": {"$numberLong": "5"}, "in": [1, 2.5, None, True]},
               "cursor": statediff.Long(9), "big": 2**40, "$db": "kiwi"}
        back = bson.decode(statediff.bson_encode(cmd))
        self.assertEqual(back["filter"]["_id"], ObjectId("65a0000000000000000000b5"))
        self.assertEqual(back["filter"]["at"]["$gte"], dt.datetime(2026, 1, 1))
        self.assertIsInstance(back["filter"]["n"], Int64)
        self.assertIsInstance(back["cursor"], Int64)
        self.assertEqual(back["big"], 2**40)
        self.assertEqual(back["filter"]["in"], [1, 2.5, None, True])


# --------------------------------------------------------------- fake stores

def fake_mongo(collections):
    received = []

    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            sock = self.request
            while True:
                try:
                    head = statediff._recv(sock, 16)
                except statediff.StoreError:
                    return
                length, req_id, _, opcode = struct.unpack("<iiii", head)
                body = statediff._recv(sock, length - 16)
                cmd = bson.decode(body[5:])
                received.append(cmd)
                if "find" in cmd:
                    docs = collections.get(cmd["find"], [])
                    reply = {"cursor": {"firstBatch": docs[:1], "id": Int64(77 if len(docs) > 1 else 0),
                                        "ns": "kiwi." + cmd["find"]}, "ok": 1.0}
                elif "getMore" in cmd:
                    reply = {"cursor": {"nextBatch": collections[cmd["collection"]][1:], "id": Int64(0)}, "ok": 1.0}
                else:
                    reply = {"ok": 0.0, "errmsg": "no such command"}
                payload = struct.pack("<I", 0) + b"\x00" + bson.encode(reply)
                sock.sendall(struct.pack("<iiii", 16 + len(payload), 1, req_id, 2013) + payload)

    return serve(socketserver.ThreadingTCPServer, Handler), received


def fake_postgres(rows_by_table):
    received = []

    def msg(kind, payload):
        return kind + struct.pack("!i", 4 + len(payload)) + payload

    class Handler(socketserver.BaseRequestHandler):
        def handle(self):
            sock = self.request
            n = struct.unpack("!i", statediff._recv(sock, 4))[0]
            statediff._recv(sock, n - 4)
            sock.sendall(msg(b"R", struct.pack("!i", 0)) + msg(b"S", b"TimeZone\x00UTC\x00")
                         + msg(b"K", struct.pack("!ii", 1, 2)) + msg(b"Z", b"I"))
            while True:
                kind, payload = statediff._pg_read(sock)
                if kind == b"X":
                    return
                sql = payload[:-1].decode()
                received.append(sql)
                table = sql.split(" FROM ")[1].split()[0]
                if table not in rows_by_table:
                    sock.sendall(msg(b"E", b"SERROR\x00Mrelation does not exist\x00\x00") + msg(b"Z", b"I"))
                    continue
                out = msg(b"T", struct.pack("!h", 1) + b"row_to_json\x00" + struct.pack("!ihihih", 0, 0, 25, -1, -1, 0))
                for r in rows_by_table[table]:
                    raw = json.dumps(r).encode()
                    out += msg(b"D", struct.pack("!hi", 1, len(raw)) + raw)
                sock.sendall(out + msg(b"C", b"SELECT 1\x00") + msg(b"Z", b"I"))

    return serve(socketserver.ThreadingTCPServer, Handler), received


def fake_redis(data):
    """data: key -> (type, value, ttl)."""

    def enc(v):
        if v is None:
            return b"$-1\r\n"
        if isinstance(v, int):
            return b":%d\r\n" % v
        if isinstance(v, list):
            return b"*%d\r\n" % len(v) + b"".join(enc(x) for x in v)
        if isinstance(v, str):
            return b"+" + v.encode() + b"\r\n"
        return b"$%d\r\n%s\r\n" % (len(v), v)

    class Handler(socketserver.StreamRequestHandler):
        def handle(self):
            while True:
                line = self.rfile.readline()
                if not line:
                    return
                args = []
                for _ in range(int(line[1:])):
                    n = int(self.rfile.readline()[1:])
                    args.append(self.rfile.read(n + 2)[:-2])
                cmd, key = args[0].decode().upper(), args[1] if len(args) > 1 else b""
                kind, value, ttl = data.get(key.decode(errors="replace"), ("none", None, -2))
                if cmd == "SCAN":
                    pattern = args[3].decode()
                    out = [b"0", [k.encode() for k in data if fnmatch.fnmatchcase(k, pattern)]]
                elif cmd == "TYPE":
                    out = kind
                elif cmd == "TTL":
                    out = ttl
                elif cmd == "HGETALL":
                    out = [x for kv in value.items() for x in kv]
                elif cmd == "ZRANGE":
                    out = [x for m, s in value for x in (m, s)]
                else:
                    out = value
                self.wfile.write(enc(out))

    return serve(socketserver.ThreadingTCPServer, Handler)


@unittest.skipIf(bson is None, "pymongo not installed")
class StateDiffTests(unittest.TestCase):
    def setUp(self):
        self.servers = []
        self.tmp = tempfile.TemporaryDirectory()

    def tearDown(self):
        for s in self.servers:
            s.shutdown()
            s.server_close()
        self.tmp.cleanup()

    def stack(self, mongo, pg, redis):
        m, m_seen = fake_mongo(mongo)
        p, p_seen = fake_postgres(pg)
        r = fake_redis(redis)
        self.servers += [m, p, r]
        urls = [f"mongodb://127.0.0.1:{m.server_address[1]}/kiwi",
                f"postgresql://kiwi@127.0.0.1:{p.server_address[1]}/kiwi_lb",
                f"redis://127.0.0.1:{r.server_address[1]}/0"]
        return urls, m_seen, p_seen

    def state(self, spec):
        path = os.path.join(self.tmp.name, "area.state.json")
        with open(path, "w") as f:
            json.dump(spec, f)
        return path

    def stores(self, voter_ip_hash="aa", ttl=30, rows=None):
        import datetime as dt
        import gzip
        mongo = {"supporters": [
            {"_id": ObjectId(), "name": "Zed", "added_by": None, "created_at": dt.datetime.now()},
            {"_id": ObjectId(), "name": "Amy", "added_by": ObjectId("65a0000000000000000000b5"),
             "created_at": dt.datetime.now(), "hash": voter_ip_hash},
        ]}
        pg = {"board": rows or [{"uuid": 1, "name": "Mastery", "updated_at": "2026-10-22T11:00:00+00:00"}]}
        redis = {
            "rl:x:203.0.113.7": ("zset", [(b"1761130000.123:1", b"1761130000.123")], ttl),
            "lb:page:1": ("string", gzip.compress(json.dumps({"rows": [1], "generated_at": str(ttl)}).encode()), -1),
            "h": ("hash", {b"b": b"2", b"a": b'{"k": 1}'}, -1),
        }
        return mongo, pg, redis

    SPEC = {
        "mongo": [{"collection": "supporters", "sort": ["name"], "mask": ["_id", "created_at"],
                   "filter": {"name": {"$ne": "nobody"}}}],
        "postgres": [{"table": "board", "order_by": "uuid", "mask": ["updated_at"], "where": "uuid > 0"}],
        "redis": [{"pattern": "rl:*", "mask_ttl": True, "mask_value": True},
                  {"pattern": "lb:*", "mask": ["generated_at"]}, {"pattern": "h"}],
    }

    def test_identical_after_masks(self):
        a, m_seen, p_seen = self.stack(*self.stores(ttl=30))
        b, _, _ = self.stack(*self.stores(ttl=29))
        rc, out = quiet(statediff.main, [self.state(self.SPEC), "--a", *a, "--b", *b])
        self.assertEqual(rc, 0, out)
        self.assertEqual(m_seen[0]["filter"], {"name": {"$ne": "nobody"}})
        self.assertIn("getMore", m_seen[1])
        self.assertEqual(p_seen[0], "SELECT row_to_json(t)::text FROM board t WHERE uuid > 0 ORDER BY uuid")
        self.assertIn("5 section(s): 5 identical", out)

    def test_reports_differences(self):
        a, _, _ = self.stack(*self.stores(voter_ip_hash="aa"))
        b, _, _ = self.stack(*self.stores(voter_ip_hash="bb", rows=[
            {"uuid": 1, "name": "Mastery!", "updated_at": None},
            {"uuid": 2, "name": "Extra", "updated_at": None}]))
        rc, out = quiet(statediff.main, [self.state(self.SPEC), "--a", *a, "--b", *b])
        self.assertEqual(rc, 1, out)
        self.assertIn('~ name="Amy"', out)
        self.assertIn('$.hash: "aa" vs "bb"', out)
        self.assertIn('"<masked:string>" vs "<masked:null>"', out)
        self.assertIn("only in B", out)

    def test_unmasked_ids_are_differences(self):
        a, _, _ = self.stack(*self.stores())
        b, _, _ = self.stack(*self.stores())
        spec = {"mongo": [{"collection": "supporters", "sort": ["name"], "mask": ["created_at"]}]}
        rc, out = quiet(statediff.main, [self.state(spec), "--a", *a, "--b", *b])
        self.assertEqual(rc, 1)
        self.assertIn("needs a mask", out)

    def test_bad_spec_and_store_errors(self):
        a, _, _ = self.stack(*self.stores())
        rc, out = quiet(statediff.main, [self.state({"postgress": []}), "--a", *a, "--b", *a])
        self.assertEqual(rc, 2)
        rc, out = quiet(statediff.main, [self.state({"postgres": [{"table": "missing"}]}), "--a", *a, "--b", *a])
        self.assertEqual(rc, 2)
        self.assertIn("relation does not exist", out)
        rc, out = quiet(statediff.main, [self.state({"redis": [{"pattern": "*"}]}), "--a", a[0], "--b", *a])
        self.assertEqual(rc, 2)


class MaskTests(unittest.TestCase):
    def test_mask_paths(self):
        doc = {"a": {"b": [{"c": 1}, {"c": None}, {"d": 2}]}, "l": [1, 2], "o": {"$oid": "ab"}}
        for p in ("a.b.c", "l.1", "o", "missing.x", "a.b.5.c"):
            statediff.mask_path(doc, p)
        self.assertEqual(doc, {"a": {"b": [{"c": "<masked:int>"}, {"c": "<masked:null>"}, {"d": 2}]},
                               "l": [1, "<masked:int>"], "o": "<masked:oid>"})

    def test_redis_value_decoding(self):
        import gzip
        self.assertEqual(statediff.redis_value(b'{"a": 1}'), {"a": 1})
        self.assertEqual(statediff.redis_value(b"plain"), "plain")
        self.assertEqual(statediff.redis_value(b"\xff\x00"), {"$hex": "ff00"})
        self.assertEqual(statediff.redis_value(gzip.compress(b"[1]")), {"$gzip": [1]})


if __name__ == "__main__":
    unittest.main()
