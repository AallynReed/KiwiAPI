"""Golden fixtures for the Blueprint Editor and voxel renderer port, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_blueprints_fixtures.py [E:/Trove/blueprints]

Writes native/kiwi-server/testdata/blueprints/: the input blueprints (bp/, a few
small game files plus synthetic ones), engine.json (every editor function over
those inputs, errors included), routes.json (the /site/blueprint-editor/* routes
driven through the real app at the ASGI level, rate limit stubbed, zip clock
pinned) and render/*.png (thumbnails as Python draws them).

kiwi_native is absent here, so v5 bytes come from CPython's zlib; the Rust tests
compare v5 files by their inflated body.
"""
import asyncio
import base64
import io
import json
import logging
import os
import shutil
import struct
import sys
import time
import types
import urllib.parse
import warnings
import zipfile
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "blueprints-fixture-secret")

from app.admin import runtime_config as rc  # noqa: E402

for _key, _spec in rc.REGISTRY.items():
    rc._CACHE[_key] = (_spec.default, float("inf"))

from app.trove.blueprint import codec, editor, qb  # noqa: E402
from app.trove.render import binpack, voxel  # noqa: E402

OUT = Path(__file__).resolve().parents[2] / "kiwi-server" / "testdata" / "blueprints"
GAME = Path(sys.argv[1]) if len(sys.argv) > 1 else Path("E:/Trove/blueprints")
GAME_FILES = [
    "equipment_weapon_1h_sword_001.blueprint",
    "equipment_weapon_1h_sword_027[nim].blueprint",
    "c_m_goldenbeetle_head.blueprint",
    "ability_discovery_thumper.blueprint",
    "ability_pvp_block_spawn_cage_01.blueprint",
    "e_cave_a_airlock_branch.blueprint",
    "e_cave_a_airlock_tier2_npc_diggsly_shadow.blueprint",
    "c_c_snowman_hat_01.blueprint",
    "char_hair_face_mustache_001.blueprint",
    "blank.blueprint",
]
ZIP_CLOCK = 1761130800  # 2025-10-22 11:00:00 UTC


def b64(b):
    return base64.b64encode(b).decode()


def synth():
    """Inputs no game file covers: procedural types, specular 5-7, a hat whose
    attachment sits under the box, a deco with an unreadable entity section."""
    vox = []
    for x in range(4):
        for y in range(3):
            vox.append({"x": x, "y": y, "z": 0, "r": 10 * x, "g": 200, "b": 30 * y, "w": (x + y) % 8, "type": 21})
    vox.append({"x": 0, "y": 0, "z": 1, "r": 5, "g": 5, "b": 5, "w": 0, "type": 1})
    vox.append({"x": 1, "y": 0, "z": 1, "r": 4, "g": 0, "b": 2, "w": 0, "type": 174})
    vox.append({"x": 2, "y": 0, "z": 1, "r": 3, "g": 3, "b": 3, "w": 2, "type": 39})
    vox.append({"x": 3, "y": 2, "z": 3, "r": 90, "g": 90, "b": 250, "w": 5, "type": 18})
    vox.append({"x": 3, "y": 2, "z": 2, "r": 250, "g": 20, "b": 20, "w": 0, "type": 56})
    mixed = codec.encode(vox, version=5, pos=(-1, 7, -2), size=(4, 3, 4),
                         entity_blob=codec.write_entity_records([(1, 0, 1, b"\x0a\x0dplaceable/x\x1e")]))
    hat = codec.encode([{"x": x, "y": 0, "z": z, "r": 200, "g": 40, "b": 40, "w": 1, "type": 21}
                        for x in range(5) for z in range(5)], version=5, pos=(-2, 8, -2))
    bad_ent = codec.encode([{"x": 0, "y": 0, "z": 0, "r": 9, "g": 9, "b": 9, "type": 21},
                            {"x": 2, "y": 0, "z": 0, "r": 200, "g": 9, "b": 9, "type": 21}],
                           version=5, entity_blob=b"\x02\x00\x00\x00\x02\x02")
    v3 = codec.encode([{"x": x, "y": x % 2, "z": 0, "r": 100, "g": 120, "b": 140, "w": x % 3, "type": 54 if x else 21}
                       for x in range(5)], version=3, offset=(-2, 0, -1))
    v4 = codec.encode([{"x": 0, "y": 0, "z": 0, "r": 1, "g": 2, "b": 3, "type": 21},
                       {"x": 1, "y": 1, "z": 1, "r": 200, "g": 2, "b": 3, "type": 55, "w": 3}],
                      version=4, offset=(0, 0, 0), entity_blob=codec.write_entity_records([(0, 1, 0, b"abc")]))
    glass = codec.encode([{"x": x, "y": y, "z": z, "r": 40 + 20 * x, "g": 120, "b": 220,
                           "w": (x + z) % 8, "type": [18, 54, 56, 21, 55][(x + y + z) % 5]}
                          for x in range(5) for y in range(4) for z in range(3)], version=5, pos=(-2, 0, -1))
    return {
        "s_mixed.blueprint": mixed,
        "s_hat.blueprint": hat,
        "s_bad_entities.blueprint": bad_ent,
        "s_v3.blueprint": v3,
        "s_v4.blueprint": v4,
        "s_glass.blueprint": glass,
        "s_v5_short.blueprint": codec.MAGIC + struct.pack("<I", 5) + __import__("zlib").compress(
            struct.pack("<8i", 0, 0, 0, 2, 2, 2, 3, 0)),
        "s_v5_empty.blueprint": codec.MAGIC + struct.pack("<I", 5) + __import__("zlib").compress(
            struct.pack("<8i", 0, 0, 0, 2, 2, 2, 0, 0)),
        "s_v3_trunc.blueprint": codec.MAGIC + struct.pack("<I", 3) + b"\x02\x02\x02\x02\x15\x00",
        "s_v9.blueprint": codec.MAGIC + struct.pack("<I", 9) + b"\x00" * 8,
        "s_nomagic.blueprint": b"not a blueprint at all",
        "s_badzlib.blueprint": codec.MAGIC + struct.pack("<I", 5) + b"\x78\x9c\xff\xff\xff\xff",
    }


def inputs():
    bp = OUT / "bp"
    if bp.exists():
        shutil.rmtree(bp)
    bp.mkdir(parents=True)
    out = {}
    for name in GAME_FILES:
        data = (GAME / name).read_bytes()
        (bp / name).write_bytes(data)
        out[name] = data
    for name, data in synth().items():
        (bp / name).write_bytes(data)
        out[name] = data
    return out


def outcome(fn):
    try:
        return {"ok": fn()}
    except Exception as exc:  # noqa: BLE001
        return {"error": type(exc).__name__, "message": str(exc)}


def blob(data, summary=None):
    return {"bytes": b64(data), "summary": summary}


WIDE = {"equipment_weapon_1h_sword_001.blueprint", "s_mixed.blueprint", "e_cave_a_airlock_branch.blueprint",
        "c_m_goldenbeetle_head.blueprint", "s_bad_entities.blueprint", "s_v5_short.blueprint"}

EDIT_SETS = {
    "none": [],
    "paint": [{"i": 0, "rgb": 0x123456}, {"i": 1, "type": 18, "w": 9}, {"i": 1, "rgb": "65280"},
              {"i": 2, "del": 1}, {"i": 3, "type": 55, "w": 4, "rgb": 0}],
    "grow": [{"add": [-1, 0, -2], "type": 21, "w": 3, "rgb": 0xFF00FF}, {"add": [0, 0, 0], "type": 56, "w": 7, "rgb": 1},
             {"add": ["40", 2.9, 1], "type": "54", "w": 1, "rgb": 99}],
    "erase_edge": [{"i": 0, "del": True}, {"i": 0, "del": True}],
    "bool_idx": [{"i": True, "rgb": 5}, {"i": 2.7, "type": 21, "w": 1}],
    "not_list": {"i": 0},
    "not_dict": [3],
    "missing_i": [{"rgb": 1}],
    "out_of_range": [{"i": 999999, "rgb": 1}],
    "bad_type": [{"i": 0, "type": 99, "w": 0}],
    "half_material": [{"i": 0, "w": 2}],
    "bad_colour": [{"i": 0, "rgb": 0x1000000}],
    "str_colour": [{"i": 0, "rgb": "red"}],
    "bad_coords": [{"add": [1, 2], "type": 21, "w": 0, "rgb": 1}],
    "coords_type": [{"add": [1, None, 2], "type": 21, "w": 0, "rgb": 1}],
    "far_add": [{"add": [5000, 0, 0], "type": 21, "w": 0, "rgb": 1}],
    "add_no_rgb": [{"add": [1, 1, 1], "type": 21, "w": 0}],
    "add_no_material": [{"add": [1, 1, 1], "rgb": 1}],
}


def engine(files):
    out = {"inspect": [], "edits": [], "transform": [], "composite": [], "export": [], "import": [],
           "check": [], "pack": [], "decode": []}
    for name, data in files.items():
        out["inspect"].append({"file": name, **outcome(lambda: editor.inspect(data, name=name))})
        out["pack"].append({"file": name, **outcome(lambda: voxel.pack_blueprint(data, "blueprints/" + name))})
        out["decode"].append({"file": name, "empty": codec.is_empty_blueprint(data), **outcome(
            lambda: [len(codec.decode_full(data).voxels), list(codec.decode_full(data).size)])})
        wide = name in WIDE
        for set_name, edits in EDIT_SETS.items():
            if not wide and set_name not in ("none", "paint", "grow"):
                continue
            out["edits"].append({"file": name, "edits": set_name, **outcome(
                lambda: blob(*editor.apply_edits(data, edits)))})
        for ops in (["rotate_x"], ["rotate_y", "mirror_z", "rotate_z"], ["mirror_x", "mirror_y"], [], "rotate_x",
                    ["bogus"], [1], [None], [["rotate_x"]], ["rotate_x"] * 17)[: None if wide else 2]:
            out["transform"].append({"file": name, "ops": ops, **outcome(
                lambda: blob(*editor.transform(data, [], ops)))})
        out["export"].append({"file": name, **outcome(lambda: export(data, name))})
        for kind in ("melee", "gun", "staff", "bow", "spear", "fist", "hat", "mask", "deco", "other", "nope"):
            out["check"].append({"file": name, "kind": kind, **outcome(lambda: editor.check(data, [], kind))})

    sword, nim, mixed = files["equipment_weapon_1h_sword_001.blueprint"], files["equipment_weapon_1h_sword_027[nim].blueprint"], files["s_mixed.blueprint"]
    hat, ent = files["s_hat.blueprint"], files["e_cave_a_airlock_branch.blueprint"]
    stacks = [
        ("attach", sword, [hat], [{"mode": "attachment", "offset": [1, 0, -1]}], 0),
        ("corner_anchor1", sword, [mixed], [{"mode": "corner", "offset": [-3, 2, 0], "edits": [{"i": 0, "rgb": 7}]}], 1),
        ("centre_two", ent, [sword, hat], [{"mode": "centre"}, {"mode": "centre", "offset": "123"}], 2),
        ("v3_attach", sword, [nim], [{}], 0),
        ("bad_mode", sword, [hat], [{"mode": "sideways"}], 0),
        ("mismatch", sword, [hat], [], 0),
        ("not_dict", sword, [hat], ["x"], 0),
        ("bad_offset", sword, [hat], [{"offset": [1, 2]}], 0),
        ("far_offset", sword, [hat], [{"offset": [0, 9999, 0]}], 0),
        ("offset_int", sword, [hat], [{"offset": 5}], 0),
        ("anchor_out", sword, [hat], [{}], 2),
        ("too_many", sword, [hat] * 9, [{}] * 9, 0),
        ("empty_layer", sword, [files["blank.blueprint"]], [{}], 0),
        ("entities", ent, [ent], [{"mode": "corner", "offset": [-2, 0, 0]}], 0),
        ("bad_entities", ent, [files["s_bad_entities.blueprint"]], [{"mode": "corner"}], 0),
    ]
    for name, base, layers, specs, anchor in stacks:
        out["composite"].append({"name": name, "base": key_of(files, base), "layers": [key_of(files, b) for b in layers],
                                 "specs": specs, "anchor_at": anchor, "edits": EDIT_SETS["paint"], **outcome(
                lambda: blob(*editor.composite(base, EDIT_SETS["paint"], layers, specs, anchor)))})

    for name in ("equipment_weapon_1h_sword_001.blueprint", "s_mixed.blueprint", "s_hat.blueprint",
                 "equipment_weapon_1h_sword_027[nim].blueprint", "s_glass.blueprint"):
        built = qb.from_blueprint(codec.decode_full(files[name]), stem="m")
        out["import"].append({"case": "roundtrip:" + name, "files": {k: b64(v) for k, v in built["files"].items()},
                              **outcome(lambda: blob(*editor.import_qb(built["files"])))})
    for case, qbs in qb_cases().items():
        out["import"].append({"case": case, "files": {k: b64(v) for k, v in qbs.items()},
                              **outcome(lambda: blob(*editor.import_qb(qbs)))})
    return out


def key_of(files, data):
    return next(k for k, v in files.items() if v is data)


def export(data, name):
    edited, _ = editor.composite(data, [], None, None, 0)
    built = qb.from_blueprint(codec.decode_full(edited), stem=name.split(".")[0])
    return {"files": {k: b64(v) for k, v in built["files"].items()}, "notes": built["notes"],
            "attachment": built["attachment"], "size": built["size"]}


def raw_qb(matrices, *, colour=0, zor=1, compression=0):
    """A .qb in any of the header's layouts, for the import paths export never writes."""
    out = struct.pack("<6I", 257, colour, zor, compression, 0, len(matrices))
    for name, size, pos, cells in matrices:
        out += bytes([len(name)]) + name + struct.pack("<3I", *size) + struct.pack("<3i", *pos)
        sx, sy, sz = size
        if compression == 0:
            for z in range(sz):
                for y in range(sy):
                    for x in range(sx):
                        c = cells.get((x, y, z))
                        out += bytes([*c, 255]) if c else b"\0\0\0\0"
        else:
            for z in range(sz):
                for y in range(sy):
                    for x in range(sx):
                        c = cells.get((x, y, z))
                        out += bytes([*c, 255]) if c else b"\0\0\0\0"
                out += struct.pack("<I", 6)
    return out


def qb_cases():
    cube = {(x, y, z): (20 + 40 * x, 60, 30 * z) for x in range(3) for y in range(2) for z in range(4)}
    marked = dict(cube)
    marked[(1, 0, 1)] = (255, 0, 255)
    floating = dict(cube)
    floating[(1, 3, 1)] = (255, 0, 255)
    tmap = {k: (128, 128, 128) if k[0] == 0 else (255, 0, 0) if k[0] == 1 else (1, 2, 3) for k in cube}
    amap = {k: (80, 80, 80) if k[2] == 0 else (255, 255, 255) if k[2] == 1 else (7, 7, 7) for k in cube}
    smap = {k: (0, 128, 0) if k[1] == 0 else (9, 9, 9) for k in cube}
    m = lambda cells, size=(3, 2, 4), pos=(0, 0, 0): [(b"M", size, pos, cells)]  # noqa: E731
    return {
        "bgra_lefthanded": {"a.qb": raw_qb(m(cube), colour=1, zor=0)},
        "rle_marker_maps": {"x.qb": raw_qb(m(marked), compression=1), "x_t.qb": raw_qb(m(tmap)),
                            "x_a.qb": raw_qb(m(amap)), "x_s.qb": raw_qb(m(smap))},
        "floating_marker": {"f.qb": raw_qb(m(floating, (3, 4, 4)))},
        "offset_attach": {"o.QB": raw_qb(m(cube, pos=(-1, -1, 0)))},
        "multi_matrix": {"mm.qb": raw_qb([(b"A", (3, 2, 4), (0, 0, 0), cube), (b"B", (1, 1, 1), (5, -2, 1), {(0, 0, 0): (9, 9, 9)})])},
        "map_mismatch": {"p.qb": raw_qb(m(cube)), "p_t.qb": raw_qb(m({(0, 0, 0): (255, 255, 255)}, (1, 1, 1)))},
        "no_base": {"q_a.qb": raw_qb(m(cube))},
        "only_marker": {"r.qb": raw_qb(m({(0, 0, 0): (255, 0, 255)}, (1, 1, 1)))},
        "short": {"s.qb": b"\x01\x01\x00\x00"},
        "no_matrices": {"n.qb": struct.pack("<6I", 257, 0, 1, 0, 0, 0)},
        "truncated": {"t.qb": raw_qb(m(cube))[:60]},
        "header_cut": {"h.qb": struct.pack("<6I", 257, 0, 1, 0, 0, 1) + b"\x01M\x03\x00"},
        "empty_grid": {"e.qb": raw_qb(m({}, (2, 2, 2)))},
    }


# --- routes through the app ----------------------------------------------------

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
    """``[(name, filename|None, bytes|str)]`` as a multipart body."""
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


KEPT = ("content-type", "cache-control", "content-disposition", "x-kiwi-summary", "x-kiwi-notes",
        "x-kiwi-recoloured", "x-kiwi-rematerialised", "x-kiwi-ignored")


def route_cases(files):
    sword, hat, nim = (files[k] for k in ("equipment_weapon_1h_sword_001.blueprint", "s_hat.blueprint",
                                          "equipment_weapon_1h_sword_027[nim].blueprint"))
    mixed = files["s_mixed.blueprint"]
    built = qb.from_blueprint(codec.decode_full(sword), stem="sw")["files"]
    paint = json.dumps(EDIT_SETS["paint"])
    p = "/site/blueprint-editor/"
    return [
        ("inspect_ok", p + "inspect", [("file", "dir/sword ✓.blueprint", sword)]),
        ("inspect_empty", p + "inspect", [("file", "x.blueprint", b"")]),
        ("inspect_placeholder", p + "inspect", [("file", "x.blueprint", files["blank.blueprint"])]),
        ("inspect_missing", p + "inspect", [("other", None, "1")]),
        ("inspect_text", p + "inspect", [("file", None, "hello")]),
        ("inspect_unhandled", p + "inspect", [("file", "x.blueprint", files["s_v5_short.blueprint"])]),
        ("save_ok", p + "save", [("file", "Épée \"best\".blueprint", sword), ("edits", None, paint)]),
        ("save_noext", p + "save", [("file", "plain", mixed), ("edits", None, "")]),
        ("save_bad_json", p + "save", [("file", "a.blueprint", sword), ("edits", None, "[{")]),
        ("save_editor_error", p + "save", [("file", "a.blueprint", sword), ("edits", None, '[{"i": -1}]')]),
        ("save_layers", p + "save", [("file", "a.blueprint", sword), ("layers", "h.blueprint", hat),
                                     ("stack", None, '[{"mode": "attachment", "offset": [0, 1, 0]}]'),
                                     ("anchor_at", None, "0")]),
        ("save_bad_stack", p + "save", [("file", "a.blueprint", sword), ("layers", "h.blueprint", hat),
                                        ("stack", None, "{}")]),
        ("save_bad_anchor", p + "save", [("file", "a.blueprint", sword), ("anchor_at", None, "x")]),
        ("save_layer_text", p + "save", [("file", "a.blueprint", sword), ("layers", None, "nope")]),
        ("save_unhandled", p + "save", [("file", "a.blueprint", files["s_bad_entities.blueprint"]),
                                        ("edits", None, '[{"add": [-1, 0, 0], "type": 21, "w": 0, "rgb": 5}]')]),
        ("flatten_no_layers", p + "flatten", [("file", "a.blueprint", sword)]),
        ("flatten_ok", p + "flatten", [("file", "a.blueprint", sword), ("layers", "b.blueprint", nim),
                                       ("layers", "c.blueprint", hat), ("anchor_at", None, "2"),
                                       ("stack", None, '[{"mode": "centre"}, {"mode": "corner"}]'),
                                       ("edits", None, paint)]),
        ("transform_ok", p + "transform", [("file", "a.blueprint", files["e_cave_a_airlock_branch.blueprint"]),
                                           ("ops", None, '["rotate_y", "mirror_x"]')]),
        ("transform_missing_ops", p + "transform", [("file", "a.blueprint", sword)]),
        ("transform_bad_json", p + "transform", [("file", "a.blueprint", sword), ("ops", None, "nope")]),
        ("transform_bad_op", p + "transform", [("file", "a.blueprint", sword), ("ops", None, '["spin"]')]),
        ("export_ok", p + "export-qb", [("file", "dir/My Sword!.blueprint", sword)]),
        ("export_weird_stem", p + "export-qb", [("file", "ü.BLUEPRINT", mixed), ("edits", None, paint)]),
        ("export_no_name", p + "export-qb", [("file", "", sword)]),
        ("import_ok", p + "import-qb", [("files", k, v) for k, v in built.items()]),
        ("import_not_qb", p + "import-qb", [("files", "a.vox", b"x")]),
        ("import_too_many", p + "import-qb", [("files", f"m{i}.qb", built["sw.qb"]) for i in range(5)]),
        ("import_none", p + "import-qb", [("x", None, "y")]),
        ("import_dupes", p + "import-qb", [("files", "sw.qb", b"junk"), ("files", "sw.qb", built["sw.qb"])]),
        ("import_short", p + "import-qb", [("files", "a.qb", b"\x01\x00")]),
        ("import_header_cut", p + "import-qb", [("files", "a.qb", struct.pack("<6I", 257, 0, 1, 0, 0, 1) + b"\x00")]),
        ("check_ok", p + "check", [("file", "a.blueprint", sword), ("kind", None, "melee"), ("edits", None, paint)]),
        ("check_hat", p + "check", [("file", "a.blueprint", hat), ("kind", None, "hat")]),
        ("check_default", p + "check", [("file", "a.blueprint", mixed)]),
    ]


async def routes(files):
    import app.site.router as site_router
    from app.main import app

    async def no_limit(*a, **k):
        return None

    site_router.check_rate_limit = no_limit
    zipfile.time = types.SimpleNamespace(time=lambda: ZIP_CLOCK, localtime=time.gmtime)
    out = []
    for i, (name, path, parts) in enumerate(route_cases(files)):
        ctype, body = multipart(parts)
        rid = f"bp-{i}"
        status, headers, raw = await asgi(app, "POST", path, [(b"content-type", ctype.encode()),
                                                              (b"x-request-id", rid.encode())], body)
        kept = [[k, v] for k, v in headers if k in KEPT]
        out.append({"name": name, "path": path, "content_type": ctype, "request_id": rid, "body": b64(body),
                    "status": status, "headers": kept, "response": b64(raw)})
    return out


def writes_spec(files):
    """native/port/parity/blueprints.writes.json: the editor routes for the twin
    sandbox (stateless, but POST)."""
    out = []
    for name, path, parts in route_cases(files):
        ctype, body = multipart(parts)
        spec = {"name": name, "method": "POST", "path": path, "headers": {"Content-Type": ctype},
                "body_b64": b64(body), "compare_headers": [h for h in KEPT if h not in ("content-type", "cache-control")]}
        if path.endswith("export-qb") and not name.endswith("_err"):
            spec["allow_diff"] = ["body"]
            spec["reason"] = "zip entries carry the wall-clock write time"
        out.append(spec)
    Path(__file__).with_name("blueprints.writes.json").write_text(
        "[\n" + ",\n".join(" " + json.dumps(s) for s in out) + "\n]\n", encoding="utf-8")


def renders(files):
    out_dir = OUT / "render"
    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True)
    out = []
    for name in ("equipment_weapon_1h_sword_001.blueprint", "s_glass.blueprint", "ability_discovery_thumper.blueprint",
                 "c_m_goldenbeetle_head.blueprint", "s_mixed.blueprint"):
        for dim in (64, 160):
            png = voxel.render_blueprint_png(files[name], dim, None, True)
            fn = f"{name.split('.')[0]}_{dim}.png"
            (out_dir / fn).write_bytes(png)
            out.append({"file": name, "dim": dim, "png": fn})
    for name in ("blank.blueprint", "s_nomagic.blueprint", "s_v5_empty.blueprint", "s_badzlib.blueprint",
                 "s_v9.blueprint", "s_v3_trunc.blueprint", "s_v5_short.blueprint"):
        out.append({"file": name, "dim": 64, **outcome(lambda: b64(voxel.render_blueprint_png(files[name], 64, None, True)))})
    return out


def kvx(files):
    payload = voxel.pack_blueprint(files["s_mixed.blueprint"], "blueprints/ü/s_mixed.blueprint")
    creature = {"rig": "x", "parts": [dict(payload, path="a"), {"path": "b", "x": [40000, 1], "y": [0, 1], "z": [0, 1],
                                                                 "rgb": [1, 2], "kind": [0, 300], "level": [255, 16]}],
                "count": 3}
    return [{"payload": p, "kvx": b64(binpack.encode(p))} for p in (payload, creature)]


def dds(w, h, bitcount, flags, masks, pixels, fourcc=0):
    head = struct.pack("<7I", 124, 0x100F, h, w, w * bitcount // 8, 0, 1) + b"\0" * 44
    head += struct.pack("<4I", 32, flags, fourcc, bitcount) + struct.pack("<4I", *(list(masks) + [0] * 4)[:4])
    head += struct.pack("<5I", 0x1000, 0, 0, 0, 0)
    return b"DDS " + head + pixels


def brdf_cases():
    """`_brdf_to_png` (Pillow's DDS reader, then RGB, then optimize=True) over the
    layouts the atlas could come in, plus ones Pillow refuses."""
    w, h = 9, 5
    bgra = bytes((x * 23 + y * 7) % 256 for y in range(h) for x in range(w) for _ in range(4))
    rgb24 = bytes((x * 31 + y * 3 + c) % 256 for y in range(h) for x in range(w) for c in range(3))
    r5g6b5 = b"".join(struct.pack("<H", (x * 2311 + y * 977) & 0xFFFF) for y in range(h) for x in range(w))
    cases = {
        "bgra8": dds(w, h, 32, 0x41, (0xFF0000, 0xFF00, 0xFF, 0xFF000000), bgra + b"\x11" * 64),
        "rgb24": dds(w, h, 24, 0x40, (0xFF0000, 0xFF00, 0xFF), rgb24),
        "rgb565": dds(w, h, 16, 0x40, (0xF800, 0x7E0, 0x1F), r5g6b5),
        "rgbx_odd_mask": dds(w, h, 32, 0x40, (0xFF, 0xFF00, 0xFF0000), bgra),
        "truncated": dds(w, h, 32, 0x41, (0xFF0000, 0xFF00, 0xFF, 0xFF000000), bgra[:40]),
        "dxt1": dds(4, 4, 0, 0x4, (0, 0, 0), b"\0" * 8, fourcc=0x31545844),
        "not_dds": b"PNG nope",
    }
    out_dir = OUT / "dds"
    if out_dir.exists():
        shutil.rmtree(out_dir)
    out_dir.mkdir(parents=True)
    from app.site.router import _brdf_to_png
    out = []
    for name, data in cases.items():
        (out_dir / f"{name}.dds").write_bytes(data)
        out.append({"file": f"{name}.dds", **outcome(lambda: b64(_brdf_to_png(data)))})
    return out


def main():
    files = inputs()
    (OUT / "engine.json").write_text(json.dumps(engine(files), ensure_ascii=False), encoding="utf-8")
    (OUT / "routes.json").write_text(json.dumps(asyncio.run(routes(files))), encoding="utf-8")
    writes_spec(files)
    (OUT / "render.json").write_text(json.dumps({"renders": renders(files), "kvx": kvx(files), "brdf": brdf_cases()},
                                                ensure_ascii=False), encoding="utf-8")
    sizes = {p.name: p.stat().st_size for p in OUT.rglob("*") if p.is_file()}
    print(sum(sizes.values()), "bytes", len(sizes), "files")


if __name__ == "__main__":
    main()
