"""Base OpenAPI spec behind kiwi-server's /openapi.json, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_openapi.py [--check]
Refresh: rerun this and commit the outputs whenever a Python route, model or docstring changes (--check exits 1 when stale).

Writes native/kiwi-server/src/openapi/{base.json,ops.json} (FastAPI's get_openapi output before
custom_openapi's markers, plus the tokenless / master / feature-gated operation sets), and
native/kiwi-server/testdata/openapi/{routes.json,golden.json} (py_routes.py's table the spec was
built from, and sha256 of the real /openapi.json body for many feature-flag combinations).
"""
import asyncio
import hashlib
import json
import logging
import os
import random
import subprocess
import sys
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())

NATIVE = Path(__file__).resolve().parents[2]
SRC = NATIVE / "kiwi-server" / "src" / "openapi"
TESTDATA = NATIVE / "kiwi-server" / "testdata" / "openapi"
GOLDEN_ENV = {"APP_NAME": 'Kiwi <API> "é" \\ {x}', "API_URL": "https://api.example.test/ü?a=1&b=2"}


def dump(v) -> bytes:
    return (json.dumps(v, ensure_ascii=False, indent=1) + "\n").encode()


def route_table():
    script = Path(__file__).with_name("py_routes.py")
    return json.loads(subprocess.run([sys.executable, str(script)], check=True, stdout=subprocess.PIPE).stdout)


def base_outputs() -> dict[Path, bytes]:
    for k in GOLDEN_ENV:
        os.environ.pop(k, None)
    os.environ.setdefault("SECRET_KEY", "x")
    import app.main as m
    from fastapi.openapi.utils import get_openapi

    app = m.app
    raw = get_openapi(title=app.title, version=app.version, openapi_version=app.openapi_version,
                      description=app.description, routes=app.routes, tags=app.openapi_tags,
                      servers=app.servers)
    ops = {
        "tokenless": sorted(map(list, m._ops_with_marker("_tokenless_scope"))),
        "master": sorted(map(list, m._ops_with_marker("_master_only"))),
        "gated": [[p, meth, flag] for (p, meth), flag in m._feature_gated_ops().items()],
    }
    return {
        SRC / "base.json": dump(raw),
        SRC / "ops.json": dump(ops),
        TESTDATA / "routes.json": dump(route_table()),
    }


def golden() -> bytes:
    os.environ.update(GOLDEN_ENV)
    os.environ.setdefault("SECRET_KEY", "x")
    import app.main as m
    from app.admin import runtime_config as rc

    flags = sorted(set(m._feature_gated_ops().values()))
    combos = [("defaults", None), ("all_on", {f: True for f in flags}), ("all_off", {f: False for f in flags})]
    for f in flags:
        combos.append((f"only_off:{f}", {g: g != f for g in flags}))
        combos.append((f"only_on:{f}", {g: g == f for g in flags}))
    rng = random.Random(20261023)
    for i in range(24):
        combos.append((f"mixed_{i:02d}", {f: rng.random() < 0.5 for f in flags}))
    # Non-bool stored values go through bool() like any other flag.
    combos.append(("truthy_values", {f: v for f, v in zip(flags, [1, 0, "", "x", [], [0], None, 0.0] * 9)}))

    cases = []
    for name, values in combos:
        rc._CACHE.clear()
        for f in flags:
            if values is None:
                rc._CACHE[f] = (rc._spec(f).default, float("inf"))
            else:
                rc._CACHE[f] = (values[f], float("inf"))
        body = asyncio.run(m.openapi_json()).body
        cases.append({
            "name": name,
            "disabled": [f for f in flags if not (rc._spec(f).default if values is None else values[f])],
            "len": len(body),
            "sha256": hashlib.sha256(body).hexdigest(),
        })
    marked = json.dumps(m.custom_openapi(), ensure_ascii=False, allow_nan=False, separators=(",", ":")).encode()
    return dump({"env": GOLDEN_ENV, "flags": flags,
                 "marked_sha256": hashlib.sha256(marked).hexdigest(), "cases": cases})


def main() -> int:
    if "--golden" in sys.argv:
        sys.stdout.buffer.write(golden())
        return 0
    outputs = base_outputs()
    env = {k: v for k, v in os.environ.items() if k not in GOLDEN_ENV}
    proc = subprocess.run([sys.executable, __file__, "--golden"], env=env, check=True, stdout=subprocess.PIPE)
    outputs[TESTDATA / "golden.json"] = proc.stdout
    if "--check" in sys.argv:
        stale = [p for p, data in outputs.items()
                 if not p.is_file() or p.read_bytes().replace(b"\r\n", b"\n") != data]
        for p in stale:
            print(f"stale: {p.relative_to(NATIVE)}")
        return 1 if stale else 0
    for p, data in outputs.items():
        p.parent.mkdir(parents=True, exist_ok=True)
        p.write_bytes(data)
        print(f"wrote {p.relative_to(NATIVE)} ({len(data)} bytes)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
