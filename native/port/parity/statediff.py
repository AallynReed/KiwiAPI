#!/usr/bin/env python3
"""Compare what the two twin-sandbox stacks stored after a write run.

Stdlib only: speaks just enough of the MongoDB (OP_MSG), PostgreSQL (simple
query, trust auth) and Redis (RESP2) protocols to read what a state spec
names. ``sandbox.sh twin-state AREA`` runs it inside the twin runner:

    python statediff.py AREA.state.json \\
        --a mongodb://kp-a-mongo:27017/kiwi postgresql://kiwi@kp-a-pg:5432/kiwi_lb redis://kp-a-redis:6379/0 \\
        --b mongodb://kp-b-mongo:27017/kiwi postgresql://kiwi@kp-b-pg:5432/kiwi_lb redis://kp-b-redis:6379/0

AREA.state.json (every section and key but the names is optional):

    {"mongo": [{"collection": "supporters", "sort": ["name"], "mask": ["_id", "created_at"],
                "filter": {"name": {"$ne": "Parity Placeholder"}}}],
     "postgres": [{"table": "lb_board", "order_by": "uuid", "mask": ["updated_at"], "where": "uuid > 0"}],
     "redis": [{"pattern": "rl:*", "mask_ttl": true, "mask_value": true},
               {"pattern": "lb:page:*", "mask": ["generated_at"]}]}

mongo: documents in extended JSON with their field order (ObjectId {"$oid"},
date {"$date"}, int64 {"$numberLong"}, a double keeps its decimal point), sorted
by the ``sort`` fields and then the whole document. ``filter`` is a find filter
({"$oid"}, {"$date"} and {"$numberLong"} values are typed). ``upsert_keys``
names the fields an upsert's filter seeds: mongod 7 lays those out in absl
hash-map order, salted per process and per table, so each stack (and each
insert) can get its own order; they are compared in the spec's order instead.
The ``$set`` fields after them are deterministic (lexicographic). postgres: rows as
row_to_json in ``order_by`` order (default: the row text), ``mask`` names
columns. redis: the keys SCAN MATCH finds for ``pattern``, each compared on
type, TTL in seconds (only set or unset with ``mask_ttl``) and value: strings
gunzipped when gzipped and parsed when JSON, hashes, lists, sets, sorted sets
with scores, streams; ``mask_value`` keeps only the type and element count.

Two schema sections compare structure instead of rows (startup twins):

    {"mongo_indexes": [{"collection": "*"}], "pg_schema": [{"schema": "public"}]}

mongo_indexes: listCollections (type and options; ``*`` = every collection but
system.*) and each collection's listIndexes specs, keyed by index name (the
catalog's order is not compared). pg_schema: the schema's relations (kind,
persistence, partition key and bound, parent, tablespace), columns (number,
type, not-null, default, identity, generated), constraints, indexes
(``pg_get_indexdef``) and sequences, each keyed by name.

``mask`` paths are dotted; a numeric part indexes a list, any other part
applies to every element of a list. A masked value becomes "<masked:TYPE>", so
presence and type still count. Nothing is masked unless the spec says so: an
ObjectId or date that each stack made at its own instant is a difference until
the spec masks it.

Exit status: 0 identical, 1 differences, 2 bad usage or a store error.
"""

from __future__ import annotations

import argparse
import base64
import datetime as dt
import difflib
import gzip
import json
import socket
import struct
import sys
import urllib.parse
import zlib

from run import _json_paths

TIMEOUT = 60


class StoreError(Exception):
    pass


# ---------------------------------------------------------------- BSON / Mongo

class Long(int):
    """An int encoded as BSON int64."""


_EPOCH = dt.datetime(1970, 1, 1, tzinfo=dt.timezone.utc)


def _cstring(buf: bytes, i: int) -> tuple[str, int]:
    j = buf.index(b"\x00", i)
    return buf[i:j].decode("utf-8", "surrogateescape"), j + 1


def _string(buf: bytes, i: int) -> tuple[str, int]:
    n = struct.unpack_from("<i", buf, i)[0]
    return buf[i + 4:i + 3 + n].decode("utf-8", "surrogateescape"), i + 4 + n


def _date(ms: int):
    try:
        d = _EPOCH + dt.timedelta(milliseconds=ms)
    except OverflowError:
        return {"$date": {"$numberLong": str(ms)}}
    return {"$date": f"{d:%Y-%m-%dT%H:%M:%S}.{ms % 1000:03d}Z"}


def bson_decode(buf: bytes, i: int = 0, array: bool = False):
    """Decode the document at ``buf[i:]`` into extended-JSON-shaped values."""
    end = i + struct.unpack_from("<i", buf, i)[0] - 1
    i += 4
    out: list | dict = [] if array else {}
    while i < end:
        t = buf[i]
        name, i = _cstring(buf, i + 1)
        if t == 0x01:
            v = struct.unpack_from("<d", buf, i)[0]; i += 8
        elif t in (0x02, 0x0D, 0x0E):
            v, i = _string(buf, i)
            v = {0x02: v, 0x0D: {"$code": v}, 0x0E: {"$symbol": v}}[t]
        elif t in (0x03, 0x04):
            v = bson_decode(buf, i, t == 0x04)
            i += struct.unpack_from("<i", buf, i)[0]
        elif t == 0x05:
            n = struct.unpack_from("<i", buf, i)[0]
            v = {"$binary": {"base64": base64.b64encode(buf[i + 5:i + 5 + n]).decode(), "subType": f"{buf[i + 4]:02x}"}}
            i += 5 + n
        elif t == 0x06:
            v = {"$undefined": True}
        elif t == 0x07:
            v = {"$oid": buf[i:i + 12].hex()}; i += 12
        elif t == 0x08:
            v = buf[i] == 1; i += 1
        elif t == 0x09:
            v = _date(struct.unpack_from("<q", buf, i)[0]); i += 8
        elif t == 0x0A:
            v = None
        elif t == 0x0B:
            pattern, i = _cstring(buf, i)
            options, i = _cstring(buf, i)
            v = {"$regularExpression": {"pattern": pattern, "options": options}}
        elif t == 0x0C:
            ns, i = _string(buf, i)
            v = {"$dbPointer": {"$ref": ns, "$id": {"$oid": buf[i:i + 12].hex()}}}; i += 12
        elif t == 0x0F:
            code, j = _string(buf, i + 4)
            v = {"$code": code, "$scope": bson_decode(buf, j)}
            i += struct.unpack_from("<i", buf, i)[0]
        elif t == 0x10:
            v = struct.unpack_from("<i", buf, i)[0]; i += 4
        elif t == 0x11:
            inc, ts = struct.unpack_from("<II", buf, i); i += 8
            v = {"$timestamp": {"t": ts, "i": inc}}
        elif t == 0x12:
            v = {"$numberLong": str(struct.unpack_from("<q", buf, i)[0])}; i += 8
        elif t == 0x13:
            v = {"$numberDecimalBits": buf[i:i + 16][::-1].hex()}; i += 16
        elif t == 0xFF:
            v = {"$minKey": 1}
        elif t == 0x7F:
            v = {"$maxKey": 1}
        else:
            raise StoreError(f"unknown BSON type 0x{t:02x} at {name!r}")
        if array:
            out.append(v)  # type: ignore[union-attr]
        else:
            out[name] = v  # type: ignore[index]
    return out


def bson_encode(doc: dict) -> bytes:
    body = b"".join(_element(str(k), v) for k, v in doc.items())
    return struct.pack("<i", len(body) + 5) + body + b"\x00"


def _element(key: str, v) -> bytes:
    name = key.encode() + b"\x00"
    if isinstance(v, bool):
        return b"\x08" + name + (b"\x01" if v else b"\x00")
    if isinstance(v, int) and (isinstance(v, Long) or not -2**31 <= v < 2**31):
        return b"\x12" + name + struct.pack("<q", v)
    if isinstance(v, int):
        return b"\x10" + name + struct.pack("<i", v)
    if isinstance(v, float):
        return b"\x01" + name + struct.pack("<d", v)
    if isinstance(v, str):
        s = v.encode() + b"\x00"
        return b"\x02" + name + struct.pack("<i", len(s)) + s
    if v is None:
        return b"\x0a" + name
    if isinstance(v, list):
        return b"\x04" + name + bson_encode({str(i): x for i, x in enumerate(v)})
    if isinstance(v, dict):
        if list(v) == ["$oid"]:
            return b"\x07" + name + bytes.fromhex(v["$oid"])
        if list(v) == ["$date"]:
            d = dt.datetime.fromisoformat(v["$date"])
            d = d if d.tzinfo else d.replace(tzinfo=dt.timezone.utc)
            ms = (d - _EPOCH) // dt.timedelta(milliseconds=1)
            return b"\x09" + name + struct.pack("<q", ms)
        if list(v) == ["$numberLong"]:
            return _element(key, Long(int(v["$numberLong"])))
        return b"\x03" + name + bson_encode(v)
    raise TypeError(f"cannot encode {type(v).__name__} as BSON")


def _recv(sock: socket.socket, n: int) -> bytes:
    buf = bytearray()
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise StoreError("connection closed mid-reply")
        buf += chunk
    return bytes(buf)


def _as_int(v) -> int:
    return int(v["$numberLong"]) if isinstance(v, dict) else int(v)


def _op_msg(sock: socket.socket, request_id: int, cmd: dict) -> dict:
    payload = struct.pack("<I", 0) + b"\x00" + bson_encode(cmd)
    sock.sendall(struct.pack("<iiii", 16 + len(payload), request_id, 0, 2013) + payload)
    length, _, _, opcode = struct.unpack("<iiii", _recv(sock, 16))
    body = _recv(sock, length - 16)
    if opcode != 2013 or body[4] != 0:
        raise StoreError(f"unexpected Mongo reply (opcode {opcode})")
    reply = bson_decode(body, 5)
    if reply.get("ok") != 1:
        raise StoreError(f"Mongo: {reply.get('errmsg', reply)}")
    return reply


def mongo_cursor(url: str, cmd: dict, collection: str) -> list[dict]:
    """Every document a cursor-returning command yields (``collection`` names
    the getMore target)."""
    u = urllib.parse.urlsplit(url)
    db = u.path.lstrip("/") or "kiwi"
    with socket.create_connection((u.hostname, u.port or 27017), timeout=TIMEOUT) as s:
        cursor = _op_msg(s, 1, {**cmd, "$db": db})["cursor"]
        docs, n = list(cursor["firstBatch"]), 2
        while _as_int(cursor["id"]):
            more = {"getMore": Long(_as_int(cursor["id"])), "collection": collection, "batchSize": 1000, "$db": db}
            cursor = _op_msg(s, n, more)["cursor"]
            docs += cursor["nextBatch"]
            n += 1
    return docs


def mongo_docs(url: str, collection: str, flt: dict | None = None) -> list[dict]:
    return mongo_cursor(url, {"find": collection, "filter": flt or {}, "batchSize": 1000}, collection)


# ------------------------------------------------------------------- Postgres

def _pg_read(sock: socket.socket) -> tuple[bytes, bytes]:
    head = _recv(sock, 5)
    return head[:1], _recv(sock, struct.unpack("!i", head[1:])[0] - 4)


def _pg_error(payload: bytes) -> str:
    fields = {f[:1]: f[1:].decode("utf-8", "replace") for f in payload.split(b"\x00") if f}
    return f"Postgres {fields.get(b'S', 'ERROR')}: {fields.get(b'M', '?')}"


def pg_rows(url: str, sql: str) -> list[list[str | None]]:
    u = urllib.parse.urlsplit(url)
    user = urllib.parse.unquote(u.username or "postgres")
    db = u.path.lstrip("/") or user
    with socket.create_connection((u.hostname, u.port or 5432), timeout=TIMEOUT) as s:
        params = b"user\x00" + user.encode() + b"\x00database\x00" + db.encode() + b"\x00\x00"
        s.sendall(struct.pack("!ii", 8 + len(params), 196608) + params)
        while True:
            kind, payload = _pg_read(s)
            if kind == b"R" and struct.unpack_from("!i", payload)[0] != 0:
                raise StoreError("Postgres wants a password; the twin runs it with POSTGRES_HOST_AUTH_METHOD=trust")
            if kind == b"E":
                raise StoreError(_pg_error(payload))
            if kind == b"Z":
                break
        q = sql.encode() + b"\x00"
        s.sendall(b"Q" + struct.pack("!i", 4 + len(q)) + q)
        rows, error = [], None
        while True:
            kind, payload = _pg_read(s)
            if kind == b"D":
                row, i = [], 2
                for _ in range(struct.unpack_from("!h", payload)[0]):
                    n = struct.unpack_from("!i", payload, i)[0]
                    i += 4
                    row.append(None if n < 0 else payload[i:i + n].decode())
                    i += max(n, 0)
                rows.append(row)
            elif kind == b"E":
                error = _pg_error(payload)
            elif kind == b"Z":
                break
        s.sendall(b"X\x00\x00\x00\x04")
    if error:
        raise StoreError(f"{error}\n  query: {sql}")
    return rows


# ---------------------------------------------------------------------- Redis

class Redis:
    def __init__(self, url: str):
        u = urllib.parse.urlsplit(url)
        self.sock = socket.create_connection((u.hostname, u.port or 6379), timeout=TIMEOUT)
        self.reader = self.sock.makefile("rb")
        if u.password:
            self.call("AUTH", urllib.parse.unquote(u.password))
        if u.path.strip("/") not in ("", "0"):
            self.call("SELECT", u.path.strip("/"))

    def close(self) -> None:
        self.reader.close()
        self.sock.close()

    def call(self, *args):
        out = [b"*%d\r\n" % len(args)]
        for a in args:
            a = a if isinstance(a, bytes) else str(a).encode()
            out.append(b"$%d\r\n%s\r\n" % (len(a), a))
        self.sock.sendall(b"".join(out))
        return self._reply()

    def _reply(self):
        line = self.reader.readline()
        if not line.endswith(b"\r\n"):
            raise StoreError("Redis closed the connection")
        kind, rest = line[:1], line[1:-2]
        if kind == b"+":
            return rest.decode()
        if kind == b"-":
            raise StoreError(f"Redis: {rest.decode()}")
        if kind == b":":
            return int(rest)
        if kind == b"$":
            n = int(rest)
            return None if n < 0 else self.reader.read(n + 2)[:-2]
        if kind == b"*":
            n = int(rest)
            return None if n < 0 else [self._reply() for _ in range(n)]
        raise StoreError(f"unexpected Redis reply {line!r}")


def _text(raw: bytes) -> str:
    return raw.decode("utf-8", "backslashreplace")


def redis_value(raw: bytes):
    """Gunzip a gzip value, parse JSON text, else keep text (or hex for binary)."""
    zipped = raw[:2] == b"\x1f\x8b"
    if zipped:
        try:
            raw = gzip.decompress(raw)
        except (OSError, EOFError, zlib.error):
            zipped = False
    try:
        text = raw.decode("utf-8")
    except UnicodeDecodeError:
        value = {"$hex": raw.hex()}
    else:
        try:
            value = json.loads(text)
        except ValueError:
            value = text
    return {"$gzip": value} if zipped else value


def redis_entries(url: str, pattern: str) -> list[dict]:
    r = Redis(url)
    try:
        cursor, keys = b"0", set()
        while True:
            cursor, batch = r.call("SCAN", cursor, "MATCH", pattern, "COUNT", 1000)
            keys.update(batch)
            if cursor == b"0":
                break
        out = []
        for key in sorted(keys):
            kind = r.call("TYPE", key)
            if kind == "string":
                value = redis_value(r.call("GET", key))
            elif kind == "hash":
                flat = r.call("HGETALL", key)
                value = {_text(k): redis_value(v) for k, v in sorted(zip(flat[::2], flat[1::2], strict=True))}
            elif kind == "list":
                value = [redis_value(v) for v in r.call("LRANGE", key, 0, -1)]
            elif kind == "set":
                value = [redis_value(v) for v in sorted(r.call("SMEMBERS", key))]
            elif kind == "zset":
                flat = r.call("ZRANGE", key, 0, -1, "WITHSCORES")
                value = [[redis_value(m), _text(s)] for m, s in zip(flat[::2], flat[1::2], strict=True)]
            elif kind == "stream":
                value = [[_text(i), [_text(f) for f in fields]] for i, fields in r.call("XRANGE", key, "-", "+")]
            else:
                continue
            out.append({"key": _text(key), "type": kind, "ttl": r.call("TTL", key), "value": value})
        return out
    finally:
        r.close()


# -------------------------------------------------------------------- compare

_MISSING = object()


def _kind(v) -> str:
    if v is None:
        return "null"
    if isinstance(v, bool):
        return "bool"
    if isinstance(v, int):
        return "int"
    if isinstance(v, float):
        return "double"
    if isinstance(v, str):
        return "string"
    if isinstance(v, list):
        return "array"
    if isinstance(v, dict) and len(v) == 1 and next(iter(v)).startswith("$"):
        return next(iter(v))[1:]
    return "object"


def mask_path(doc, path: str) -> None:
    parts = path.split(".")

    def walk(node, i: int) -> None:
        if isinstance(node, list) and not parts[i].isdigit():
            for item in node:
                walk(item, i)
            return
        if isinstance(node, dict):
            key = parts[i]
        elif isinstance(node, list) and int(parts[i]) < len(node):
            key = int(parts[i])
        else:
            return
        if isinstance(node, dict) and key not in node:
            return
        if i == len(parts) - 1:
            node[key] = f"<masked:{_kind(node[key])}>"
        else:
            walk(node[key], i + 1)

    walk(doc, 0)


def get_path(doc, path: str):
    node = doc
    for part in path.split("."):
        if isinstance(node, dict) and part in node:
            node = node[part]
        elif isinstance(node, list) and part.isdigit() and int(part) < len(node):
            node = node[int(part)]
        else:
            return _MISSING
    return node


def _line(v) -> str:
    return json.dumps(v, ensure_ascii=False)


def diff_records(title: str, a: list[tuple[str, object]], b: list[tuple[str, object]], limit: int) -> bool:
    """Print how two ordered (label, record) lists part; True when they differ."""
    la, lb = [_line(v) for _, v in a], [_line(v) for _, v in b]
    out: list[str] = []
    sm = difflib.SequenceMatcher(None, la, lb, autojunk=False)
    for op, i1, i2, j1, j2 in sm.get_opcodes():
        if op == "equal":
            continue
        xs, ys = list(range(i1, i2)), list(range(j1, j2))
        if len(xs) == len(ys):
            pairs, xs, ys = list(zip(xs, ys, strict=True)), [], []
        else:
            pairs = []
            for x in list(xs):
                y = next((y for y in ys if b[y][0] == a[x][0]), None)
                if y is not None:
                    pairs.append((x, y))
                    xs.remove(x)
                    ys.remove(y)
        for x, y in pairs:
            label = a[x][0] if a[x][0] == b[y][0] else f"{a[x][0]} / {b[y][0]}"
            out.append(f"  ~ {label}")
            out += ["      " + p for p in _json_paths(a[x][1], b[y][1])]
        out += [f"  - only in A {a[x][0]}: {la[x][:400]}" for x in xs]
        out += [f"  + only in B {b[y][0]}: {lb[y][:400]}" for y in ys]
    print(f"{'DIFF' if out else 'OK  '}  {title}  ({len(a)} vs {len(b)})")
    for line in out[:limit]:
        print(line)
    if len(out) > limit:
        print(f"  ... {len(out) - limit} more line(s)")
    if any("$oid" in line or "$date" in line for line in out):
        print("  note: an ObjectId or date each stack made at its own instant needs a mask in the state spec")
    return bool(out)


def upsert_order(doc: dict, keys: list[str]) -> dict:
    """``doc`` with the fields after ``_id`` that are exactly ``keys`` (in any
    order) put in ``keys`` order; anything else is left as it is."""
    head = [k for k in doc if k != "_id"][:len(keys)]
    if sorted(head) != sorted(keys):
        return doc
    out = {"_id": doc["_id"]} if "_id" in doc else {}
    out.update((k, doc[k]) for k in keys)
    out.update((k, v) for k, v in doc.items() if k not in out)
    return out


def mongo_records(url: str, rule: dict) -> list[tuple[str, object]]:
    docs = mongo_docs(url, rule["collection"], rule.get("filter"))
    if rule.get("upsert_keys"):
        docs = [upsert_order(d, rule["upsert_keys"]) for d in docs]
    for doc in docs:
        for path in rule.get("mask", []):
            mask_path(doc, path)
    sort = rule.get("sort", [])

    def keyed(d) -> tuple[list[str], str]:
        return [_line(v) if (v := get_path(d, f)) is not _MISSING else "" for f in sort], _line(d)

    docs.sort(key=keyed)
    out = []
    for n, d in enumerate(docs):
        label = ", ".join(f"{f}={_line(v)}" for f in sort if (v := get_path(d, f)) is not _MISSING)
        out.append((label or f"#{n + 1}", d))
    return out


def pg_records(url: str, rule: dict) -> list[tuple[str, object]]:
    where = f" WHERE {rule['where']}" if rule.get("where") else ""
    order = rule.get("order_by") or 'row_to_json(t)::text COLLATE "C"'
    sql = f"SELECT row_to_json(t)::text FROM {rule['table']} t{where} ORDER BY {order}"
    keys = [p.split()[0] for p in (rule.get("order_by") or "").split(",") if p.strip()]
    out = []
    for n, (text,) in enumerate(pg_rows(url, sql)):
        row = json.loads(text or "null")
        for col in rule.get("mask", []):
            mask_path(row, col)
        label = ", ".join(f"{k}={_line(row[k])}" for k in keys) if keys and all(k in row for k in keys) else ""
        out.append((label or f"#{n + 1}", row))
    return out


def mongo_index_records(url: str, rule: dict) -> list[tuple[str, object]]:
    name = rule["collection"]
    flt = {} if name == "*" else {"name": name}
    colls = mongo_cursor(url, {"listCollections": 1, "filter": flt}, "$cmd.listCollections")
    out = []
    for c in sorted(colls, key=lambda c: c["name"]):
        if c["name"].startswith("system."):
            continue
        out.append((f"collection {c['name']}", {"type": c.get("type"), "options": c.get("options", {})}))
        if c.get("type", "collection") != "collection":
            continue
        specs = mongo_cursor(url, {"listIndexes": c["name"]}, c["name"])
        for spec in sorted(specs, key=lambda s: s["name"]):
            for path in rule.get("mask", []):
                mask_path(spec, path)
            out.append((f"index {c['name']}.{spec['name']}", spec))
    return out


_PG_SCHEMA_SQL = """SELECT row_to_json(t)::text FROM (
 SELECT 'relation' AS kind, c.relname::text AS label, json_build_object(
   'relkind', c.relkind::text, 'persistence', c.relpersistence::text,
   'partkey', pg_get_partkeydef(c.oid), 'partbound', pg_get_expr(c.relpartbound, c.oid),
   'parent', (SELECT p.relname::text FROM pg_inherits i JOIN pg_class p ON p.oid = i.inhparent
              WHERE i.inhrelid = c.oid),
   'tablespace', COALESCE(ts.spcname::text, 'pg_default')) AS def
 FROM pg_class c JOIN pg_namespace n ON n.oid = c.relnamespace
 LEFT JOIN pg_tablespace ts ON ts.oid = c.reltablespace
 WHERE n.nspname = {schema} AND c.relkind IN ('r', 'p', 'v', 'm', 'f', 'S', 'i', 'I')
 UNION ALL
 SELECT 'column', c.relname || '.' || a.attname, json_build_object(
   'attnum', a.attnum, 'type', format_type(a.atttypid, a.atttypmod), 'notnull', a.attnotnull,
   'default', pg_get_expr(d.adbin, d.adrelid), 'identity', a.attidentity::text,
   'generated', a.attgenerated::text)
 FROM pg_attribute a JOIN pg_class c ON c.oid = a.attrelid JOIN pg_namespace n ON n.oid = c.relnamespace
 LEFT JOIN pg_attrdef d ON d.adrelid = a.attrelid AND d.adnum = a.attnum
 WHERE n.nspname = {schema} AND c.relkind IN ('r', 'p', 'v', 'm', 'f') AND a.attnum > 0 AND NOT a.attisdropped
 UNION ALL
 SELECT 'constraint', c.relname || '.' || k.conname, json_build_object(
   'type', k.contype::text, 'def', pg_get_constraintdef(k.oid))
 FROM pg_constraint k JOIN pg_class c ON c.oid = k.conrelid JOIN pg_namespace n ON n.oid = c.relnamespace
 WHERE n.nspname = {schema}
 UNION ALL
 SELECT 'index', i.indexname::text, json_build_object('table', i.tablename, 'def', i.indexdef)
 FROM pg_indexes i WHERE i.schemaname = {schema}
 UNION ALL
 SELECT 'sequence', s.sequencename::text, json_build_object(
   'type', s.data_type::text, 'start', s.start_value, 'increment', s.increment_by, 'min', s.min_value,
   'max', s.max_value, 'cache', s.cache_size, 'cycle', s.cycle)
 FROM pg_sequences s WHERE s.schemaname = {schema}
) t ORDER BY t.kind COLLATE "C", t.label COLLATE "C"
"""


def pg_schema_records(url: str, rule: dict) -> list[tuple[str, object]]:
    schema = "'" + rule["schema"].replace("'", "''") + "'"
    out = []
    for (text,) in pg_rows(url, _PG_SCHEMA_SQL.format(schema=schema)):
        row = json.loads(text or "null")
        for path in rule.get("mask", []):
            mask_path(row["def"], path)
        out.append((f"{row['kind']} {row['label']}", row["def"]))
    return out


def redis_records(url: str, rule: dict) -> list[tuple[str, object]]:
    out = []
    for e in redis_entries(url, rule["pattern"]):
        ttl = e["ttl"]
        e["ttl"] = ("set" if ttl >= 0 else "none") if rule.get("mask_ttl") else (ttl if ttl >= 0 else "none")
        v = e["value"]
        if rule.get("mask_value"):
            e["value"] = f"<masked:{e['type']}>" if e["type"] == "string" else f"<masked:{e['type']} x{len(v)}>"
        else:
            inner = v["$gzip"] if isinstance(v, dict) and list(v) == ["$gzip"] else v
            for path in rule.get("mask", []):
                mask_path(inner, path)
        out.append((e["key"], e))
    return out


SECTIONS = {
    "mongo": ("mongodb", "collection", mongo_records),
    "postgres": ("postgresql", "table", pg_records),
    "redis": ("redis", "pattern", redis_records),
    "mongo_indexes": ("mongodb", "collection", mongo_index_records),
    "pg_schema": ("postgresql", "schema", pg_schema_records),
}


def _stores(urls: list[str]) -> dict[str, str]:
    out = {}
    for url in urls:
        scheme = urllib.parse.urlsplit(url).scheme
        out["postgresql" if scheme == "postgres" else scheme] = url
    return out


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("spec", help="AREA.state.json")
    ap.add_argument("--a", nargs="+", required=True, metavar="URL", help="stack A's mongodb://, postgresql://, redis:// URLs")
    ap.add_argument("--b", nargs="+", required=True, metavar="URL", help="stack B's URLs")
    ap.add_argument("--limit", type=int, default=40, help="difference lines shown per section")
    args = ap.parse_args(argv)

    try:
        with open(args.spec, encoding="utf-8") as f:
            spec = json.load(f)
    except (OSError, ValueError) as e:
        print(f"cannot read state spec: {e}", file=sys.stderr)
        return 2
    unknown = set(spec) - set(SECTIONS) if isinstance(spec, dict) else {"<not an object>"}
    if unknown:
        print(f"state spec: unknown section(s) {sorted(unknown)}; expected {sorted(SECTIONS)}", file=sys.stderr)
        return 2
    stores = {"A": _stores(args.a), "B": _stores(args.b)}
    for section, rules in spec.items():
        scheme, name, _ = SECTIONS[section]
        if rules and any(scheme not in s for s in stores.values()):
            print(f"{section} needs a {scheme}:// URL for --a and --b", file=sys.stderr)
            return 2
        if any(name not in rule for rule in rules):
            print(f"every {section} rule needs \"{name}\"", file=sys.stderr)
            return 2

    differing = sections = 0
    for section, rules in spec.items():
        scheme, name, read = SECTIONS[section]
        for rule in rules:
            sections += 1
            title = f"{section} {rule[name]}"
            try:
                got = {side: read(stores[side][scheme], rule) for side in ("A", "B")}
            except (StoreError, OSError, ValueError) as e:
                print(f"{title}: {e}", file=sys.stderr)
                return 2
            differing += diff_records(title, got["A"], got["B"], args.limit)

    print(f"\n{sections} section(s): {sections - differing} identical, {differing} differing")
    return 1 if differing else 0


if __name__ == "__main__":
    sys.exit(main())
