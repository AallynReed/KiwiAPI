#!/usr/bin/env python3
"""Replay request specs against two servers and report every difference.

Stdlib only. Used to prove a ported route answers exactly like Python:

    python native/port/parity/run.py SPEC.json http://python:8000 http://rust:8000

SPEC.json is a list of requests:

    [
      {"name": "health", "method": "GET", "path": "/health"},
      {"method": "GET", "path": "/v1/stats/classes?lang=de",
       "headers": {"Accept-Encoding": "gzip", "Origin": "https://docs.aallyn.net"}},
      {"method": "POST", "path": "/v1/thing", "body": {"a": 1}},
      {"method": "GET", "path": "/v1/feeds/news", "mask": ["generated_at", "items[0].fetched"],
       "compare_headers": ["x-ratelimit-limit"]},
      {"method": "GET", "path": "/v1/x", "allow_diff": ["header:allow"], "reason": "FastAPI set order"}
    ]

Per request: ``method`` (default GET), ``path`` (with query), ``headers`` (object,
or list of [name, value] pairs to repeat a header; ``${NAME}`` in a value is
the environment variable, e.g. a token), ``body`` (a string is sent as
UTF-8, anything else as JSON) or ``body_b64``, ``compare_headers`` (extra
headers to compare), ``ignore_headers``, ``mask`` (JSON fields replaced before
comparing; the bodies are then compared re-serialized instead of byte for
byte), ``allow_diff`` (``status``, ``body`` or ``header:<name>``: reported but
not failing) with a ``reason``.

Compared: the status, the headers content-type, cache-control, vary, location,
set-cookie, content-encoding and every access-control-*, and the body bytes. A
gzip body is compared after decoding (``--raw`` compares the encoded bytes
too). When both bodies are JSON a structural diff shows where they part.

Each request gets ``X-Request-ID: parity-<n>`` unless it sets one, so error
envelopes compare equal. Redirects are not followed.

Safety: only GET, HEAD and OPTIONS run unless ``--allow-writes`` is given. Against
production use those only; run anything that writes against throwaway stores.

Exit status: 0 when nothing differs (allowed diffs included), 1 when something
does, 2 on bad usage.
"""

from __future__ import annotations

import argparse
import base64
import difflib
import gzip
import http.client
import json
import os
import re
import sys
import urllib.parse
import zlib

DEFAULT_HEADERS = ("content-type", "cache-control", "vary", "location", "set-cookie", "content-encoding")
SAFE_METHODS = {"GET", "HEAD", "OPTIONS"}


class Reply:
    def __init__(self, status: int, headers: list[tuple[str, str]], body: bytes, error: str | None = None):
        self.status = status
        self.headers = headers
        self.body = body
        self.error = error

    def values(self, name: str) -> list[str]:
        return [v for k, v in self.headers if k.lower() == name]

    def decoded(self) -> bytes:
        enc = ",".join(self.values("content-encoding")).strip().lower()
        if enc == "gzip" and self.body:
            try:
                return gzip.decompress(self.body)
            except (OSError, EOFError, zlib.error):
                return self.body
        return self.body


_ENV = re.compile(r"\$\{(\w+)\}")


def _request_headers(spec: dict) -> list[tuple[str, str]]:
    h = spec.get("headers") or {}
    pairs = h.items() if isinstance(h, dict) else h
    return [(k, _ENV.sub(lambda m: os.environ.get(m.group(1), ""), str(v))) for k, v in pairs]


def _request_body(spec: dict) -> bytes | None:
    if "body_b64" in spec:
        return base64.b64decode(spec["body_b64"])
    if "body" not in spec:
        return None
    body = spec["body"]
    return body.encode() if isinstance(body, str) else json.dumps(body).encode()


def send(base: str, spec: dict, rid: str, timeout: float) -> Reply:
    method = spec.get("method", "GET").upper()
    headers = _request_headers(spec)
    body = _request_body(spec)
    lower = {k.lower() for k, _ in headers}
    if body is not None and "content-type" not in lower and not isinstance(spec.get("body"), str):
        headers.append(("Content-Type", "application/json"))
    if "x-request-id" not in lower:
        headers.append(("X-Request-ID", rid))
    if body is not None and "content-length" not in lower:
        headers.append(("Content-Length", str(len(body))))
    url = urllib.parse.urlsplit(base)
    conn_cls = http.client.HTTPSConnection if url.scheme == "https" else http.client.HTTPConnection
    conn = conn_cls(url.netloc, timeout=timeout)
    try:
        conn.putrequest(method, url.path.rstrip("/") + spec["path"], skip_host="host" in lower,
                        skip_accept_encoding=True)
        for k, v in headers:
            conn.putheader(k, v)
        conn.endheaders(body)
        resp = conn.getresponse()
        return Reply(resp.status, resp.getheaders(), resp.read())
    except Exception as e:  # noqa: BLE001 - any transport failure is a reportable result
        return Reply(0, [], b"", error=f"{type(e).__name__}: {e}")
    finally:
        conn.close()


_PATH_TOKEN = re.compile(r"([^.\[\]]+)|\[(\d+)\]")


def _mask(doc, path: str):
    parts = [(m.group(1), m.group(2)) for m in _PATH_TOKEN.finditer(path)]
    node = doc
    for i, (key, idx) in enumerate(parts):
        last = i == len(parts) - 1
        if key is not None and isinstance(node, dict) and key in node:
            if last:
                node[key] = "<masked>"
            node = node[key]
        elif idx is not None and isinstance(node, list) and int(idx) < len(node):
            if last:
                node[int(idx)] = "<masked>"
            node = node[int(idx)]
        else:
            return


def _json(data: bytes):
    try:
        return json.loads(data)
    except (ValueError, UnicodeDecodeError):
        return None


def _json_paths(a, b, path: str = "$", out: list[str] | None = None, limit: int = 20) -> list[str]:
    out = [] if out is None else out
    if len(out) >= limit:
        return out
    if isinstance(a, dict) and isinstance(b, dict):
        if list(a) != list(b) and set(a) == set(b):
            out.append(f"{path}: same keys, different order: {list(a)} vs {list(b)}")
        for k in a:
            if k not in b:
                out.append(f"{path}.{k}: only in A")
            else:
                _json_paths(a[k], b[k], f"{path}.{k}", out, limit)
        out.extend(f"{path}.{k}: only in B" for k in b if k not in a)
    elif isinstance(a, list) and isinstance(b, list):
        if len(a) != len(b):
            out.append(f"{path}: length {len(a)} vs {len(b)}")
        for i, (x, y) in enumerate(zip(a, b, strict=False)):
            _json_paths(x, y, f"{path}[{i}]", out, limit)
    elif a != b or type(a) is not type(b):
        out.append(f"{path}: {json.dumps(a, ensure_ascii=False)[:200]} vs {json.dumps(b, ensure_ascii=False)[:200]}")
    return out[:limit]


def _first_difference(a: bytes, b: bytes) -> str:
    i = next((n for n, (x, y) in enumerate(zip(a, b, strict=False)) if x != y), min(len(a), len(b)))
    lo = max(0, i - 40)
    return (f"first difference at byte {i} (lengths {len(a)} vs {len(b)}):\n"
            f"      A ...{a[lo:i + 60]!r}\n      B ...{b[lo:i + 60]!r}")


def compare(spec: dict, a: Reply, b: Reply, raw: bool) -> list[tuple[str, str]]:
    """(kind, message) per difference; kind is status, body or header:<name>."""
    if a.error or b.error:
        return [("status", f"transport: A={a.error or 'ok'} B={b.error or 'ok'}")]
    diffs: list[tuple[str, str]] = []
    if a.status != b.status:
        diffs.append(("status", f"status {a.status} vs {b.status}"))

    ignore = {h.lower() for h in spec.get("ignore_headers", [])}
    names = set(DEFAULT_HEADERS) | {h.lower() for h in spec.get("compare_headers", [])}
    names |= {k.lower() for k, _ in a.headers + b.headers if k.lower().startswith("access-control-")}
    for name in sorted(names - ignore):
        va, vb = a.values(name), b.values(name)
        if va != vb:
            diffs.append((f"header:{name}", f"{name}: {va} vs {vb}"))

    da, db = a.decoded(), b.decoded()
    if raw and a.body != b.body:
        diffs.append(("body", "encoded body bytes differ\n    " + _first_difference(a.body, b.body)))
    if spec.get("mask"):
        ja, jb = _json(da), _json(db)
        if ja is None or jb is None:
            diffs.append(("body", "mask given but a body is not JSON"))
            return diffs
        for p in spec["mask"]:
            _mask(ja, p)
            _mask(jb, p)
        if ja != jb or json.dumps(ja) != json.dumps(jb):
            diffs.append(("body", "JSON differs after masking:\n    " + "\n    ".join(_json_paths(ja, jb))))
        return diffs
    if da != db:
        ja, jb = _json(da), _json(db)
        if ja is not None and jb is not None:
            paths = _json_paths(ja, jb)
            msg = "JSON differs:\n    " + "\n    ".join(paths) if paths else "JSON equal, bytes differ (format)"
            pretty = difflib.unified_diff(
                json.dumps(ja, indent=1, ensure_ascii=False).splitlines(),
                json.dumps(jb, indent=1, ensure_ascii=False).splitlines(), "A", "B", n=1, lineterm="")
            msg += "\n    " + "\n    ".join(list(pretty)[:40])
            msg += "\n    " + _first_difference(da, db)
        else:
            msg = "body differs\n    " + _first_difference(da, db)
        diffs.append(("body", msg))
    return diffs


def main(argv: list[str] | None = None) -> int:
    ap = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    ap.add_argument("spec", help="JSON file with a list of requests")
    ap.add_argument("base_a", help="e.g. http://api-py:8000 (Python)")
    ap.add_argument("base_b", help="e.g. http://127.0.0.1:8000 (Rust)")
    ap.add_argument("--only", action="append", default=[], help="run only requests with this name (repeatable)")
    ap.add_argument("--timeout", type=float, default=30.0)
    ap.add_argument("--raw", action="store_true", help="also compare encoded (gzip) body bytes")
    ap.add_argument("--allow-writes", action="store_true", help="permit methods other than GET/HEAD/OPTIONS")
    ap.add_argument("--quiet", action="store_true", help="print only differences and the summary")
    args = ap.parse_args(argv)

    try:
        with open(args.spec, encoding="utf-8") as f:
            specs = json.load(f)
    except (OSError, ValueError) as e:
        print(f"cannot read spec: {e}", file=sys.stderr)
        return 2
    if not isinstance(specs, list):
        print("spec must be a JSON list", file=sys.stderr)
        return 2
    unsafe = [s for s in specs if s.get("method", "GET").upper() not in SAFE_METHODS]
    if unsafe and not args.allow_writes:
        print(f"{len(unsafe)} request(s) are not GET/HEAD/OPTIONS; pass --allow-writes (never against production)",
              file=sys.stderr)
        return 2

    failed = allowed = ran = 0
    for n, spec in enumerate(specs):
        name = spec.get("name") or f"{spec.get('method', 'GET')} {spec['path']}"
        if args.only and name not in args.only:
            continue
        ran += 1
        rid = f"parity-{n}"
        a = send(args.base_a, spec, rid, args.timeout)
        b = send(args.base_b, spec, rid, args.timeout)
        diffs = compare(spec, a, b, args.raw)
        accepted = set(spec.get("allow_diff", []))
        hard = [d for d in diffs if d[0] not in accepted]
        soft = [d for d in diffs if d[0] in accepted]
        if hard:
            failed += 1
            print(f"DIFF  {name}")
            for _, msg in hard:
                print(f"  - {msg}")
        elif soft:
            allowed += 1
            print(f"ALLOW {name}  ({spec.get('reason', 'no reason given')})")
        elif not args.quiet:
            print(f"OK    {name}  [{a.status}]")
        if soft and hard:
            for _, msg in soft:
                print(f"  ~ allowed: {msg}")

    print(f"\n{ran} request(s): {ran - failed - allowed} identical, {allowed} allowed diff(s), {failed} differing")
    return 1 if failed else 0


if __name__ == "__main__":
    sys.exit(main())
