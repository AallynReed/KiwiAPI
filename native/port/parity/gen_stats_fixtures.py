"""Golden fixtures for kiwi-server's /v1/stats routes, captured from Python.

Mounts the real ``stats_router`` (scope dependencies overridden, the real error
handlers and request-id middleware) and records what it answers for the repo
baseline and for hand-made gamedata directories (``store.BASELINE_DIR`` and the
stats dir pointed at each case). PNG bodies are recorded as the SHA-256 of
their decoded RGBA pixels; ``sigil.png_sha256`` pins each sigil's bytes as
Pillow on Linux writes them (hence linux_png.py).

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_stats_fixtures.py
Writes native/kiwi-server/testdata/stats/golden.json.
"""

from __future__ import annotations

import base64
import hashlib
import io
import json
import logging
import os
import struct
import sys
import tempfile
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "stats" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402
from PIL import Image  # noqa: E402

from app.core.config import settings  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.core.observability import add_request_context_middleware  # noqa: E402
from app.trove import router as trove_router  # noqa: E402
from app.trove import stats  # noqa: E402
from app.trove.decode import store  # noqa: E402

RID = "fixture-rid"

app = FastAPI()
register_error_handlers(app)
add_request_context_middleware(app)
app.include_router(trove_router.stats_router)
app.dependency_overrides[trove_router._STAT.dependency] = lambda: None
app.dependency_overrides[trove_router._STAT_PUBLIC.dependency] = lambda: None
client = TestClient(app, raise_server_exceptions=False)


def pixels_sha(png: bytes) -> str:
    return hashlib.sha256(Image.open(io.BytesIO(png)).convert("RGBA").tobytes()).hexdigest()


def call(method: str, path: str, body: bytes | None = None, headers: dict | None = None) -> dict:
    h = {"X-Request-ID": RID, **(headers or {})}
    r = client.request(method, path, content=body, headers=h)
    out = {"status": r.status_code, "content_type": r.headers.get("content-type"),
           "cache_control": r.headers.get("cache-control")}
    if r.headers.get("content-type") == "image/png":
        out["pixels"] = pixels_sha(r.content)
    elif len(r.content) > 20000:
        out["body_sha256"] = hashlib.sha256(r.content).hexdigest()
    else:
        out["body"] = r.content.decode("utf-8")
    return out


def png_b64(img: Image.Image, **save) -> str:
    buf = io.BytesIO()
    img.save(buf, format="PNG", **save)
    return base64.b64encode(buf.getvalue()).decode()


def gradient(mode: str, size=(8, 8)) -> Image.Image:
    w, h = size
    rgba = Image.new("RGBA", size)
    rgba.putdata([((x * 37 + y * 11) % 256, (x * 5 + y * 53) % 256, (x * y * 7 + 9) % 256,
                   (x * 31 + y * 17 + 3) % 256) for y in range(h) for x in range(w)])
    return rgba if mode == "RGBA" else rgba.convert(mode)


# --- baseline ---------------------------------------------------------------

def baseline_section() -> dict:
    out = {"tables": {}, "classes": {}}
    for key in stats.STAT_TABLE_KEYS:
        out["tables"][key] = call("GET", f"/v1/stats/{key}")
    out["list"] = call("GET", "/v1/stats/classes")
    techs = [c["tech_name"] for c in stats.all_classes()["items"]]
    for raw in techs + ["nope", "KNIGHT", "%E2%9C%93", "%FF", "a%3Fb", "%2E%2E", "knight%20"]:
        out["classes"][raw] = call("GET", f"/v1/stats/classes/{raw}")
    return out


COEFFICIENT_BODIES = [
    {"physical_damage": 100, "magic_damage": 50, "critical_damage": 100},
    {"physical_damage": 50, "magic_damage": 100.5, "critical_damage": 3438.3},
    {"physical_damage": 7, "magic_damage": 7, "critical_damage": 0},
    {"magic_damage": 123456.789, "critical_damage": 2500},
    {"physical_damage": 0.1, "critical_damage": 0.2},
    {"physical_damage": 1e20, "critical_damage": 0},
    {"physical_damage": 1.7e308, "critical_damage": 0},
    {"physical_damage": 1.7e308, "critical_damage": 100},
    {"physical_damage": 12.5, "magic_damage": None, "critical_damage": 99.99},
    {"physical_damage": "42", "critical_damage": "10.5"},
    {"physical_damage": True, "critical_damage": False},
    {"physical_damage": -0.0, "critical_damage": 0},
    {"critical_damage": 100},
    {"physical_damage": 100},
    {"physical_damage": -1, "magic_damage": -2, "critical_damage": -3},
    {"physical_damage": "abc", "critical_damage": None},
    {"physical_damage": [1], "magic_damage": {}, "critical_damage": "1e3"},
    {"physical_damage": 10, "critical_damage": 10, "extra": "ignored"},
    {"physical_damage": 2.675, "critical_damage": 0.0001},
    [],
    "text",
    123,
]


def coefficient_section() -> list:
    cases = []
    for b in COEFFICIENT_BODIES:
        raw = json.dumps(b).encode()
        cases.append({"ctype": "application/json", "request": raw.decode(), **call("POST", "/v1/stats/coefficient", raw, {"Content-Type": "application/json"})})
    raws = [
        ("application/json", "{"),
        ("application/json", ""),
        ("application/json", "null"),
        ("text/plain", '{"physical_damage": 1, "critical_damage": 1}'),
        (None, '{"physical_damage": 1, "critical_damage": 1}'),
        ("application/vnd.api+json", '{"physical_damage": 3, "critical_damage": 1}'),
    ]
    for ctype, text in raws:
        headers = {"Content-Type": ctype} if ctype else {}
        cases.append({"ctype": ctype, "request": text, **call("POST", "/v1/stats/coefficient", text.encode(), headers)})
    return cases


SIGIL_QUERIES = [
    "power_rank=0&mastery=0", "power_rank=249&mastery=9", "power_rank=250&mastery=10",
    "power_rank=40000&mastery=500", "power_rank=65000&mastery=1100", "power_rank=10000000&mastery=100000",
    "power_rank=31337&mastery=777", "power_rank=1_000&mastery=+5", "power_rank=%2012%20&mastery=1.0",
    "", "power_rank=5", "mastery=5", "power_rank=-1&mastery=0", "power_rank=10000001&mastery=100001",
    "power_rank=abc&mastery=1.5", "power_rank=&mastery=", "power_rank=1&power_rank=2&mastery=3",
]


def sigil_section() -> dict:
    data = store.load("insignia.json")
    tiers, png = {}, {}
    for s in range(len(data["power_rank"])):
        for w in range(len(data["mastery"])):
            tiers[f"{s},{w}"] = pixels_sha(stats._sigil(s, w))
            png[f"{s},{w}"] = hashlib.sha256(stats._sigil(s, w)).hexdigest()
    return {"queries": {q: call("GET", f"/v1/stats/sigil?{q}") for q in SIGIL_QUERIES}, "tiers": tiers,
            "png_sha256": png}


def composite_section() -> str:
    dst, src = bytearray(), bytearray()
    for i in range(65536):
        dst += bytes([(i * 7 + 3) & 255, (i * 13 + 5) & 255, (i * 29 + 11) & 255, i >> 8])
        src += bytes([(i * 31 + 17) & 255, (i * 37 + 19) & 255, (i * 41 + 23) & 255, i & 255])
    a = Image.frombytes("RGBA", (256, 256), bytes(dst))
    b = Image.frombytes("RGBA", (256, 256), bytes(src))
    return hashlib.sha256(Image.alpha_composite(a, b).tobytes()).hexdigest()


def decode_section() -> list:
    imgs = [
        ("RGBA", png_b64(gradient("RGBA"))),
        ("RGB", png_b64(gradient("RGB"))),
        ("RGB+trns", png_b64(gradient("RGB"), transparency=gradient("RGB").getpixel((1, 0)))),
        ("L", png_b64(gradient("L"))),
        ("L+trns", png_b64(gradient("L"), transparency=gradient("L").getpixel((2, 1)))),
        ("LA", png_b64(gradient("LA"))),
        ("1", png_b64(gradient("1"))),
        ("P", png_b64(gradient("RGB").convert("P", palette=Image.ADAPTIVE, colors=16))),
        ("P+trns", png_b64(gradient("RGB").convert("P", palette=Image.ADAPTIVE, colors=16), transparency=3)),
        ("P-rgba", png_b64(gradient("RGBA").convert("P"))),
        ("P-4bit", png_b64(gradient("RGB").convert("P", palette=Image.ADAPTIVE, colors=4), bits=2)),
    ]
    out = []
    for name, b64 in imgs:
        im = Image.open(io.BytesIO(base64.b64decode(b64)))
        out.append({"name": name, "mode": im.mode, "png_b64": b64, "rgba_hex": im.convert("RGBA").tobytes().hex()})
    return out


# --- hand-made gamedata -----------------------------------------------------

def stat_rows(**kw):
    row = {"name": "Dragons", "type": "slider", "percentage": False, "value": 2130}
    row.update(kw)
    return row


def klass(tech: str, name: str, **kw) -> dict:
    c = {
        "qualified_name": tech, "name": name, "shorts": ["X"], "damage_type": "Physical",
        "weapons": ["Sword"], "attributes": ["Melee"],
        "stats": [{"name": "Physical Damage", "value": 1950, "percentage": False},
                  {"name": "Critical Hit", "value": 2.5, "percentage": True, "absolute": True},
                  {"name": "Nothing", "value": None, "percentage": False}],
        "bonuses": [{"name": "Attack Speed", "value": 10, "percentage": True}],
        "levels": {"2": [{"name": "Maximum Health", "value": 10}]},
        "subclass": {"name": "Sub", "description": "Desc", "level": {"1": [{"name": "Light", "value": 1}]},
                     "power": {"10": "Something", "20": ""}},
    }
    c.update(kw)
    return c


ABILITY = {"name": "Slash", "description": "Hits", "icon": "slash.png", "type": "Active", "prefab": "abilities/slash",
           "active": True, "energy": 10, "cooldown": 1.5, "healing": [], "effects": ["burn"], "vfx": [],
           "stages": [{"name": "Hit", "base": 100, "multiplier": 1.25, "prefab": "s1", "knockback": 3},
                      {"name": "Hit 2", "multiplier": 0}, {}]}


def insignia(pr_imgs, m_imgs, pr_from=None, m_from=None) -> dict:
    pr_from = pr_from or [i * 100 for i in range(len(pr_imgs))]
    m_from = m_from or [i * 10 for i in range(len(m_imgs))]
    return {"size": [8, 8],
            "mastery": [{"from": f, "image": im} for f, im in zip(m_from, m_imgs)],
            "power_rank": [{"from": f, "image": im} for f, im in zip(pr_from, pr_imgs)]}


def j(v) -> str:
    return json.dumps(v, ensure_ascii=False)


RGBA_A = png_b64(gradient("RGBA"))
RGBA_B = png_b64(gradient("RGBA").rotate(90))
GRAY = png_b64(gradient("LA"))
PAL = png_b64(gradient("RGB").convert("P", palette=Image.ADAPTIVE, colors=16), transparency=3)
BIG = png_b64(gradient("RGBA", (9, 8)))

CASES = [
    ("plain", {"classes.json": j([klass("knight", "Knight"), klass("bard", "Bard ✓ \"q\"\n")]),
               "class_abilities.json": j([{"name": "Knight", "abilities": [ABILITY]}])}),
    ("own_abilities", {"classes.json": j([klass("knight", "Knight", abilities=[dict(ABILITY, name="Own")])]),
                       "class_abilities.json": j([{"name": "Knight", "abilities": [ABILITY]}])}),
    ("duplicate", {"classes.json": j([klass("a", "A"), klass("b", "B"), klass("a", "A2")])}),
    ("one_invalid", {"classes.json": j([klass("ok", "Ok"), klass("bad", "Bad", damage_type=5)])}),
    ("power_not_str", {"classes.json": j([klass("p", "P", subclass={"power": {"1": 2}}), klass("q", "Q")])}),
    ("stat_no_name", {"classes.json": j([klass("x", "X", stats=[{"value": 1}])])}),
    ("levels_list", {"classes.json": j([klass("x", "X", levels=[])])}),
    ("levels_null", {"classes.json": j([klass("x", "X", levels=None)])}),
    ("subclass_list", {"classes.json": j([klass("x", "X", subclass=[])])}),
    ("no_tech", {"classes.json": j([{"name": "X"}])}),
    ("tech_int", {"classes.json": j([klass(5, "Five"), klass("y", "Y")])}),
    ("tech_list", {"classes.json": j([klass(["a"], "L")])}),
    ("lax_types", {"classes.json": j([klass("x", "X", stats=[{"name": "S", "value": "1.5", "percentage": 1},
                                                                {"name": "T", "value": True, "percentage": "yes",
                                                                 "absolute": 0}])])}),
    ("float_shapes", {"classes.json": j([klass("x", "X", stats=[{"name": n, "value": v, "percentage": False}
                                                                   for n, v in [("a", 1e16), ("b", 1e-7), ("c", -0.0),
                                                                                ("d", 123456789012345678),
                                                                                ("e", 0.1), ("f", 1.5e300)]])])}),
    ("str_value_bad", {"classes.json": j([klass("x", "X", stats=[{"name": "S", "value": "abc", "percentage": False}]),
                                          klass("y", "Y")])}),
    ("abilities_bad_stage", {"classes.json": j([klass("x", "X", abilities=[dict(ABILITY, stages=[[1]])])])}),
    ("abilities_str", {"classes.json": j([klass("x", "X", abilities="")]),
                       "class_abilities.json": j([{"name": "X", "abilities": "abc"}])}),
    ("fallback_bad", {"classes.json": j([klass("x", "X")]),
                      "class_abilities.json": j([{"name": "X", "abilities": [5]}])}),
    ("fallback_other_bad", {"classes.json": j([klass("x", "X")]),
                            "class_abilities.json": j([{"name": "Y", "abilities": [5]}])}),
    ("ca_no_name", {"classes.json": j([klass("x", "X")]), "class_abilities.json": j([{"abilities": []}])}),
    ("ca_broken_json", {"classes.json": j([klass("x", "X")]), "class_abilities.json": "{"}),
    ("ca_null", {"classes.json": j([klass("x", "X")]), "class_abilities.json": "null"}),
    ("classes_obj", {"classes.json": "{}"}),
    ("classes_obj_full", {"classes.json": '{"a": 1}'}),
    ("classes_null", {"classes.json": "null"}),
    ("classes_empty_str", {"classes.json": '""'}),
    ("classes_bom", {"classes.json": "﻿[]"}),
    ("missing", {}),
    ("sigil_ok", {"insignia.json": j(insignia([RGBA_A, RGBA_B], [RGBA_B, RGBA_A]))}),
    ("sigil_modes", {"insignia.json": j(insignia([PAL, GRAY], [GRAY, PAL]))}),
    ("sigil_from_float_bool", {"insignia.json": j(insignia([RGBA_A, RGBA_B], [RGBA_B, RGBA_A],
                                                            pr_from=[False, 99.5], m_from=[True, 2]))}),
    ("sigil_size_mismatch", {"insignia.json": j(insignia([BIG], [RGBA_A]))}),
    ("sigil_bad_b64", {"insignia.json": j(insignia(["!!!" + RGBA_A[:10]], [RGBA_A]))}),
    ("sigil_noise_b64", {"insignia.json": j(insignia([RGBA_A[:40] + "\n*" + RGBA_A[40:]], [RGBA_A]))}),
    ("sigil_not_png", {"insignia.json": j(insignia([base64.b64encode(b"hello").decode()], [RGBA_A]))}),
    ("sigil_high_from", {"insignia.json": j(insignia([RGBA_A], [RGBA_A], pr_from=[5]))}),
    ("sigil_from_str", {"insignia.json": j(insignia([RGBA_A], [RGBA_A], pr_from=["0"]))}),
    ("sigil_empty_obj", {"insignia.json": "{}"}),
    ("sigil_list", {"insignia.json": "[1]"}),
    ("sigil_zero", {"insignia.json": "0"}),
    ("sigil_no_mastery", {"insignia.json": j({"power_rank": [{"from": 0, "image": RGBA_A}]})}),
    ("sigil_broken", {"insignia.json": "[1,"}),
]

CASE_REQUESTS = ["/v1/stats/classes", "/v1/stats/classes/knight", "/v1/stats/classes/a", "/v1/stats/classes/b",
                 "/v1/stats/classes/ok", "/v1/stats/classes/bad", "/v1/stats/classes/p", "/v1/stats/classes/q",
                 "/v1/stats/classes/x", "/v1/stats/classes/y", "/v1/stats/classes/5",
                 "/v1/stats/sigil?power_rank=0&mastery=0", "/v1/stats/sigil?power_rank=150&mastery=15",
                 "/v1/stats/sigil?power_rank=99&mastery=1"]

TABLES = [
    ("ints", [stat_rows(), stat_rows(name="Hat", value=2324, type="switch")]),
    ("light", [{"name": "Base", "type": "slider", "percentage": False, "value": 2.5, "step": 2.5, "perm": True},
               {"name": "Food", "value": 300, "step": 10, "perm": False}, {"name": "X", "value": 1, "perm": 1}]),
    ("lax", [stat_rows(value="12.5", percentage="true"), stat_rows(value=False, step="2")]),
    ("bad_type", [stat_rows(type=None)]),
    ("no_value", [{"name": "x"}]),
    ("not_list", {"a": 1}),
    ("empty_obj", {}),
    ("unicode", [stat_rows(name="Drågöns ✓  ")]),
]


def cases_section() -> tuple[list, list]:
    out = []
    root = Path(tempfile.mkdtemp(prefix="kiwi-stats-gen-"))
    for name, files in CASES:
        d = root / name
        d.mkdir()
        for fname, text in files.items():
            (d / fname).write_text(text, encoding="utf-8", newline="")
        store.BASELINE_DIR = d
        settings.gamedata_dir = ""
        wanted = [p for p in CASE_REQUESTS if not files
                  or ("sigil" in p) == ("insignia.json" in files)]
        results = {p: call("GET", p) for p in wanted}
        out.append({"name": name, "files": files, "results": results})
    # A runtime rebuild wins over the baseline, file by file.
    base, rt = root / "override_base", root / "override_rt"
    base.mkdir()
    rt.mkdir()
    base_files = {"classes.json": j([klass("x", "Base")]), "class_abilities.json": j([{"name": "Base",
                                                                                        "abilities": [ABILITY]}])}
    rt_files = {"classes.json": j([klass("x", "Runtime")])}
    for fname, text in base_files.items():
        (base / fname).write_text(text, encoding="utf-8")
    for fname, text in rt_files.items():
        (rt / fname).write_text(text, encoding="utf-8")
    store.BASELINE_DIR = base
    settings.gamedata_dir = str(rt)
    override = {"base": base_files, "runtime": rt_files, "results": {p: call("GET", p) for p in CASE_REQUESTS[:3]}}
    out.append({"name": "override", "files": {}, **override})

    tables = []
    for name, rows in TABLES:
        d = root / ("table_" + name)
        d.mkdir()
        (d / "power_rank.json").write_text(json.dumps(rows, ensure_ascii=False), encoding="utf-8")
        stats._STATS_DIR = d
        stats._read_json.cache_clear()
        tables.append({"name": name, "file": json.dumps(rows, ensure_ascii=False), **call("GET", "/v1/stats/power-rank")})
    return out, tables


def int_digits_section() -> list:
    vals = [0.0, -0.0, 0.5, -0.5, 1.9999999, 9.007199254740993e15, 9.2e18, 9.3e18, 1.8446744073709552e19, 1e20,
            -1e20, 2 ** 70, 1.7976931348623157e308, -1.7976931348623157e308, 123456789.987654321, 5e-324]
    return [[struct.pack(">d", float(v)).hex(), str(int(float(v)))] for v in vals]


def main() -> None:
    base = baseline_section()
    coefficient = coefficient_section()
    sigil = sigil_section()
    cases, tables = cases_section()
    doc = {
        "rid": RID,
        "baseline": base,
        "coefficient": coefficient,
        "sigil": sigil,
        "composite_sha256": composite_section(),
        "decode": decode_section(),
        "int_digits": int_digits_section(),
        "cases": cases,
        "tables": tables,
        "boards": {"order": list(stats._BOARD_CLASS_ORDER), "board_uuids": stats.class_board_uuids(),
                   "index_for": [[u, stats.class_index_for_board(u)] for u in (1000, 1017, 4005, 5017, -3, 21012)]},
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(doc, ensure_ascii=False, indent=1) + "\n", encoding="utf-8", newline="\n")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
