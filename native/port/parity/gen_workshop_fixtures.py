"""Golden fixtures for the Mod Workshop, the creature rigs and the Blueprint
Editor model projects, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_workshop_fixtures.py

Writes native/kiwi-server/testdata/workshop/{engine,routes}.json and
native/kiwi-server/testdata/rigs/{engine,routes}.json, plus the twin-sandbox
specs workshop.writes.json and rigs.writes.json next to this file.

zipfile is pinned to Linux behaviour (create system 3, no backslash folding)
and a fixed clock; the game file index and the rig binding map are stubbed
with the fixed tables below, which the Rust tests install through their test
hooks.
"""
import asyncio
import base64
import hashlib
import io
import json
import logging
import os
import posixpath
import struct
import sys
import time
import types
import urllib.parse
import warnings
import zipfile
import zlib
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "workshop-fixture-secret")

_os = types.ModuleType("os")
_os.__dict__.update(os.__dict__)
_os.sep, _os.altsep, _os.path = "/", None, posixpath
zipfile.os = _os
zipfile.sys = types.SimpleNamespace(platform="linux", flags=sys.flags, stderr=sys.stderr, exit=sys.exit)
ZIP_CLOCK = 1_700_000_000
zipfile.time = types.SimpleNamespace(time=lambda: ZIP_CLOCK, localtime=time.gmtime)

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))
for _flag in ("feature_mod_workshop_enabled", "feature_blueprint_editor_enabled"):
    rc._CACHE[_flag] = (True, float("inf"))

from app.trove import tmod  # noqa: E402
from app.trove.blueprint import codec  # noqa: E402
from app.trove.mods_hub import assembly, rig_index, trove_layout, workshop  # noqa: E402

NATIVE = Path(__file__).resolve().parents[2]
TD = NATIVE / "kiwi-server" / "testdata"
OUT_W, OUT_R = TD / "workshop", TD / "rigs"
PARITY = Path(__file__).resolve().parent


def b64(b):
    return base64.b64encode(b).decode()


def bp(name):
    return (TD / "blueprints" / "bp" / name).read_bytes()


SWF = (TD / "media" / "pvpplayercounterui.swf").read_bytes()
BINFAB = (TD / "codexes" / "prefabs__collections__flask__effect_crit.binfab").read_bytes()
SWORD, HAT, HEAD = bp("equipment_weapon_1h_sword_001.blueprint"), bp("c_c_snowman_hat_01.blueprint"), bp("c_m_goldenbeetle_head.blueprint")
BLANK, STACHE, GLASS = bp("blank.blueprint"), bp("char_hair_face_mustache_001.blueprint"), bp("s_glass.blueprint")

GAME_PATHS = [
    "blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint",
    "ui/pvpplayercounterui.swf",
    "prefabs/collections/flask/effect_crit.binfab",
    "blueprints/creatures/beetle/c_m_goldenbeetle_head.blueprint",
    "blueprints/skins/adventurer/c_c_snowman_hat_01.blueprint",
    "blueprints/npc/merchant/c_c_snowman_hat_01.blueprint",
    "Textures/Shared/Foo.dds",
    "blueprints/skins/adventurer/char_hair_face_mustache_001.blueprint",
]
GAME_MAP, GAME_ALL = {}, {}
for _p in GAME_PATHS:
    _n = _p.rsplit("/", 1)[-1].lower()
    GAME_MAP.setdefault(_n, _p)
    GAME_ALL.setdefault(_n, []).append(_p)
GAME_FILES = {
    "blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint": SWORD,
    "blueprints/creatures/beetle/c_m_goldenbeetle_head.blueprint": HEAD,
    "blueprints/skins/adventurer/c_c_snowman_hat_01.blueprint": HAT,
    "blueprints/npc/merchant/c_c_snowman_hat_01.blueprint": GLASS,
    "blueprints/skins/adventurer/char_hair_face_mustache_001.blueprint": STACHE,
}

ROWS = sorted([
    ("prefabs/npc/beetle.binfab", "c_m_goldenbeetle_head", "adventurer", "head", 0.75, 1.0),
    ("prefabs/npc/delve/beetle.binfab", "c_m_goldenbeetle_head", "adventurer", "head", 1.0, 1.0),
    ("prefabs/skins/adventurer/adv_knight.binfab", "c_c_snowman_hat_01", "adventurer", "hat", 1.0, 0.588),
    ("prefabs/skins/adventurer/adv_knight.binfab", "c_m_goldenbeetle_head", "adventurer", "head", 1.0, 0.588),
    ("prefabs/skins/adventurer/adv_knight.binfab", "equipment_weapon_1h_sword_001", "adventurer", "r_hand", 1.0, 0.588),
    ("prefabs/skins/adventurer/adv_knight.binfab", "char_hair_face_mustache_001", "adventurer", "face", 0.5, 0.588),
    ("prefabs/collections/mount/nobake.binfab", "c_c_snowman_hat_01", "not_a_baked_rig", "hat", 1.0, 1.0),
    ("prefabs/collections/mount/ghost.binfab", "e_cave_a_airlock_branch", "bat", "nope_ap", 1.0, 1.0),
    ("prefabs/collections/mount/lonely.binfab", "s_hat", "bat", "head", 1.0, 1.0),
], key=lambda r: (r[0], r[1]))


async def _fake_map(branch=trove_layout.LIVE_BRANCH):
    return dict(GAME_MAP)


async def _fake_paths(branch=trove_layout.LIVE_BRANCH):
    return {k: list(v) for k, v in GAME_ALL.items()}


async def _fake_rig_map(branch):
    return rig_index._build(ROWS)


async def _fake_read(path):
    return GAME_FILES.get(path)


trove_layout.game_file_map = _fake_map
trove_layout.game_file_paths = _fake_paths
rig_index._rig_map = _fake_rig_map


def run(coro):
    return asyncio.run(coro) if asyncio.iscoroutine(coro) else coro


def outcome(fn):
    try:
        return {"ok": run(fn())}
    except workshop.WorkshopError as e:
        return {"error": str(e)}
    except Exception as e:  # noqa: BLE001
        return {"unhandled": type(e).__name__}


def mkzip(entries, *, method=zipfile.ZIP_DEFLATED, comment=b""):
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", method) as zf:
        for e in entries:
            if isinstance(e, tuple):
                zf.writestr(*e)
            else:
                e(zf)
        zf.comment = comment
    return buf.getvalue()


def patch(data, old, new, count=-1):
    assert old in data, old
    return data.replace(old, new, count)


def cd_offset(data, name=b"MyMod/blueprints/a.blueprint"):
    """The central record of ``name``."""
    return data.index(name, data.index(b"PK\x01\x02")) - 46


def lh_offset(data, name=b"MyMod/blueprints/a.blueprint"):
    """The local header of ``name``."""
    return data.index(name) - 30


def zip_cases():
    base = [("MyMod/", b""), ("MyMod/blueprints/a.blueprint", SWORD), ("MyMod/.DS_Store", b"junk"),
            ("__MACOSX/MyMod/._a.blueprint", b"meta"), ("MyMod/readme.txt", b"hello"),
            ("MyMod/sub/", b""), ("MyMod/ui/Th\u00fcmb.swf", SWF[:200])]
    good = mkzip(base)
    stored = mkzip(base, method=zipfile.ZIP_STORED)
    cp = patch(mkzip([("ui/xZy.dds", b"cp437 name")], method=zipfile.ZIP_STORED), b"xZy", b"x\x81\xe9y")

    def unicode_extra(zf):
        info = zipfile.ZipInfo("ui/plain.dds", date_time=(2024, 1, 2, 3, 4, 6))
        name = "ui/fancy_\u00e9.dds".encode()
        info.extra = struct.pack("<HHBL", 0x7075, 5 + len(name), 1, zlib.crc32(b"ui/plain.dds")) + name
        zf.writestr(info, b"unicode path")
    upath = mkzip([unicode_extra], method=zipfile.ZIP_STORED)

    def z64(zf):
        with zf.open("ui/big.dds", "w", force_zip64=True) as f:
            f.write(b"zip64 local extra")
    z64z = mkzip([z64], method=zipfile.ZIP_STORED)

    enc = bytearray(stored)
    for sig, off in ((b"PK\x03\x04", 6), (b"PK\x01\x02", 8)):
        i = 0
        while (i := enc.find(sig, i)) >= 0:
            enc[i + off] |= 1
            i += 4
    bad_crc = bytearray(stored)
    c = cd_offset(bad_crc)
    bad_crc[c + 16:c + 20] = b"\x00\x00\x00\x00"
    deflate64 = bytearray(good)
    c = cd_offset(deflate64)
    deflate64[c + 10:c + 12] = struct.pack("<H", 9)
    bzip = bytearray(good)
    c = cd_offset(bzip)
    bzip[c + 10:c + 12] = struct.pack("<H", 12)
    lzma_garbage = bytearray(good)
    lzma_garbage[c + 10:c + 12] = struct.pack("<H", 14)
    lzma_props = bytearray(mkzip(base[:2], method=zipfile.ZIP_LZMA))
    lzma_props[lh_offset(lzma_props) + 30 + len(b"MyMod/blueprints/a.blueprint") + 4] = 0xFF
    bad_local = bytearray(stored)
    i = lh_offset(bad_local)
    bad_local[i:i + 4] = b"PK\x03\x05"
    name_mismatch = bytearray(stored)
    name_mismatch[lh_offset(name_mismatch) + 30] = ord("N")
    overlap = bytearray(stored)
    c = cd_offset(overlap)
    overlap[c + 20:c + 24] = struct.pack("<I", 4000)
    version = bytearray(stored)
    c = cd_offset(version)
    version[c + 6] = 64
    bad_extra = bytearray(upath)
    c = cd_offset(bad_extra, b"ui/plain.dds")
    bad_extra[c + 46 + 12 + 2:c + 46 + 12 + 4] = b"\xff\xff"
    deflate_garbage = bytearray(mkzip([("ui/a.dds", b"A" * 300)]))
    deflate_garbage[30 + 8:30 + 12] = b"\xff\xff\xff\xff"
    huge = bytearray(mkzip([("blueprints/a.blueprint", b"x")], method=zipfile.ZIP_STORED))
    c = cd_offset(huge, b"blueprints/a.blueprint")
    huge[c + 24:c + 28] = struct.pack("<I", 200 * 1024 * 1024)
    utf8_bad = bytearray(cp)
    c = cd_offset(utf8_bad, b"ui/x\x81\xe9y.dds")
    utf8_bad[c + 8:c + 10] = struct.pack("<H", 0x800)
    return {
        "good": good, "stored": stored, "comment": mkzip(base[:2], comment=b"a comment PK"),
        "cp437": cp, "unicode_extra": upath, "zip64_local": z64z, "encrypted": bytes(enc),
        "bad_crc": bytes(bad_crc), "deflate64": bytes(deflate64), "bad_local": bytes(bad_local),
        "name_mismatch": bytes(name_mismatch), "overlap": bytes(overlap), "version": bytes(version),
        "bad_extra": bytes(bad_extra), "deflate_garbage": bytes(deflate_garbage), "only_dirs": mkzip([("a/", b""), ("b/", b"")]),
        "not_zip": b"PK\x03\x04garbage", "truncated": good[: len(good) // 2], "tiny": b"PK",
        "too_big": bytes(huge), "utf8_bad": bytes(utf8_bad), "bzip2": bytes(bzip),
        "bzip2_ok": mkzip(base, method=zipfile.ZIP_BZIP2), "lzma_ok": mkzip(base, method=zipfile.ZIP_LZMA),
        "lzma_garbage": bytes(lzma_garbage), "lzma_bad_props": bytes(lzma_props),
    }


def tmod_bytes(files, props=None, **kw):
    return tmod.build_tmod(1, props or {"title": "Test Mod"}, files, **kw)


MOD_FILES = [("blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint", SWORD),
             ("ui/pvpplayercounterui.swf", SWF),
             ("prefabs/collections/flask/effect_crit.binfab", BINFAB),
             ("ui/Test Mod.cfg", b"key=value\n"),
             ("ui/test mod.png", b"\x89PNG fake"),
             ("Blueprints/Blank.blueprint", BLANK)]
MOD = tmod_bytes(MOD_FILES, {"title": "Test Mod", "author": "Kiwi", "tags": "Dragons,GUI", "previewPath": "ui/test mod.png",
                             "configPath": "ui/Test Mod.cfg", "flags": "-1"}, lowercase_paths=False)
PLAIN_MOD = tmod_bytes([("blueprints/a.blueprint", SWORD), ("Readme.TXT", b"x")], {"title": "Plain", "flags": "x1"})


def plan_cases():
    a = ["MyMod/blueprints/equipment_weapon_1h_sword_001.blueprint", "MyMod/preview.png",
         "MyMod/ui/pvpplayercounterui.swf", "MyMod/ui/mymod.cfg", "MyMod/notes.txt", "MyMod/.DS_Store",
         "MyMod/blueprints/sub/equipment_weapon_1h_sword_001.blueprint", "MyMod/Blueprints/X.blueprint"]
    return [
        ("wrapper_fix", a, True, []),
        ("wrapper_keep", a, True, ["MyMod/blueprints/equipment_weapon_1h_sword_001.blueprint"]),
        ("wrapper_nofix", a, False, []),
        ("windows", ["Mod\\textures\\shared\\foo.DDS", "Mod\\ui\\a.swf"], True, []),
        ("root_game_file", ["c_m_goldenbeetle_head.blueprint", "readme.md"], True, []),
        ("trove_root", ["blueprints/x.blueprint", "ui/a.swf", "x.cfg"], True, []),
        ("deep", ["a/b/c/d/e/blueprints/x.blueprint", "a/b/c/d/e/ui/y.swf"], True, []),
        ("dots", ["./x/../blueprints/./y.blueprint", "x/../../ui//z.swf"], True, []),
        ("clash", ["blueprints/equipment_weapon_1h_sword_001.blueprint",
                   "blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint"], True, []),
        ("empty_bits", ["", "/", "ui/", "__MACOSX", ".git/config", "blueprints/thumbs.db"], True, []),
        ("unicode", ["M\u00f6d/blueprints/\u00c4.blueprint", "M\u00f6d/UI/\u0130.swf"], True, []),
    ]


def engine():
    out = {"game_paths": GAME_PATHS, "zips": [], "plans": [], "previews": [], "clean": [], "titles": [],
           "nearest": [], "describe": [], "candidates": [], "cp437": "", "filemode": []}
    for name, data in zip_cases().items():
        res = outcome(lambda d=data: workshop.read_zip(d))
        if "ok" in res:
            res = {"ok": [[p, b64(b)] for p, b in res["ok"]]}
        out["zips"].append({"name": name, "data": b64(data), **res})
    out["cp437"] = bytes(range(128, 256)).decode("cp437")
    import stat
    for mode in (0o100644, 0o040755, 0o120777, 0o104755, 0o102644, 0o101000, 0o101001, 0o060600, 0o170644, 0o000644):
        out["filemode"].append([mode, stat.filemode(mode)])
    for name, paths, fix, keep in plan_cases():
        p = run(workshop.plan(paths, fix=fix, keep=keep))
        p.pop("mapping")
        out["plans"].append({"name": name, "paths": paths, "fix": fix, "keep": keep, "plan": p})
        q = run(workshop.preview(paths))
        q.pop("mapping")
        out["previews"].append({"name": name, "paths": paths, "plan": q})
        out["candidates"].append({"paths": paths, "config": workshop.config_candidates(paths),
                                  "preview": workshop.preview_candidates(paths)})
    async def down(branch=None):
        raise RuntimeError("down")

    trove_layout.game_file_map = down
    p = run(workshop.plan(["blueprints/a.blueprint"]))
    p.pop("mapping")
    out["plans"].append({"name": "index_down", "paths": ["blueprints/a.blueprint"], "fix": True, "keep": [], "plan": p,
                         "no_index": True})
    trove_layout.game_file_map = _fake_map
    for raw in [None, {}, {"title": "  ..My:Mod?.. ", "author": "a\x01b\nc", "modVersion": "v" * 50,
                           "notes": " line1\nline2\x07 ", "tags": "Dragons, ,gui,x" + "y" * 50,
                           "previewPath": "UI\\Pre.PNG", "configPath": "/ui/../Cfg/My.cfg"},
                {"title": 5, "tags": ["Dragons", 3, None, True, "  "], "author": 1.5, "notes": False},
                {"title": ["a", "b"], "tags": {"GUI": 1, "Mounts": 2}},
                {"title": "", "previewPath": "", "configPath": None}, [], {"tags": 7}]:
        res = outcome(lambda r=raw: workshop.clean_properties(r))
        if "ok" in res:
            res = {"ok": [[k, v] for k, v in res["ok"].items()]}
        out["clean"].append({"raw": raw, **res})
    for t in [None, "", "  ..My:Mod?.. ", "a" * 200, "<>", " . x", "\u00dcn\u00efc\u00f6d\u00e9 \"q\"", "x\ty"]:
        from app.trove.mods_hub import service as hub
        out["titles"].append([t, workshop.safe_title(t), hub._safe_filename(t)])
    for cands, hint in [([], "x"), (["b/a", "a/a"], ""), (["z/x/a", "b/skins/bee/a", "b/npc/a"], "prefabs/skins/bee/p.binfab"),
                        (["B/a", "a/a", "b/a"], "q/r.binfab"), (["x\\Skins\\a", "y/skins/a"], "p\\SKINS\\c")]:
        out["nearest"].append([cands, hint, trove_layout.nearest_path(cands, hint)])
    for name, data in [("mod", MOD), ("plain", PLAIN_MOD), ("garbage", b"\x00" * 40), ("short", b"abc")]:
        res = outcome(lambda d=data: workshop.describe(d))
        out["describe"].append({"name": name, "data": b64(data), **res})
    return out


# --- rigs ---------------------------------------------------------------------------

def rig_engine():
    rm = rig_index._build(ROWS)
    out = {"rows": [list(r) for r in ROWS], "head_aps": {}, "resolve": [], "scales_for": [], "creature_scales": [],
           "prefab_path": [], "creature_for": [], "assemble": [], "placements": [], "voxels": []}
    for name in ("adventurer", "bat", "nope"):
        out["head_aps"][name] = sorted(assembly.head_aps(name))
    for names in [["c_m_goldenbeetle_head", "c_c_snowman_hat_01", "equipment_weapon_1h_sword_001"],
                  ["c_c_snowman_hat_01_lvl2[techno]", "x"], ["e_cave_a_airlock_branch", "s_hat", "c_c_snowman_hat_01"],
                  ["s_hat", "e_cave_a_airlock_branch"], ["nothing"], [],
                  ["char_hair_face_mustache_001[a]_lvl3", "c_m_goldenbeetle_head"]]:
        skel, attach = run(rig_index.resolve(names))
        out["resolve"].append([names, skel, [[k, v] for k, v in attach.items()]])
        sc = run(rig_index.scales_for(attach, skel if skel and assembly.has_baked_rig(skel) else None))
        out["scales_for"].append([[[k, v] for k, v in attach.items()], skel, sc.head, sorted(sc.parts.items())])
    for prefab in ["prefabs/skins/adventurer/adv_knight.binfab", "prefabs/npc/beetle.binfab", "nope", "Prefabs\\NPC\\beetle.binfab"]:
        sc = run(rig_index.creature_scales(prefab))
        out["creature_scales"].append([prefab, sc.head, sorted(sc.parts.items())])
    for name in ["adv_knight", "beetle", "prefabs/npc/beetle", "/Prefabs\\npc\\BEETLE.binfab ", "nope", "", "x/nope", "ADV_KNIGHT.BINFAB"]:
        canon, cands = run(rig_index.prefab_path(name))
        out["prefab_path"].append([name, canon, cands])
    for b in ["c_m_goldenbeetle_head", "s_hat", "nope"]:
        skel, prefab, parts = run(rig_index.creature_for(b))
        out["creature_for"].append([b, skel, prefab, [[k, v] for k, v in parts.items()]])

    cube = codec.encode([{"x": x, "y": y, "z": z, "r": 120, "g": 120, "b": 120, "w": 0, "type": 21}
                         for x in range(7) for y in range(7) for z in range(7)], version=5, pos=(-3, -3, -3))
    glow = codec.encode([{"x": 0, "y": 0, "z": 0, "r": 250, "g": 30, "b": 30, "w": 0, "type": 55},
                         {"x": 1, "y": 0, "z": 0, "r": 250, "g": 30, "b": 30, "w": 0, "type": 55}], version=5, pos=(2, 1, 0))
    mask = codec.encode([{"x": x, "y": 0, "z": 0, "r": 200 + x * 10, "g": 10 * x, "b": 5, "w": 0, "type": 21} for x in range(5)]
                        + [{"x": 0, "y": 1, "z": 0, "r": 90, "g": 80, "b": 70, "w": 2, "type": 21}], version=3)
    blobs = {"sword": SWORD, "hat": HAT, "head": HEAD, "blank": BLANK, "stache": STACHE, "glass": GLASS,
             "cube": cube, "glow": glow, "mask": mask, "v4": bp("s_v4.blueprint"), "v3": bp("s_v3.blueprint")}
    out["blobs"] = {k: b64(v) for k, v in blobs.items()}

    def files(names):
        return [{"path": f"blueprints/{n}.blueprint", "content_base64": b64(blobs[n])} for n in names]
    for rig_name, names, overrides, scales in [
        ("adventurer", ["sword", "hat", "head", "blank"], {"sword": "r_hand", "hat": "hat", "head": "head", "blank": "torso"}, {}),
        ("adventurer", ["hat", "glass", "stache", "v4"], {"hat": "hat", "glass": "hat", "stache": "face", "v4": "nope"}, {"stache": 0.5}),
        ("adventurer", ["cube", "glow"], {"cube": "torso", "glow": "torso"}, {"glow": 0.5}),
        ("adventurer", ["sword"], {}, {}),
        ("nope", ["sword"], {"sword": "r_hand"}, {}),
    ]:
        model = assembly.assemble(files(names), rig_name=rig_name, ap_overrides=overrides, part_scales=scales)
        out["assemble"].append({"rig": rig_name, "files": names, "overrides": overrides, "scales": scales, "model": model})
    for rig_name, pl in [
        ("adventurer", [{"ap": "torso", "blob": "cube", "scale": 1.0}, {"ap": "torso", "blob": "glow", "scale": 0.5}]),
        ("adventurer", [{"ap": "head", "blob": "mask", "scale": 0.588, "tint": [10, 200, 300]},
                        {"ap": "face", "blob": "mask", "scale": 1.0, "tint": [40, 50, 60], "max_off": 20},
                        {"ap": "hat", "blob": "hat", "scale": 0.5}, {"ap": "nope", "blob": "hat", "scale": 1.0},
                        {"ap": "hat", "blob": "blank", "scale": 1.0}]),
        ("bat", [{"ap": "head", "blob": "v3", "scale": 1.0}]),
    ]:
        placements = []
        for p in pl:
            t = (p["ap"], blobs[p["blob"]], p["scale"])
            if "tint" in p:
                tint = tuple(p["tint"])
                t += ((tint, p["max_off"]),) if "max_off" in p else (tint,)
            placements.append(t)
        model = assembly.assemble_placements(placements, rig_name)
        baked = assembly.placement_voxels(placements, rig_name)
        out["placements"].append({"rig": rig_name, "placements": pl, "model": model,
                                  "baked": [[list(k), list(v)] for k, v in baked.items()]})
    for rig_name, parts in [("adventurer", [("head", "head"), ("hat", "hat", 0.5), ("r_hand", "sword"), ("nope", "hat")]),
                            ("nope", [("head", "head")])]:
        built = assembly.assemble_voxels([(p[0], blobs[p[1]], *p[2:]) for p in parts], rig_name)
        out["voxels"].append({"rig": rig_name, "parts": [list(p) for p in parts], "baked": [[list(k), list(v)] for k, v in built.items()]})
    # numpy's BLAS rounding (gemm/gemv order, getf2 inverse, pairwise means) across every baked rig
    out["unbury"] = []
    rigs = assembly._rigs()
    for rig_name in sorted(rigs):
        for ap in list(rigs[rig_name]["rest"])[:3]:
            g = assembly.assemble_placements([(ap, cube, 1.0), (ap, glow, 0.5)], rig_name)["parts"][-1]
            out["unbury"].append([rig_name, ap, g["x"][0], g["y"][0], g["z"][0]])
    return out


# --- routes through the app ------------------------------------------------------------

async def asgi(app, method, url, headers, body=b""):
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
    return start["status"], [(k.lower().decode(), v.decode("latin-1")) for k, v in start["headers"]], raw


def multipart(parts):
    boundary = "kiwiboundary7MA4YWxkTrZu0gW"
    out = b""
    for name, filename, data in parts:
        out += f"--{boundary}\r\n".encode()
        disp = f'form-data; name="{name}"'
        if filename is not None:
            disp += f'; filename="{filename}"'
        out += f"Content-Disposition: {disp}\r\n".encode("utf-8")
        if filename is not None:
            out += b"Content-Type: application/octet-stream\r\n"
        out += b"\r\n" + (data.encode("utf-8") if isinstance(data, str) else data) + b"\r\n"
    out += f"--{boundary}--\r\n".encode()
    return f"multipart/form-data; boundary={boundary}", out


KEPT = ("content-type", "cache-control", "content-disposition", "x-kiwi-packed", "x-kiwi-moved", "x-kiwi-skipped",
        "x-kiwi-summary", "x-kiwi-cache", "vary")
W = "/site/mod-workshop/"
E = "/site/blueprint-editor/"


def workshop_cases():
    zips = zip_cases()
    loose = ["MyMod/blueprints/equipment_weapon_1h_sword_001.blueprint", "MyMod/ui/pvpplayercounterui.swf",
             "MyMod/ui/settings.cfg", "MyMod/preview.png", "MyMod/notes.txt"]
    loose_data = [SWORD, SWF, b"a=1\n", b"\x89PNG not really", b"notes"]
    files = [("files", p.rsplit("/", 1)[-1], d) for p, d in zip(loose, loose_data, strict=True)]
    spec = json.dumps({"properties": {"title": "My \u00c9pic: Mod?", "author": "Kiwi, Bird", "modVersion": "1.0",
                                      "notes": "first\nsecond", "tags": ["Dragons", "gui", "nope"]},
                       "fix": True, "keep": [], "config_path": "MyMod/ui/settings.cfg", "preview_path": "MyMod/preview.png"})
    return [
        ("inspect_paths", W + "inspect", [("paths", None, json.dumps(loose + ["MyMod/.DS_Store", "MyMod/blueprints/c_m_goldenbeetle_head.blueprint"]))]),
        ("inspect_nofix_alt", W + "inspect", [("paths", None, json.dumps(["c_m_goldenbeetle_head.blueprint", "ui/a.swf"]))]),
        ("inspect_zip", W + "inspect", [("archive", "mod.zip", zips["good"])]),
        ("inspect_tmod", W + "inspect", [("archive", "Test Mod.tmod", MOD)]),
        ("inspect_bad_json", W + "inspect", [("paths", None, "[nope")]),
        ("inspect_empty_list", W + "inspect", [("paths", None, "[]")]),
        ("inspect_not_list", W + "inspect", [("paths", None, '{"a": 1}')]),
        ("inspect_default", W + "inspect", []),
        ("inspect_too_many", W + "inspect", [("paths", None, json.dumps(list(range(4001))))]),
        ("inspect_text_archive", W + "inspect", [("archive", None, "zip")]),
        ("inspect_bad_zip", W + "inspect", [("archive", "x.zip", zips["truncated"])]),
        ("inspect_encrypted", W + "inspect", [("archive", "x.zip", zips["encrypted"])]),
        ("inspect_bad_tmod", W + "inspect", [("archive", "x.tmod", b"\x01" * 30)]),
        ("inspect_empty_archive", W + "inspect", [("archive", "x.tmod", b"")]),
        ("inspect_nonstr_paths", W + "inspect", [("paths", None, '[1, null, true, 1.5, ["a"], {"b": 2}]')]),
        ("build_loose", W + "build", [("spec", None, spec), ("paths", None, json.dumps(loose))] + files),
        ("build_loose_nofix", W + "build", [("spec", None, json.dumps({"fix": False, "properties": {"title": ""}})),
                                            ("paths", None, json.dumps(["equipment_weapon_1h_sword_001.blueprint", "ui/a.swf"])),
                                            ("files", "a", SWORD), ("files", "b", SWF)]),
        ("build_keep", W + "build", [("spec", None, json.dumps({"keep": ["c_m_goldenbeetle_head.blueprint"], "properties": []})),
                                     ("paths", None, json.dumps(["c_m_goldenbeetle_head.blueprint", "blueprints/x.blueprint"])),
                                     ("files", "a", HEAD), ("files", "b", HAT)]),
        ("build_tmod_rebuild", W + "build", [("spec", None, json.dumps({"properties": {"title": "Renamed", "tags": [], "author": ""}})),
                                             ("archive", "Test Mod.tmod", MOD)]),
        ("build_tmod_keep_header", W + "build", [("spec", None, "{}"), ("archive", "Test Mod.tmod", MOD)]),
        ("build_zip", W + "build", [("spec", None, json.dumps({"properties": {"title": "Zipped"}})), ("archive", "m.zip", zips["good"])]),
        ("build_attached", W + "build", [("spec", None, json.dumps({"properties": {"title": "Att"}, "config_path": "ignored"})),
                                         ("paths", None, json.dumps(["ui/pvpplayercounterui.swf"])), ("files", "x.swf", SWF),
                                         ("config", "C:\\Users\\me\\My.cfg", b"k=v"), ("preview", "pic.GIF", b"GIF89a")]),
        ("build_not_buildable", W + "build", [("spec", None, "{}"), ("paths", None, json.dumps(["readme.md"])), ("files", "r", b"x")]),
        ("build_cfg_no_swf", W + "build", [("spec", None, json.dumps({"config_path": "blueprints/x.cfg"})),
                                           ("paths", None, json.dumps(["blueprints/a.blueprint", "blueprints/x.cfg"])),
                                           ("files", "a", SWORD), ("files", "b", b"k=v")]),
        ("build_cfg_binary", W + "build", [("spec", None, json.dumps({"config_path": "ui/x.cfg"})),
                                           ("paths", None, json.dumps(["ui/a.swf", "ui/x.cfg"])),
                                           ("files", "a", SWF), ("files", "b", b"k\x00v")]),
        ("build_preview_webp", W + "build", [("spec", None, json.dumps({"preview_path": "ui/p.webp"})),
                                             ("paths", None, json.dumps(["ui/a.swf", "ui/p.webp"])),
                                             ("files", "a", SWF), ("files", "b", b"RIFF")]),
        ("build_preview_missing", W + "build", [("spec", None, json.dumps({"preview_path": "ui/p.png"})),
                                                ("paths", None, json.dumps(["ui/a.swf"])), ("files", "a", SWF)]),
        ("build_preview_empty", W + "build", [("spec", None, json.dumps({"preview_path": "ui/p.png"})),
                                              ("paths", None, json.dumps(["ui/a.swf", "ui/p.png"])), ("files", "a", SWF), ("files", "p", b"")]),
        ("build_mismatch", W + "build", [("spec", None, "{}"), ("paths", None, json.dumps(["a", "b"])), ("files", "a", b"x")]),
        ("build_no_files", W + "build", [("spec", None, "{}")]),
        ("build_bad_spec", W + "build", [("spec", None, "[1]"), ("paths", None, "[]")]),
        ("build_spec_missing", W + "build", [("paths", None, "[]"), ("files", None, "text")]),
        ("build_bad_paths", W + "build", [("spec", None, "{}"), ("paths", None, "nope")]),
        ("build_props_list_tmod", W + "build", [("spec", None, json.dumps({"properties": ["x"]})), ("archive", "Test Mod.tmod", MOD)]),
        ("build_tags_number", W + "build", [("spec", None, json.dumps({"properties": {"tags": 5}})),
                                            ("paths", None, json.dumps(["ui/a.swf"])), ("files", "a", SWF)]),
        ("extract_ok", W + "extract", [("file", "Test Mod.tmod", MOD)]),
        ("extract_plain", W + "extract", [("file", "p.tmod", PLAIN_MOD)]),
        ("extract_bad", W + "extract", [("file", "p.tmod", b"nope")]),
        ("extract_missing", W + "extract", [("other", None, "x")]),
        ("blueprint_json", W + "preview/blueprint", [("file", "m.tmod", MOD), ("path", None, " Blueprints/Equipment/Weapon/EQUIPMENT_WEAPON_1H_SWORD_001.blueprint ")]),
        ("blueprint_bin", W + "preview/blueprint?fmt=bin", [("file", "m.tmod", MOD), ("path", None, "blueprints/equipment/weapon/equipment_weapon_1h_sword_001.blueprint")]),
        ("blueprint_blank", W + "preview/blueprint", [("file", "m.tmod", MOD), ("path", None, "blueprints/blank.blueprint")]),
        ("blueprint_missing", W + "preview/blueprint", [("file", "m.tmod", MOD), ("path", None, "ui/pvpplayercounterui.swf")]),
        ("blueprint_empty", W + "preview/blueprint", [("file", "m.tmod", b""), ("path", None, "x.blueprint")]),
        ("blueprint_bad_fmt", W + "preview/blueprint?fmt=xml", [("path", None, "x")]),
        ("blueprint_bad_tmod", W + "preview/blueprint", [("file", "m.tmod", b"\x02" * 20), ("path", None, "x.blueprint")]),
        ("scripts_no_path", W + "swf/scripts", [("file", "m.tmod", MOD)]),
        ("scripts_long_path", W + "swf/scripts", [("file", "m.tmod", MOD), ("path", None, "a" * 401)]),
        ("scripts_empty", W + "swf/scripts", [("file", "m.tmod", b""), ("path", None, "ui/a.swf")]),
        ("scripts_missing", W + "swf/scripts", [("file", "m.tmod", MOD), ("path", None, "ui/nope.swf")]),
        ("scripts_not_swf", W + "swf/scripts", [("file", "m.tmod", MOD), ("path", None, "prefabs/collections/flask/effect_crit.binfab")]),
        ("scripts_bad_tmod", W + "swf/scripts", [("file", "m.tmod", b"zzzzzzzzzzzzzzzz"), ("path", None, "ui/a.swf")]),
        ("binfab_ok", W + "preview/binfab", [("file", "m.tmod", MOD), ("path", None, "\\Prefabs\\Collections\\flask\\effect_crit.BINFAB")]),
        ("binfab_missing", W + "preview/binfab", [("file", "m.tmod", MOD), ("path", None, "prefabs/nope.binfab")]),
        ("binfab_not_binfab", W + "preview/binfab", [("file", "m.tmod", MOD), ("path", None, "ui/pvpplayercounterui.swf")]),
        ("binfab_garbage", W + "preview/binfab", [("file", "m.tmod", tmod_bytes([("prefabs/x.binfab", b"\x00\x01")])), ("path", None, "prefabs/x.binfab")]),
        ("binfab_bad_tmod", W + "preview/binfab", [("file", "m.tmod", b""), ("path", None, "a.binfab")]),
        ("download_zip", W + "extract/download", [("file", "dir/Test Mod.TMOD", MOD)]),
        ("download_zip_noname", W + "extract/download", [("file", "", PLAIN_MOD)]),
        ("download_one", W + "extract/download", [("file", "m.tmod", MOD), ("path", None, "./ui/Test Mod.cfg")]),
        ("download_missing", W + "extract/download", [("file", "m.tmod", MOD), ("path", None, "ui/x.cfg")]),
        ("download_bad", W + "extract/download", [("file", "m.tmod", b"bad")]),
    ]


def model_cases():
    zips = {"mixed": mkzip([("Mod/blueprints/c_m_goldenbeetle_head.blueprint", HEAD), ("Mod/blueprints/c_c_snowman_hat_01_lvl2[techno].blueprint", HAT),
                            ("Mod/blueprints/blank.blueprint", BLANK), ("Mod/textures/t.dds", b"dds")])}
    mod = tmod_bytes([("blueprints/c_m_goldenbeetle_head.blueprint", HEAD), ("blueprints/equipment_weapon_1h_sword_001.blueprint", SWORD),
                      ("blueprints/char_hair_face_mustache_001.blueprint", STACHE), ("ui/x.swf", SWF)],
                     {"title": "Beetle", "tags": "Mounts", "modLoader": "BTT"})
    nobake = tmod_bytes([("blueprints/c_c_snowman_hat_01.blueprint", HAT)])
    paint = json.dumps({"blueprints/c_m_goldenbeetle_head.blueprint": [{"i": 0, "rgb": 0xFF0000}]})
    return [
        ("model_tmod", E + "model", [("file", "dir/Beetle.tmod", mod)]),
        ("model_zip", E + "model", [("file", "mixed.zip", zips["mixed"])]),
        ("model_loose", E + "model", [("files", "a\\c_c_snowman_hat_01.blueprint", HAT), ("files", "e_cave_a_airlock_branch.blueprint", bp("e_cave_a_airlock_branch.blueprint")),
                                      ("files", "", b"skip")]),
        ("model_ghost_rig", E + "model", [("files", "e_cave_a_airlock_branch.blueprint", bp("e_cave_a_airlock_branch.blueprint")), ("files", "s_hat.blueprint", bp("s_hat.blueprint"))]),
        ("model_unbaked", E + "model", [("file", "n.tmod", nobake)]),
        ("model_only_blank", E + "model", [("files", "blank.blueprint", BLANK)]),
        ("model_no_blueprints", E + "model", [("file", "x.tmod", tmod_bytes([("ui/x.swf", SWF)]))]),
        ("model_not_blueprint", E + "model", [("files", "x.qb", b"qb")]),
        ("model_nothing", E + "model", []),
        ("model_empty_file", E + "model", [("file", "x.tmod", b"")]),
        ("model_bad_zip", E + "model", [("file", "x.zip", b"PK\x05\x06")]),
        ("model_text_files", E + "model", [("files", None, "t")]),
        ("save_tmod", E + "model-save", [("file", "Beetle.tmod", mod), ("edits", None, paint),
                                         ("moves", None, json.dumps({"blueprints/equipment_weapon_1h_sword_001.blueprint": [1, 0, -2],
                                                                     "blueprints/char_hair_face_mustache_001.blueprint": [0, 0.0, False]})),
                                         ("files", "new.blueprint", HAT), ("paths", None, json.dumps(["parts/added"]))]),
        ("save_zip", E + "model-save", [("file", "mixed.ZIP", zips["mixed"]), ("name", None, "dir/Out.zip"),
                                        ("moves", None, json.dumps({"Mod/blueprints/blank.blueprint": None}))]),
        ("save_loose", E + "model-save", [("files", "c_c_snowman_hat_01.blueprint", HAT), ("files", "h.blueprint", HEAD),
                                          ("edits", None, json.dumps({"h.blueprint": [{"i": 1, "rgb": 0x00FF00}]})), ("name", None, "Beetle \u00e9")]),
        ("save_replace", E + "model-save", [("file", "Beetle.tmod", mod), ("files", "x", SWORD),
                                            ("paths", None, json.dumps(["blueprints/c_m_goldenbeetle_head.blueprint"]))]),
        ("save_null_path", E + "model-save", [("file", "Beetle.tmod", mod), ("files", "named.blueprint", SWORD), ("paths", None, "[null]")]),
        ("save_unknown", E + "model-save", [("file", "Beetle.tmod", mod), ("edits", None, json.dumps({"nope.blueprint": [{"i": 0}]}))]),
        ("save_bad_json", E + "model-save", [("file", "Beetle.tmod", mod), ("moves", None, "{")]),
        ("save_bad_types", E + "model-save", [("file", "Beetle.tmod", mod), ("paths", None, "{}")]),
        ("save_mismatch", E + "model-save", [("file", "Beetle.tmod", mod), ("files", "a", SWORD)]),
        ("save_bad_move", E + "model-save", [("file", "Beetle.tmod", mod),
                                             ("moves", None, json.dumps({"blueprints/equipment_weapon_1h_sword_001.blueprint": [1, "x", 0]}))]),
        ("save_move_string", E + "model-save", [("file", "Beetle.tmod", mod),
                                                ("moves", None, json.dumps({"blueprints/equipment_weapon_1h_sword_001.blueprint": "123"}))]),
        ("save_far_move", E + "model-save", [("file", "Beetle.tmod", mod),
                                             ("moves", None, json.dumps({"blueprints/equipment_weapon_1h_sword_001.blueprint": [10000, 0, 0]}))]),
        ("save_bad_edit", E + "model-save", [("file", "Beetle.tmod", mod),
                                             ("edits", None, json.dumps({"blueprints/c_m_goldenbeetle_head.blueprint": [{"i": -5}]}))]),
        ("save_nothing", E + "model-save", []),
    ]


GAME_MODEL = ["adv_knight", "beetle", "prefabs/npc/beetle", "nope", "prefabs/collections/mount/nobake.binfab",
              "prefabs/collections/mount/ghost.binfab", "prefabs/collections/mount/lonely", "", None]


async def routes(cases, *, rid_prefix):
    import app.core.ratelimit as core_rl
    import app.embed.service as embed_service
    import app.site.router as site_router
    from app.main import app

    async def no_limit(*a, **k):
        return None

    site_router.check_rate_limit = no_limit
    core_rl.check_rate_limit = no_limit
    embed_service._read_game_file = _fake_read
    out = []
    for i, (name, path, parts) in enumerate(cases):
        rid = f"{rid_prefix}-{i}"
        if parts is None:
            status, headers, raw = await asgi(app, "GET", path, [(b"x-request-id", rid.encode())])
            body, ctype = b"", ""
        else:
            ctype, body = multipart(parts)
            status, headers, raw = await asgi(app, "POST", path, [(b"content-type", ctype.encode()),
                                                                  (b"x-request-id", rid.encode())], body)
        kept = [[k, v] for k, v in headers if k in KEPT]
        out.append({"name": name, "path": path, "method": "GET" if parts is None else "POST", "content_type": ctype,
                    "request_id": rid, "body": b64(body), "status": status, "headers": kept, "response": b64(raw)})
    return out


def game_model_cases():
    cases = []
    for p in GAME_MODEL:
        q = "" if p is None else "?" + urllib.parse.urlencode({"prefab": p})
        cases.append((f"game_{p}", E + "game-model" + q, None))
    return cases


def rig_route_cases():
    return [("anim_ok", "/site/rigs/adventurer/anim/unarmed_ability_mine_begin", None),
            ("anim_missing", "/site/rigs/adventurer/anim/nope", None),
            ("anim_bad_name", "/site/rigs/Adventurer/anim/x", None),
            ("anim_dots", "/site/rigs/adventurer/anim/..%5Cx", None),
            ("graph_ok", "/site/rigs/adventurer/graph", None),
            ("graph_missing", "/site/rigs/nope/graph", None)]


def specs(cases, *, compare_headers):
    out = []
    for name, path, parts in cases:
        if parts is None:
            out.append({"name": name, "method": "GET", "path": path, "compare_headers": compare_headers})
            continue
        ctype, body = multipart(parts)
        spec = {"name": name, "method": "POST", "path": path, "headers": {"Content-Type": ctype},
                "body_b64": b64(body), "compare_headers": compare_headers}
        if name in ("download_zip", "download_zip_noname", "save_zip", "save_loose"):
            spec["allow_diff"] = ["body"]
            spec["reason"] = "zip entries carry the wall-clock write time"
        out.append(spec)
    return out


def write_spec(name, items):
    (PARITY / name).write_text("[\n" + ",\n".join(" " + json.dumps(s) for s in items) + "\n]\n", encoding="utf-8")


def main():
    OUT_W.mkdir(parents=True, exist_ok=True)
    OUT_R.mkdir(parents=True, exist_ok=True)
    (OUT_W / "engine.json").write_text(json.dumps(engine(), ensure_ascii=False), encoding="utf-8")
    (OUT_W / "routes.json").write_text(json.dumps(asyncio.run(routes(workshop_cases(), rid_prefix="ws"))), encoding="utf-8")
    (OUT_R / "engine.json").write_text(json.dumps(rig_engine(), ensure_ascii=False), encoding="utf-8")
    rig_cases = model_cases() + game_model_cases() + rig_route_cases()
    (OUT_R / "routes.json").write_text(json.dumps(asyncio.run(routes(rig_cases, rid_prefix="rig"))), encoding="utf-8")
    heads = ["content-disposition", "x-kiwi-packed", "x-kiwi-moved", "x-kiwi-skipped", "x-kiwi-summary"]
    write_spec("workshop.writes.json", specs(workshop_cases(), compare_headers=heads))
    write_spec("rigs.writes.json", specs(rig_cases, compare_headers=heads))
    sizes = {p.name: p.stat().st_size for d in (OUT_W, OUT_R) for p in d.rglob("*") if p.is_file()}
    print(sizes)


if __name__ == "__main__":
    main()
