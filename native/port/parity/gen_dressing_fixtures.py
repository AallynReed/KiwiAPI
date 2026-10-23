"""Golden fixtures for the Dressing Room, the embed data plane and creature
assembly, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/linux_png.py run native/port/parity/gen_dressing_fixtures.py

Writes native/kiwi-server/testdata/dressing/{engine,routes}.json.

The class, skin, equipment and custom-head prefabs and the locale strings come
from a local game extract (KIWI_TROVE_DIR, default E:/Trove); the blueprints
are small synthetic ones; the rig map is the skins' own `extract_rig_refs`.
The archive, rig map, Redis and Postgres are stubbed with those tables, which
the Rust tests install through their test hooks.
"""
import asyncio
import base64
import glob
import gzip
import hashlib
import json
import logging
import os
import random
import sys
import types
import urllib.parse
import warnings
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "dressing-fixture-secret")

import numpy as np  # noqa: E402

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

from app.core.config import settings  # noqa: E402

settings.postgres_dsn = "postgresql://stub/stub"

from app.embed import backdrop as bd  # noqa: E402
from app.embed import service as embed  # noqa: E402
from app.embed import uploads  # noqa: E402
from app.trove.blueprint import codec  # noqa: E402
from app.trove.codexes import binfab, pg_store  # noqa: E402
from app.trove.dressing import catalogue as cat  # noqa: E402
from app.trove.dressing import customhead, sockets  # noqa: E402
from app.trove.dressing import service as dress  # noqa: E402
from app.trove.mods_hub import assembly, rig_index, trove_layout  # noqa: E402
from app.trove.render import service as render_service  # noqa: E402
from app.trove.render import source  # noqa: E402
from app.trove.updates import read as updates_read  # noqa: E402

TROVE = Path(os.environ.get("KIWI_TROVE_DIR", "E:/Trove"))
NATIVE = Path(__file__).resolve().parents[2]
OUT = NATIVE / "kiwi-server" / "testdata" / "dressing"
BRANCH = "live-us"


def b64(b):
    return base64.b64encode(b).decode()


def rel(p):
    return Path(p).relative_to(TROVE).as_posix()


def read(path):
    return (TROVE / path).read_bytes()


CLASSES = sorted(rel(p) for p in glob.glob(str(TROVE / "prefabs/class/*.binfab")))
SKINS = [f"prefabs/skins/{n}.binfab" for n in (
    "knight_blood", "knight_bone", "knight_bruiser", "candybarbarian_amperium", "candybarbarian_arena",
    "shadowhunter_berzerker", "shadowhunter_bog", "bard_actress", "gunslinger_alley", "gunslinger_atlantean",
    "solarion_chaos", "adventurer_artist")]
EQUIP = sorted({rel(p) for n in (
    "starterweapon_*", "adventure/helm_clubits_01", "adventure/face_clubits_01", "candy_helm_001",
    "events/2024/december_hat_1", "events/2024/december_hat_2", "face_cbq", "face_hideyhood", "face_mask_spiderhug",
    "adventure/weapon_1h_sword_clubits_01", "adventure/weapon_bow_clubits_01", "adventure/weapon_pistol_clubits_01",
    "adventure/weapon_staff_clubits_01", "adventure/weapon_1h_spear_clubits_01", "crystal_fist_001", "axe_purity",
    "chaos_spear", "buff_ring", "devgun_adept",
    "banner/allrounder_attackspeed_energyregen_frontier_pistols") for p in glob.glob(str(TROVE / f"prefabs/equipment/{n}.binfab"))})
HEADS = "prefabs/custom_heads_service.binfab"
FILES = {p: read(p) for p in CLASSES + SKINS + EQUIP + [HEADS]}


def full_locale():
    loc = {}
    for p in sorted(glob.glob(str(TROVE / "languages/en/*.binfab"))):
        loc.update(binfab.extract_localization_map(Path(p).read_bytes()))
    return loc


def wanted_keys():
    keys = set()
    for p in CLASSES:
        c = sockets.parse_class(FILES[p])
        if c:
            keys.add(c["name_key"])
    for p in SKINS:
        keys.update(s for _o, _f, s in binfab.harvest_strings(FILES[p]) if cat._NAME_KEY_RE.match(s))
    for p in EQUIP:
        ident = binfab.decode_identity(FILES[p]) or {}
        keys.add(ident.get("name_key") or "")
    for row in customhead.parse(FILES[HEADS]):
        keys.add(row["name_key"])
        for bps in row["pieces"].values():
            keys.update(f"$CustomHead_Piece_{bp}" for bp in bps)
    return keys


_FULL = full_locale()
LOC = {k: _FULL[k] for k in sorted(wanted_keys()) if k in _FULL}
LOC["$CustomHead_Piece_c_p_hair_bob"] = "  `Bob\\ncut`  "

# --- the rig map ------------------------------------------------------------------------

ROWS = []
for p in SKINS:
    r = binfab.extract_rig_refs(FILES[p])
    for base, ap in r["parts"].items():
        ROWS.append((p, base, r["skeleton"], ap, float(r["scales"].get(base, 1.0)), float(r["head_scale"])))
KNIGHT_PARTS = [r for r in ROWS if r[0] == "prefabs/skins/knight_blood.binfab"]
for prefab in ("prefabs/npc/a/guard.binfab", "prefabs/npc/b/guard.binfab", "prefabs/collections/mount/mount_knightly.binfab"):
    for r in KNIGHT_PARTS:
        ROWS.append((prefab, r[1], "biped_medium", r[3], 1.0, 1.0))
ROWS.append(("prefabs/collections/mount/nobake.binfab", "c_p_knight_blood_torso", "not_a_rig", "torso", 1.0, 1.0))
ROWS.sort(key=lambda r: (r[0], r[1]))

# --- synthetic blueprints ---------------------------------------------------------------

def tiny(name, kind="body"):
    h = hashlib.sha256(name.encode()).digest()
    base = (60 + h[0] % 150, 60 + h[1] % 150, 60 + h[2] % 150)
    vox = []
    w, d, t = 2 + h[3] % 3, 2 + h[4] % 2, 1 + h[5] % 2
    for x in range(w):
        for y in range(d):
            for z in range(t):
                r, g, b = base
                ty = 21
                if kind == "hair" and (x + y) % 2 == 0:
                    r, g, b = 220, 20 + 10 * x, 30
                if kind == "eyes":
                    r, g, b = 40, 200, 240
                if kind == "head" and x == 0 and y == 0:
                    r, g, b = 230, 10 + z * 5, 12
                if kind == "glow" and z == 0:
                    ty = 55
                vox.append({"x": x, "y": y, "z": z, "r": r, "g": g, "b": b, "w": 0, "type": ty})
    return codec.encode(vox, version=5, pos=(-(w // 2), 0, -(t // 2)))


BLUEPRINTS = {}
GAME_PATHS = []


def add(path, data):
    GAME_PATHS.append(path)
    BLUEPRINTS[path[len("blueprints/"):].lower()] = data


seen_parts = set()
for p in SKINS:
    r = binfab.extract_rig_refs(FILES[p])
    for base in r["parts"]:
        ref = r["refs"].get(base, base)
        path = f"blueprints/{ref}.blueprint"
        if path in seen_parts:
            continue
        seen_parts.add(path)
        add(path, tiny(base, "glow" if base.endswith("_hand_l") else "body"))
add("blueprints/2019/other/c_p_knight_blood_torso.blueprint", tiny("decoy torso"))
add("blueprints/npc/a/c_p_knight_bone_torso.blueprint", tiny("decoy bone"))
SKIPPED_STYLES = set()
for i, p in enumerate(EQUIP):
    ref = cat._blueprint_ref(FILES[p])
    if not ref:
        continue
    if i % 9 == 4:
        SKIPPED_STYLES.add(ref)
        continue
    add(f"blueprints/{ref}.blueprint", tiny(ref))
for row in customhead.parse(FILES[HEADS]):
    for kind, bps in row["pieces"].items():
        for j, bp in enumerate(bps):
            if kind == "hair" and j % 20 != 0:
                continue
            path = f"blueprints/2018/custom_heads/{bp.lower()}.blueprint"
            if path[len("blueprints/"):] not in BLUEPRINTS:
                add(path, tiny(bp, kind))
add("blueprints/somewhere/else/c_p_hair_bob.blueprint", tiny("bob twin", "hair"))
add("blueprints/props/barrel.blueprint", tiny("barrel"))
add("blueprints/props/barrel_big.blueprint", tiny("barrel big"))
EXTRA_FILES = {
    "particles/auras/aura_test.pkfx": b"Version 2\nAsset \"Textures/FX/glow.dds\"\nAsset \"Particles/Auras/inner.pkfx\"\n",
    "audio/sfx/test_bank.bnk": b"BKHD not a real bank",
}
GAME_PATHS += list(EXTRA_FILES)
GAME_MAP, GAME_ALL = {}, {}
for _p in GAME_PATHS:
    _n = _p.rsplit("/", 1)[-1].lower()
    GAME_MAP.setdefault(_n, _p)
    GAME_ALL.setdefault(_n, []).append(_p)
GAME_FILES = {p: BLUEPRINTS[p[len("blueprints/"):].lower()] for p in GAME_PATHS if p.startswith("blueprints/")}
GAME_FILES.update(EXTRA_FILES)
SHA = {hashlib.sha256(v).hexdigest(): v for v in GAME_FILES.values()}


def basenames():
    seen = {}
    for row in GAME_PATHS:
        if not row.startswith("blueprints/"):
            continue
        path = row[len("blueprints/"):]
        base = path.rsplit("/", 1)[-1].removesuffix(".blueprint").lower()
        seen[base] = None if base in seen else path
    return {b: p for b, p in seen.items() if p}


BASENAMES = basenames()

# --- stubs -----------------------------------------------------------------------------------

SIG = (27, None)


async def _meta_signature(branch):
    return SIG


async def _locale(branch):
    return dict(LOC)


async def _load(branch, prefix):
    return [(p, FILES[p]) for p in CLASSES + SKINS + EQUIP if p.startswith(prefix) and p.endswith(".binfab")]


async def _load_one(branch, path):
    return FILES.get(path)


async def _rig_map(branch):
    return rig_index._build(ROWS)


async def _index_signature(branch=None):
    return None


async def _paths(branch=trove_layout.LIVE_BRANCH):
    return {k: list(v) for k, v in GAME_ALL.items()}


async def _map(branch=trove_layout.LIVE_BRANCH):
    return dict(GAME_MAP)


async def _by_basename(branch):
    return dict(BASENAMES)


async def _bytes(name, branch=None):
    n = name.replace("\\", "/").strip().lower()
    if n.startswith("blueprints/"):
        n = n[len("blueprints/"):]
    return BLUEPRINTS.get(n)


async def _file_meta(branch, path):
    data = GAME_FILES.get(path)
    return None if data is None else {"path": path, "content_sha256": hashlib.sha256(data).hexdigest()}


async def _game_blob(sha):
    return SHA.get(sha)


pg_store.meta_signature = _meta_signature
cat._locale_map = _locale
cat._load = _load
cat._load_one = _load_one
rig_index._rig_map = _rig_map
rig_index.index_signature = _index_signature
trove_layout.game_file_paths = _paths
trove_layout.game_file_map = _map
source.blueprint_by_basename = _by_basename
source.get_blueprint_bytes = _bytes
for mod in (dress, embed):
    for name, fn in (("game_file_paths", _paths), ("game_file_map", _map), ("blueprint_by_basename", _by_basename),
                     ("get_blueprint_bytes", _bytes)):
        if hasattr(mod, name):
            setattr(mod, name, fn)
render_service.get_blueprint_bytes = _bytes
bd.get_blueprint_bytes = _bytes
updates_read.get_file_meta = _file_meta
embed._read_game_blob = _game_blob
for mod in (dress, render_service, uploads):
    mod.get_redis = lambda: None


def run(coro):
    return asyncio.run(coro) if asyncio.iscoroutine(coro) else coro


CATALOGUE = run(cat._build(BRANCH))
cat._cache[BRANCH] = (SIG, CATALOGUE)


def jenc(v):
    from fastapi.encoders import jsonable_encoder
    return json.loads(json.dumps(jsonable_encoder(v)))


def outcome(fn):
    from app.core.errors import APIError
    try:
        return {"ok": run(fn())}
    except APIError as e:
        return {"error": [e.status_code, e.code, e.message]}


# --- engine ---------------------------------------------------------------------------------

def outfit_cases():
    return [
        {"class": "knight"},
        {"class": " KNIGHT ", "costume": "knight_bone", "race": "undead"},
        {"class": "knight", "costume": "candybarbarian_arena", "hat": "helm_clubits_01", "hair": "c_p_hair_bob",
         "face": "face_cbq"},
        {"class": "knight", "hat": "december_hat_1", "face": "face_cbq", "weapon": "starterweapon_knight",
         "hair_color": "#FF8800", "eye_color": "00ff00", "hair_scale": 0.33337},
        {"class": "knight", "weapon": "starterweapon_shadowhunter", "hat": "nope_hat", "face": "none",
         "head": "c_p_head_male_01", "eyes": "none"},
        {"class": "candybarbarian", "weapon": "starterweapon_candybarbarian", "hair": "none", "race": "robot",
         "hair_color": "#123456"},
        {"class": "shadowhunter", "weapon": "props/barrel", "weapon_family": "bow"},
        {"class": "shadowhunter", "costume": "shadowhunter_bog", "weapon": "barrel", "weapon_family": "melee"},
        {"class": "gunslinger", "weapon": "Barrel.Blueprint", "hat": "barrel"},
        {"class": "bard", "hair": "c_p_hair_bob", "race": "headless"},
        {"class": "solarion", "weapon": "starterweapon_solarion", "face": "a/../b"},
        {"class": "knight", "hat": "x" * 221},
        {"class": "dinotamer_ultimate"},
        {"class": "nope"},
        {"class": "knight", "hat": "candy_helm_001", "hair": "none", "face": "props/barrel"},
    ]


def selection(q):
    return dict(class_key=q.get("class", ""), costume=q.get("costume"), hat=q.get("hat"), face=q.get("face"),
                weapon=q.get("weapon"), head=q.get("head"), hair=q.get("hair"), weapon_family=q.get("weapon_family"),
                eyes=q.get("eyes"), race=q.get("race"), hair_color=q.get("hair_color"), eye_color=q.get("eye_color"),
                hair_scale=q.get("hair_scale"))


def engine():
    from app.trove.dressing import router as dr
    out = {
        "files": {p: b64(d) for p, d in FILES.items()}, "classes": CLASSES, "skins": SKINS, "equipment": EQUIP,
        "heads": HEADS, "loc": LOC, "rows": [list(r) for r in ROWS], "game_paths": GAME_PATHS,
        "basenames": BASENAMES, "blueprints": {k: b64(v) for k, v in BLUEPRINTS.items()},
        "game_files": {k: b64(v) for k, v in EXTRA_FILES.items()},
    }
    out["parse_class"] = [[p, sockets.parse_class(FILES[p])] for p in CLASSES]
    out["parse_heads"] = customhead.parse(FILES[HEADS])
    out["humanize"] = [[s, cat._humanize(s)] for s in ("c_p_hair_long_bob[kiwi]", "knight_blood", "[x]", "c_p_eyes",
                                                        "c_p_head_male_01", "a__b//c", "ÉCOLE_ß_x")]
    out["catalogue"] = {"classes": jenc(run(dr.list_classes())), "races": jenc(run(dr.list_races()))}
    queries = [
        {"slot": "costume", "class": "knight"}, {"slot": "costume"}, {"slot": "costume", "class": "KNIGHT", "q": "BO"},
        {"slot": "weapon", "class": "candybarbarian"}, {"slot": "weapon"}, {"slot": "hat", "q": "helm"},
        {"slot": "head", "race": "guy"}, {"slot": "eyes", "race": "UNDEAD"}, {"slot": "hair", "limit": 5, "offset": 3},
        {"slot": "hair", "q": "  "}, {"slot": "face"}, {"slot": "hair", "offset": 100000}, {"slot": "weapon", "class": "nope"},
        {"slot": "head", "race": "nope", "q": "male"},
    ]
    out["options"] = []
    for q in queries:
        res = outcome(lambda q=q: dr.list_options(slot=q["slot"], class_key=q.get("class"), race=q.get("race"),
                                                  q=q.get("q"), offset=q.get("offset", 0), limit=q.get("limit", 100)))
        if "ok" in res:
            res = {"ok": jenc(res["ok"])}
        out["options"].append({"query": q, **res})
    out["outfits"] = []
    for q in outfit_cases():
        res = outcome(lambda q=q: dr.resolve_query(**selection(q)))
        case = {"query": q}
        if "ok" in res:
            o = res["ok"]
            case.update(outfit=jenc(dr.DressOutfit(**o.as_dict(), dropped=o.dropped, issues=o.issues)),
                        ident=o.ident, header=o.issue_header)
            built = run(dress.model(o, "json"))
            case["model"] = b64(gzip.decompress(built.gz)) if built else None
            png = run(dress.still(o, dim=64, camera={"az": 30.0, "el": 8.0}))
            case["still"] = b64(png) if png else None
        else:
            case.update(res)
        out["outfits"].append(case)
    out["stills"] = []
    o = run(dr.resolve_query(**selection({"class": "knight", "hat": "december_hat_1", "weapon": "starterweapon_knight"})))
    for dim, az, el in ((48, -45.5, 20.0), (40, 360.0, -89.0), (33, 0.0, 0.0)):
        out["stills"].append({"query": {"class": "knight", "hat": "december_hat_1", "weapon": "starterweapon_knight"},
                              "dim": dim, "az": az, "el": el,
                              "png": b64(run(dress.still(o, dim=dim, camera={"az": az, "el": el})))})
    out["creature_png"] = []
    for prefab, dim in (("prefabs/skins/knight_blood.binfab", 64), ("prefabs/skins/candybarbarian_arena.binfab", 48),
                        ("prefabs/skins/shadowhunter_bog.binfab", 40), ("prefabs/collections/mount/nobake.binfab", 40),
                        ("Prefabs\\NPC\\A\\guard.binfab", 40)):
        png = run(render_service._build_creature_png(prefab, dim, BRANCH))
        out["creature_png"].append([prefab, dim, b64(png) if png else None])
    out["backdrop"] = {k: run(bd.backdrop(k)) for k in bd.KINDS}
    rig = assembly._rigs()["biped_medium"]
    out["matrices"] = []
    for name in ("head", "face", "hair", "hat"):
        for size in (rig["voxel_scale"] * 0.5, rig["voxel_scale"] * 0.5879999995231628, rig["voxel_scale"], 0.25):
            to_hat = np.diag([bd.EFFECT_PER_WORLD] * 3 + [1.0]) @ np.linalg.inv(
                np.array(rig["rest"]["hat"], dtype=float).reshape(4, 4).T)
            m = to_hat @ np.array(rig["rest"][name], dtype=float).reshape(4, 4).T @ np.diag([size] * 3 + [1.0])
            out["matrices"].append({"part": name, "size": size, "m": [round(float(v), 6) for v in m.T.flatten()],
                                    "raw": [float(v) for v in m.T.flatten()]})
    rnd = random.Random(7)
    out["inverses"] = []
    for rig_name in sorted(assembly._rigs())[:60]:
        for ap, mat in list(assembly._rigs()[rig_name]["rest"].items())[:2]:
            a = np.array(mat, dtype=float).reshape(4, 4).T
            out["inverses"].append([[float(v) for v in a.flatten()], [float(v) for v in np.linalg.inv(a).flatten()]])
    for _ in range(40):
        a = np.array([[rnd.uniform(-3, 3) for _ in range(4)] for _ in range(4)])
        out["inverses"].append([[float(v) for v in a.flatten()], [float(v) for v in np.linalg.inv(a).flatten()]])
    from app.trove import tmod
    from app.trove.render import bp_cache
    parts = [r[1] for r in ROWS if r[0] == "prefabs/skins/candybarbarian_arena.binfab"]
    out["tmod_assembly"] = []
    for name, files in (
        ("parts", [(f"Blueprints/{b.upper()}.blueprint" if i == 0 else f"blueprints/x/{b}.blueprint",
                    BLUEPRINTS[next(k for k in BLUEPRINTS if k.endswith(b + ".blueprint"))]) for i, b in enumerate(parts)]
         + [("ui/readme.txt", b"hi")]),
        ("unbound", [("blueprints/props/barrel.blueprint", BLUEPRINTS["props/barrel.blueprint"])]),
    ):
        data = tmod.build_tmod(1, {"title": name}, files)

        async def build(data=data):
            fl = tmod.read_tmod(data)["files"]
            names = [f["path"].split("/")[-1][:-len(".blueprint")].lower()
                     for f in fl if f["path"].lower().endswith(".blueprint")]
            skeleton, attach = await rig_index.resolve(names)
            scales = await rig_index.scales_for(attach, skeleton)
            model = assembly.assemble(fl, rig_name=skeleton, ap_overrides=attach, part_scales=scales.parts)
            if model is None:
                raise bp_cache.NoPayload
            return model
        try:
            body = b64(gzip.decompress(run(bp_cache.build_uncached(build, "json")).gz))
        except bp_cache.NoPayload:
            body = None
        out["tmod_assembly"].append({"name": name, "tmod": b64(data), "model": body})
    out["format_g"] = [[x, f"{x:g}"] for x in (30.0, -45.5, 360.0, -89.0, 0.0, -0.0, 1e-5, 123.456789, 359.99999)]
    return out


# --- routes through the app -------------------------------------------------------------------

async def asgi(app, method, url, headers):
    path, _, qs = url.partition("?")
    scope = {
        "type": "http", "asgi": {"version": "3.0"}, "http_version": "1.1", "method": method,
        "scheme": "http", "path": urllib.parse.unquote(path), "raw_path": path.encode(),
        "query_string": qs.encode(), "root_path": "",
        "headers": [(b"host", b"testserver")] + headers,
        "client": ("127.0.0.1", 50000), "server": ("testserver", 80),
    }
    done = asyncio.Event()
    pending = [{"type": "http.request", "body": b"", "more_body": False}]
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
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


KEPT = ("content-type", "cache-control", "etag", "x-dressing-dropped", "x-kiwi-cache", "vary", "content-disposition")
Q = urllib.parse.urlencode


def route_cases():
    knight = Q({"class": "knight", "hat": "december_hat_1", "weapon": "starterweapon_knight", "face": "nope"})
    dress_tok = "knight:knight_bone:december_hat_1::starterweapon_knight::::guy_eyes_01:undead"
    return [
        ("races", "/v1/dressing/races"), ("classes", "/v1/dressing/classes"),
        ("options_costume", "/v1/dressing/options?slot=costume&class=knight"),
        ("options_no_class", "/v1/dressing/options?slot=costume"),
        ("options_422", "/v1/dressing/options?slot=boots&limit=0&offset=-1&q=" + "x" * 81),
        ("options_missing", "/v1/dressing/options"),
        ("outfit", "/v1/dressing/outfit?" + knight),
        ("outfit_covered", "/v1/dressing/outfit?" + Q({"class": "knight", "hat": "candy_helm_001", "hair": "c_p_hair_bob", "face": "face_cbq"})),
        ("outfit_unknown", "/v1/dressing/outfit?class=nope"),
        ("outfit_long", "/v1/dressing/outfit?class=knight&weapon=" + "w" * 300),
        ("outfit_422", "/v1/dressing/outfit?hair_scale=2&class=knight"),
        ("outfit_missing", "/v1/dressing/outfit"),
        ("model", "/v1/dressing/model?" + knight),
        ("model_bin", "/v1/dressing/model?fmt=bin&" + knight),
        ("model_422", "/v1/dressing/model?fmt=xml&class=knight"),
        ("image", "/v1/dressing/image?size=48&" + knight),
        ("image_cam", "/v1/dressing/image?size=40&az=-12.5&el=30&class=candybarbarian"),
        ("image_422", "/v1/dressing/image?size=9&az=400&el=x&class=knight"),
        ("site_palette", "/site/dressing/palette"), ("site_classes", "/site/dressing/classes"),
        ("site_races", "/site/dressing/races"),
        ("site_options", "/site/dressing/options?slot=hat&q=hat"),
        ("site_options_422", "/site/dressing/options?slot=x"),
        ("site_model", "/site/dressing/model?" + knight),
        ("site_model_color_422", "/site/dressing/model?class=knight&hair_color=%23ff00aa1"),
        ("site_model_none", "/site/dressing/model?class=dinotamer_ultimate"),
        ("site_image", "/site/dressing/image?size=40&" + knight),
        ("site_render_costume", "/site/dressing/render?prefab=prefabs/skins/knight_blood.binfab&dim=40"),
        ("site_render_nothing", "/site/dressing/render"),
        ("site_render_bad_prefab", "/site/dressing/render?prefab=prefabs/nope.binfab"),
        ("site_render_422", "/site/dressing/render?dim=300&blueprint="),
        ("embed_manifest_prefab", "/site/embed/manifest?prefab=prefabs/skins/knight_blood.binfab"),
        ("embed_manifest_stem", "/site/embed/manifest?prefab=knight_bone"),
        ("embed_manifest_ambiguous", "/site/embed/manifest?prefab=guard"),
        ("embed_manifest_unknown", "/site/embed/manifest?prefab=nope_prefab"),
        ("embed_manifest_dress", "/site/embed/manifest?dress=" + urllib.parse.quote(dress_tok)),
        ("embed_manifest_dress_qs", "/site/embed/manifest?dress=" + urllib.parse.quote("class=candybarbarian&hair=c_p_hair_bob&hair_color=%2300ff00&race=")),
        ("embed_manifest_dress_bad", "/site/embed/manifest?dress=nope:x"),
        ("embed_manifest_game", "/site/embed/manifest?game=c_p_knight_blood_torso.blueprint"),
        ("embed_manifest_game_full", "/site/embed/manifest?game=" + urllib.parse.quote("\\blueprints/props/barrel.blueprint")),
        ("embed_manifest_pkfx", "/site/embed/manifest?game=aura_test.pkfx"),
        ("embed_manifest_bnk", "/site/embed/manifest?game=Audio/SFX/test_bank.bnk"),
        ("embed_manifest_game_ext", "/site/embed/manifest?game=a.dds"),
        ("embed_manifest_game_missing", "/site/embed/manifest?game=nope.blueprint"),
        ("embed_manifest_two", "/site/embed/manifest?game=a.blueprint&prefab=b"),
        ("embed_manifest_none", "/site/embed/manifest"),
        ("embed_manifest_tmod", "/site/embed/manifest?tmod=" + "ab" * 32),
        ("embed_manifest_422", "/site/embed/manifest?release=" + "r" * 65 + "&tmod=" + "t" * 65),
        ("embed_assembled_prefab", "/site/embed/assembled?prefab=prefabs/skins/knight_blood.binfab"),
        ("embed_assembled_game", "/site/embed/assembled?game=c_p_candybarbarian_arena_hips.blueprint"),
        ("embed_assembled_game_unbound", "/site/embed/assembled?game=props/barrel_big.blueprint"),
        ("embed_assembled_dress", "/site/embed/assembled?dress=" + urllib.parse.quote(dress_tok)),
        ("embed_assembled_nobake", "/site/embed/assembled?prefab=prefabs/collections/mount/nobake.binfab"),
        ("embed_assembled_pkfx", "/site/embed/assembled?game=aura_test.pkfx"),
        ("embed_assembled_422", "/site/embed/assembled?prefab=knight_bone&fmt=gif"),
        ("embed_blueprint_prefab", "/site/embed/blueprint?prefab=knight_blood&path=c_p_knight_blood_torso.blueprint"),
        ("embed_blueprint_prefab_other", "/site/embed/blueprint?prefab=knight_blood&path=barrel.blueprint"),
        ("embed_blueprint_game", "/site/embed/blueprint?game=props/barrel.blueprint&path=BARREL.blueprint"),
        ("embed_blueprint_game_other", "/site/embed/blueprint?game=props/barrel.blueprint&path=x.blueprint"),
        ("embed_blueprint_dress", "/site/embed/blueprint?dress=knight"),
        ("embed_vfx_prefab", "/site/embed/vfx/manifest?prefab=knight_blood&path=a.pkfx"),
        ("embed_vfx_422", "/site/embed/vfx/manifest?prefab=knight_blood"),
        ("embed_asset_dress", "/site/embed/vfx/asset?dress=knight&path=a.dds"),
        ("embed_audio_prefab", "/site/embed/audio/bank?prefab=knight_blood"),
        ("embed_audio_other_bank", "/site/embed/audio/bank?game=test_bank.bnk&path=other.bnk"),
        ("embed_audio_blueprint", "/site/embed/audio/bank?game=props/barrel.blueprint"),
        ("embed_sound_422", "/site/embed/audio/sound?prefab=knight_blood&id=-1&raw=maybe"),
        ("embed_backdrop_melee", "/site/embed/vfx/backdrop?kind=melee"),
        ("embed_backdrop_pistol", "/site/embed/vfx/backdrop?kind=pistol"),
        ("embed_backdrop_spear", "/site/embed/vfx/backdrop?kind=spear"),
        ("embed_backdrop_hat", "/site/embed/vfx/backdrop?kind=hat"),
        ("embed_backdrop_422", "/site/embed/vfx/backdrop?kind=bow"),
    ]


async def _allow(key, max_requests, window_seconds):
    from datetime import datetime, timezone
    from app.core import ratelimit
    return ratelimit.RateLimitInfo(limit=max_requests, remaining=max_requests - 1,
                                   reset=datetime(2030, 1, 1, tzinfo=timezone.utc))


def routes():
    import app.main as m
    from app.core import ratelimit
    original = ratelimit.check_rate_limit
    for mod in list(sys.modules.values()):
        if getattr(mod, "check_rate_limit", None) is original:
            mod.check_rate_limit = _allow
    out = []
    for i, (name, url) in enumerate(route_cases()):
        rid = f"dr-{i}"
        status, headers, raw = run(asgi(m.app, "GET", url, [(b"x-request-id", rid.encode())]))
        kept = [[k, v] for k, v in headers if k in KEPT]
        out.append({"name": name, "path": url, "request_id": rid, "status": status, "headers": kept, "response": b64(raw)})
    return out


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    (OUT / "engine.json").write_text(json.dumps(engine(), indent=0, ensure_ascii=False), encoding="utf-8")
    (OUT / "routes.json").write_text(json.dumps(routes(), indent=0, ensure_ascii=False), encoding="utf-8")


if __name__ == "__main__":
    main()
