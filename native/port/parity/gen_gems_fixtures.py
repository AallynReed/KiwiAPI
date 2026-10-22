"""Golden fixtures for kiwi-server's /v1/gems routes, captured from Python.

Mounts ``gems_router`` on a bare FastAPI app with the real error handlers and
the ``gems:read`` dependency stubbed out, then records every case's status and
body. Random routes run after ``random.seed(n)``; the Rust test seeds its
CPython-compatible generator with the same ``n`` and must reproduce the bytes
(a clock-made ``id`` is masked). ``builds.json`` holds the pre-model output of
``calculate_builds`` (the route itself 500s) and ``star_chart.json`` decoded
codes. ``sum`` is CPython 3.13's (the server's), not this venv's.

Run from the repo root with the project venv:
    .venv/Scripts/python.exe native/port/parity/gen_gems_fixtures.py
"""

from __future__ import annotations

import base64
import builtins
import json
import math
import os
import random
import re
import sys
import warnings
from pathlib import Path

ROOT = Path(__file__).resolve().parents[3]
OUT = ROOT / "native" / "kiwi-server" / "testdata" / "gems"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
warnings.filterwarnings("ignore")

_py_sum = builtins.sum


def sum313(iterable, start=0):
    """CPython 3.12/3.13 ``sum``: Neumaier compensation for float terms only."""
    if not isinstance(start, (int, float)) or isinstance(start, bool):
        return _py_sum(iterable, start)
    it = iter(iterable)
    result = start
    if type(result) is int:
        for item in it:
            if type(item) is int or type(item) is bool:
                result += item
                continue
            result = result + item
            break
        else:
            return result
    if type(result) is not float:
        for item in it:
            result = result + item
        return result
    f, c = result, 0.0
    for item in it:
        if type(item) is float:
            t = f + item
            if abs(f) >= abs(item):
                c += (f - t) + item
            else:
                c += (item - t) + f
            f = t
        elif isinstance(item, int):
            f += float(item)
        else:
            f = f + c + item
            c = 0.0
    if c and math.isfinite(c):
        f += c
    return f


builtins.sum = sum313

import logging  # noqa: E402

logging.disable(logging.CRITICAL)

from fastapi import FastAPI  # noqa: E402
from fastapi.testclient import TestClient  # noqa: E402

import app.trove.router as R  # noqa: E402
from app.core.errors import register_error_handlers  # noqa: E402
from app.trove.gems import builds as gem_builds  # noqa: E402
from app.trove.gems.model import Gem  # noqa: E402

app = FastAPI()
register_error_handlers(app)
app.include_router(R.gems_router)
app.dependency_overrides[R._GEM.dependency] = lambda: None
client = TestClient(app, raise_server_exceptions=False)

ID = re.compile(r'"id":\d+')
CASES: list[dict] = []


def case(name, method, path, body=None, *, raw=None, seed=None, mask_id=False, content_type="application/json"):
    if seed is not None:
        random.seed(seed)
    headers = {}
    if raw is not None:
        content = raw.encode() if isinstance(raw, str) else raw
        headers["content-type"] = content_type
    elif body is not None:
        content = json.dumps(body).encode()
        headers["content-type"] = content_type
    else:
        content = None
    r = client.request(method, path, content=content, headers=headers)
    text = r.text
    if mask_id:
        text = ID.sub('"id":0', text)
    row = {"name": name, "method": method, "path": path, "status": r.status_code, "body": text}
    if content is not None:
        row["request"] = content.decode()
        row["content_type"] = content_type
    if seed is not None:
        row["seed"] = seed
    if mask_id:
        row["mask_id"] = True
    CASES.append(row)
    return r


def gem_dict(seed, **kw):
    random.seed(seed)
    g = Gem.create(**kw)
    return json.loads(g.model_dump_json())


def reference_cases():
    case("lookups", "GET", "/v1/gems/lookups")
    case("build_options", "GET", "/v1/gems/builds/options")
    for tier in range(1, 5):
        for t in (1, 2):
            case(f"level_costs_{tier}_{t}", "GET", f"/v1/gems/level-costs?tier={tier}&type={t}")
    for e in range(1, 5):
        case(f"level_costs_mystic_emp_{e}", "GET", f"/v1/gems/level-costs?tier=4&type=2&element={e}")
    for b in ("booster2", "booster4", "booster5", "booster5_mystic"):
        case(f"level_costs_{b}", "GET", f"/v1/gems/level-costs?tier=3&type=1&element=3&booster={b}&from_level=5")
    for q in ["", "?tier=4", "?tier=9&type=1", "?tier=4&type=3", "?tier=4&type=1&element=0",
              "?tier=4&type=1&booster=nope", "?tier=4&type=1&booster=%27x%22", "?tier=4&type=1&booster=",
              "?tier=x&type=y&from_level=0", "?tier=4&type=1&from_level=34", "?tier=4&type=1&from_level=99",
              "?tier=2&type=2&element=4&from_level=3&booster=booster4", "?tier=1.0&type=%201&element=+2"]:
        case(f"level_costs_q{q}", "GET", "/v1/gems/level-costs" + q)
    for tier in range(1, 5):
        for t in (1, 2):
            for st in range(1, 11):
                case(f"stat_range_{tier}_{t}_{st}", "GET",
                     f"/v1/gems/stat-range?tier={tier}&type={t}&stat_type={st}&level={5 * tier + t}&extra_containers={st % 4}")
    for q in ["", "?tier=4&type=1&stat_type=7&element=1", "?tier=4&type=2&stat_type=7&level=35&extra_containers=3&element=4",
              "?tier=4&type=1&stat_type=11", "?tier=0&type=1&stat_type=1", "?tier=4&type=1&stat_type=1&level=0&extra_containers=-1&element=x",
              "?tier=4&type=1&stat_type=1&level=1000", "?tier=1&type=1&stat_type=6&element=4&level=3"]:
        case(f"stat_range_q{q}", "GET", "/v1/gems/stat-range" + q)


def evaluator_cases():
    rng = random.Random(7)
    for i in range(60):
        tier, t = rng.randint(1, 4), rng.randint(1, 2)
        level = rng.choice([1, 5, 10, 15, 20, 25, 30, 35])
        cosmic = rng.random() < 0.3
        pool = [1, 3, 4, 5, 6] if rng.random() < 0.5 else [2, 3, 4, 5, 6]
        types = rng.sample(pool, 2 if cosmic else 3) + ([7] if cosmic else [])
        rng.shuffle(types)
        stats = []
        for st in types:
            r = case_range(tier, t, st, level, 7 if cosmic else 1)
            extra = rng.randint(0, 2)
            value = round(r[0] + (r[1] - r[0]) * rng.uniform(-0.2, 1.2), rng.choice([0, 1, 2, 3]))
            stats.append({"type": st, "value": value, "extra_containers": extra})
        body = {"tier": tier, "type": t, "level": level, "stats": stats, "auto_guess_procs": rng.random() < 0.5}
        case(f"evaluate_{i}", "POST", "/v1/gems/evaluate", body)
    bad = [
        {"stats": [{"type": 1, "value": 5000}]},
        {"stats": [{"type": 1, "value": 5000}, {"type": 1, "value": 5000}, {"type": 3, "value": 50}]},
        {"stats": [{"type": 1, "value": 5000}, {"type": 2, "value": 5000}, {"type": 3, "value": 50}]},
        {"stats": [{"type": 1, "value": 5000, "extra_containers": -1}, {"type": 4, "value": 5}, {"type": 3, "value": 50}]},
        {"tier": 9, "stats": [{"type": 1, "value": 5000}, {"type": 4, "value": 5}, {"type": 3, "value": 50}]},
        {"type": 0, "stats": [{"type": 1, "value": 5000}, {"type": 4, "value": 5}, {"type": 3, "value": 50}]},
        {"stats": [{"type": 8, "value": 5000}, {"type": 4, "value": 5}, {"type": 3, "value": 50}]},
        {"stats": [{"type": 11, "value": 5000}, {"type": 4, "value": 5}, {"type": 3, "value": 50}]},
        {"stats": [{"type": 1, "value": 5000, "extra_containers": 3}, {"type": 4, "value": 5}, {"type": 3, "value": 50}], "level": 35},
        {"stats": [{"type": 1, "value": 0}, {"type": 4, "value": 0}, {"type": 3, "value": 0}], "level": 15},
        {"stats": [{"type": 10, "value": 2000}, {"type": 4, "value": 5}, {"type": 3, "value": 50}], "level": 20, "auto_guess_procs": True},
        {"stats": [{"type": 1, "value": 1e9}, {"type": 4, "value": -5}, {"type": 3, "value": 50}], "level": -3},
        {"stats": "x"},
        {"stats": [{"value": "a"}, 5]},
        {},
    ]
    for i, b in enumerate(bad):
        case(f"evaluate_bad_{i}", "POST", "/v1/gems/evaluate", b)
    for tier in range(1, 5):
        for t in (1, 2):
            for level in (1, 5, 15, 40):
                for pr in (0, 600, 1200, 2000):
                    case(f"simple_{tier}_{t}_{level}_{pr}", "POST", "/v1/gems/evaluate-simple",
                         {"tier": tier, "type": t, "level": level, "power_rank": pr})
    for i, b in enumerate([{}, {"tier": 7}, {"type": "x"}, {"level": -5}, {"power_rank": 1.5}, [1]]):
        case(f"simple_bad_{i}", "POST", "/v1/gems/evaluate-simple", b)


def case_range(tier, t, st, level, element):
    from app.trove.gems import evaluator
    try:
        r = evaluator.gem_stat_range(tier, t, st, level, 0, 4 if st == 7 else 1)
        return r["min_value"], r["max_value"]
    except ValueError:
        return 0.0, 100.0


def chart_codes(parser):
    rng = random.Random(11)
    codes = ["", "SC:", "SC:AAAA", "v2:Zz9-_w", "SC:c|ca|ca0|ca0a|ca0a0|pz|q1", "SC:ca0a0|ga1b", "Y29tYmF0LmE=",
             "not base64!!", "SC:A", "SC:AB=C", "été", "  SC:CA0A0  "]
    sel = parser.selectable_paths
    for _ in range(12):
        ids = rng.sample(range(len(sel)), rng.randint(1, 25))
        codes.append("SC:" + base64.urlsafe_b64encode(bytes(ids)).decode().rstrip("="))
    for _ in range(4):
        ps = rng.sample(sel, rng.randint(1, 8))
        codes.append(base64.b64encode("$".join(ps).encode()).decode())
    bounty = list(parser.bounty_nodes)
    for p in bounty:
        codes.append("SC:" + base64.urlsafe_b64encode(bytes([sel.index(p)])).decode())
    return codes


def builds_and_health(codes):
    configs = [
        {}, {"build_type": "Farm", "light": 6000}, {"build_type": "Health"}, {"high_precision": True},
        {"build_type": "Light", "character": "Knight", "subclass": "Lunar Lancer", "food": "fishing_phys_3_base",
         "ally": "goat_pinata_moontouched", "berserker_battler": True, "litany": True},
        {"character": "Ice Sage", "subclass": "Bard", "subclass_active": True, "ally_buff": False, "critical_damage_count": 1,
         "no_face": True, "ally": "phoenix_stars"},
        {"character": "Solarion", "subclass": "Gunslinger", "subclass_active": True, "build_type": "Farm", "light": 4500},
        {"character": "Nope"}, {"subclass": "Nope"}, {"critical_damage_count": 4}, {"build_type": "Xyz"},
    ]
    for code in codes[:30]:
        configs.append({"character": "Shadow Hunter", "subclass": "Candy Barbarian", "subclass_active": True,
                        "star_chart": code, "bounty_hunt": True})
    rows = []
    for i, cfg in enumerate(configs):
        case(f"builds_{i}", "POST", "/v1/gems/builds/calculate", cfg)
        full = R.BuildConfigRequest(**{k: v for k, v in cfg.items() if k != "critical_damage_count" or v <= 3}).model_dump()
        try:
            out = gem_builds.calculate_builds(full)
            rows.append({"config": full, "count": len(out), "results": out[:25]})
        except gem_builds.BuildError as e:
            rows.append({"config": full, "error": str(e)})
    hcfgs = [{}, {"character": "Knight", "weapon_ch": True, "ring_ch": True, "hat_health": True, "face_health": True},
             {"character": "x"}, {"high_precision": True}, {"character": "Revenant", "hat_health": True}]
    for code in codes[-3:]:
        hcfgs.append({"character": "Revenant", "star_chart": code, "weapon_ch": True})
    for i, cfg in enumerate(hcfgs):
        case(f"health_{i}", "POST", "/v1/gems/builds/health-optimizer", cfg)
    return rows


def simulator_cases():
    for i in range(40):
        rng = random.Random(100 + i)
        body = {}
        for k, lo, hi in (("tier", 0, 4), ("type", 0, 2), ("element", 0, 4), ("restriction", 0, 2)):
            if rng.random() < 0.6:
                body[k] = rng.randint(lo, hi)
        if rng.random() < 0.5:
            body["level"] = rng.choice([1, 5, 10, 15, 20, 35, 50, -1])
        if rng.random() < 0.3:
            body["augmentation"] = rng.choice([0.5, 1.0, 0.0, 0.123])
        if rng.random() < 0.3:
            body["retired_stats"] = True
        case(f"generate_{i}", "POST", "/v1/gems/generate", body, seed=1000 + i, mask_id=True)
    for i, b in enumerate([{"tier": 5}, {"type": 3}, {"element": -1}, {"type": 1, "restriction": 3}, {"tier": "x", "level": "y"}]):
        case(f"generate_bad_{i}", "POST", "/v1/gems/generate", b, seed=1)

    gems = []
    for i in range(16):
        rng = random.Random(500 + i)
        g = gem_dict(600 + i, tier=rng.randint(1, 4), type=rng.randint(1, 2), element=rng.randint(1, 4),
                     level=rng.choice([1, 5, 10, 15, 20, 30]), retired_stats=rng.random() < 0.2)
        if i % 5 == 4:
            for s in g["stats"]:
                for c in s["containers"]:
                    c.pop("base", None)
            g.pop("id")
        if i % 7 == 3:
            g["stats"][0]["containers"][0]["augments"] = [{"type": 2, "count": 2}]
        gems.append(g)
    for i, g in enumerate(gems):
        mask = "id" not in g
        for pos in (0, 1, 2):
            aug = (i + pos) % 3 + 1
            case(f"augment_{i}_{pos}_{aug}", "POST", "/v1/gems/augment",
                 {"gem": g, "stat_position": pos, "augment_type": aug}, seed=i, mask_id=mask)
            case(f"spark_{i}_{pos}", "POST", "/v1/gems/spark", {"gem": g, "stat_position": pos}, seed=2000 + i * 3 + pos, mask_id=mask)
            case(f"flare_{i}_{pos}", "POST", "/v1/gems/flare", {"gem": g, "stat_position": pos}, seed=3000 + i * 3 + pos, mask_id=mask)
        for b, booster in enumerate([None, "booster4", "booster5_mystic" if i % 2 else "", "booster2"][: 2 + i % 3]):
            case(f"level_up_{i}_{b}", "POST", "/v1/gems/level-up", {"gem": g, "booster": booster}, seed=4000 + i * 7 + b, mask_id=mask)
        for lv in (1, 12, 40) if i % 2 else (5, 16):
            case(f"set_level_{i}_{lv}", "POST", "/v1/gems/set-level", {"gem": g, "level": lv}, seed=5000 + i * 5 + lv, mask_id=mask)
    g = gems[0]
    karma = dict(g, karma=124, level=20)
    for s in range(6):
        case(f"level_up_karma_{s}", "POST", "/v1/gems/level-up", {"gem": karma, "booster": None}, seed=s)
    for s in range(4):
        case(f"level_up_max_{s}", "POST", "/v1/gems/level-up", {"gem": dict(g, level=g["level"] + 100), "booster": "nope"}, seed=s)
    one = dict(g, stats=g["stats"][:1])
    two = dict(g, stats=[dict(g["stats"][0], containers=g["stats"][0]["containers"] * 3), g["stats"][1]])
    weird = dict(g, stats=[{"type": 8, "containers": [{"base": 0.5}]}, g["stats"][1], g["stats"][2]])
    edge = [
        ("augment", {"gem": g, "stat_position": 3}), ("augment", {"gem": g, "stat_position": 0, "augment_type": 9}),
        ("augment", {"gem": g, "stat_position": -1}), ("augment", {"gem": weird, "stat_position": 1}),
        ("spark", {"gem": g, "stat_position": 5}), ("flare", {"gem": one, "stat_position": 0}),
        ("flare", {"gem": dict(one, stats=one["stats"] * 1)}), ("flare", {"gem": dict(g, stats=[dict(g["stats"][0], containers=g["stats"][0]["containers"] * 2)]), "stat_position": 0}),
        ("set-level", {"gem": dict(g, stats=[]), "level": 30}), ("set-level", {"gem": two, "level": 1}),
        ("set-level", {"gem": g, "level": 0}), ("level-up", {"gem": dict(one, level=4), "booster": None}),
        ("level-up", {"gem": g, "booster": "nope"}), ("augment", {"gem": dict(g, tier="9")}),
        ("augment", {"gem": dict(g, tier="4", restriction=True, spent={"a": "x"}), "stat_position": "0"}),
        ("spark", {"gem": 5, "stat_position": 0}), ("spark", [1]),
    ]
    for i, (route, b) in enumerate(edge):
        case(f"edge_{i}_{route}", "POST", f"/v1/gems/{route}", b, seed=77 + i, mask_id="id" not in b.get("gem", {}) if isinstance(b, dict) and isinstance(b.get("gem"), dict) else False)
    case("raw_bad_json", "POST", "/v1/gems/augment", raw='{"gem": ')
    case("raw_empty", "POST", "/v1/gems/augment", raw="")
    case("raw_null", "POST", "/v1/gems/augment", raw="null")
    case("raw_text_plain", "POST", "/v1/gems/evaluate-simple", raw="{}", content_type="text/plain")


def star_chart_rows(codes):
    out = []
    for code in codes:
        r = gem_builds.parse_star_chart(code)
        out.append({"code": code, "stats": r["stats"], "bounty_hunt": r["bounty_hunt"], "paths_count": r["paths_count"]})
    return out


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    parser = gem_builds._engine().star_parser
    codes = chart_codes(parser)
    reference_cases()
    evaluator_cases()
    rows = builds_and_health(codes)
    simulator_cases()
    (OUT / "routes.json").write_text(json.dumps(CASES, indent=0, ensure_ascii=False) + "\n", encoding="utf-8")
    (OUT / "builds.json").write_text(json.dumps(rows, indent=0) + "\n", encoding="utf-8")
    (OUT / "star_chart.json").write_text(json.dumps(star_chart_rows(codes), indent=0, ensure_ascii=False) + "\n",
                                         encoding="utf-8")
    print(f"{len(CASES)} route cases, {len(rows)} build configs, {len(codes)} chart codes -> {OUT}")


if __name__ == "__main__":
    main()
