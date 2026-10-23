"""Golden fixtures for kiwi-server's Mods Hub releases (``src/mod_releases``).

The release-section helpers of ``app/trove/mods_hub/service.py`` run on canned
documents (written as canonical extended JSON, so the Rust test loads the very
same BSON) and small artifacts packed from the repo's testdata; the content
store is a dict. Writes native/kiwi-server/testdata/mod_releases/golden.json.

Run from the repo root with the production-pinned venv:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mod_releases_fixtures.py
"""

from __future__ import annotations

import asyncio
import base64
import datetime as dt
import hashlib
import io
import json
import logging
import os
import sys
import warnings
import zipfile
from pathlib import Path

warnings.filterwarnings("ignore")
logging.disable(logging.CRITICAL)
ROOT = Path(__file__).resolve().parents[3]
TESTDATA = ROOT / "native" / "kiwi-server" / "testdata"
OUT = TESTDATA / "mod_releases" / "golden.json"
os.chdir(ROOT)
sys.path.insert(0, str(ROOT))
os.environ.setdefault("SECRET_KEY", "mod-releases-fixture-secret")

from beanie import init_beanie  # noqa: E402
from beanie.odm.utils import init as beanie_init  # noqa: E402
from bson import ObjectId  # noqa: E402
from bson.json_util import CANONICAL_JSON_OPTIONS  # noqa: E402
from bson.json_util import dumps as ext_dumps  # noqa: E402
from pymongo import AsyncMongoClient  # noqa: E402

from app.core.config import settings  # noqa: E402
from app.trove import tmod  # noqa: E402
from app.trove.mods_hub import rig_index, service, store, vfx  # noqa: E402
from app.trove.mods_hub.models import ModImageAsset, ModProject, ModRelease  # noqa: E402

assert tmod._native is None, "generate with the pure-Python reference"
UTC = dt.timezone.utc
T0 = dt.datetime(2026, 1, 2, 3, 4, 5, 678000, tzinfo=UTC)
T1 = dt.datetime(2026, 1, 14, 9, 30, 0, 125000, tzinfo=UTC)
BLOBS: dict[str, bytes] = {}


def b64(b: bytes) -> str:
    return base64.b64encode(b).decode()


def ext(value):
    return json.loads(ext_dumps(value, json_options=CANONICAL_JSON_OPTIONS))


async def get_blob(sha):
    return BLOBS.get(sha)


async def put_blob(data):
    sha = hashlib.sha256(data).hexdigest()
    created = sha not in BLOBS
    BLOBS[sha] = data
    return sha, created


store.get_blob = get_blob
store.put_blob = put_blob


def bp(name: str) -> bytes:
    return (TESTDATA / "blueprints" / "bp" / name).read_bytes()


def lcg(seed: int, n: int) -> bytes:
    x = seed & 0xFFFFFFFF
    out = bytearray()
    for _ in range(n):
        x = (x * 1103515245 + 12345) & 0x7FFFFFFF
        out.append((x >> 16) & 0xFF)
    return bytes(out)


PKFX = (
    'CParticleEffect $LOCAL$/Resource\n{\n\tOnSpawn = "$LOCAL$/Spawner";\n}\n'
    'CParticleRenderer_Billboard $LOCAL$/R1\n{\n\tDiffuse = "Textures/FX/Spark.dds";\n'
    '\tChild = "Particles/Sub/Child.pkfx";\n\tMesh = "Meshes/Rock.pkmm";\n}\n'
    'CParticleSamplerShape $LOCAL$/S1\n{\n\tAnimResource = "Meshes/Path.fbx";\n}\n'
    'CNEdEditorParticle_Backdrop $LOCAL$/B\n{\n\tMesh = "Editor/Room.fbx";\n}\n'
)
CHILD = 'CParticleEffect $LOCAL$/Resource\n{\n\tDiffuse = "Textures/FX/Glow.png";\n}\n'

ARTIFACTS = [
    {
        "name": "full",
        "props": {"title": "Parity Mod", "author": "Ann, Bob", "modVersion": "1.0", "tags": "GUI,Radar",
                  "flags": "262272", "previewPath": "UI/Parity-Mod.PNG", "configPath": "UI\\Parity Mod.cfg"},
        "files": [
            ("ui/parity-mod.png", lcg(1, 40)),
            ("ui/Parity Mod.cfg", b"[parity.swf]\nkey=1\n"),
            ("ui/extra.cfg", b"x=2\n"),
            ("ui/parity.swf", lcg(2, 64)),
            ("Blueprints/C_M_GoldenBeetle_Head.blueprint", bp("c_m_goldenbeetle_head.blueprint")),
            ("blueprints/blank.blueprint", bp("blank.blueprint")),
            ("blueprints/equipment_weapon_1h_sword_027[nim].blueprint", bp("equipment_weapon_1h_sword_027[nim].blueprint")),
            ("audio/Sounds/Synthetic.bnk", (TESTDATA / "media" / "synthetic.bnk").read_bytes()),
            ("audio/sounds/synthetic.txt", (TESTDATA / "media" / "synthetic.txt").read_bytes()),
            ("particles/vfx/Spark.pkfx", PKFX.encode()),
            ("particles/vfx/sub/child.pkfx", CHILD.encode()),
            ("textures/fx/spark.dds", lcg(3, 24)),
            ("A/b/Zeta.txt", b"zeta"),
            ("a/B/alpha.txt", b""),
        ],
    },
    {
        "name": "plain",
        "props": {"title": "  ", "flags": " 12_0 ", "notes": "caf\u00e9 notes"},
        "files": [("ui/one.cfg", b"a"), ("ui/Two.CFG", b"bb"), ("prefabs/x.binfab", b"\x01\x02")],
    },
    {
        "name": "titled_cfg",
        "props": {"title": "Store Log", "configPath": "/ui/storelog.cfg", "flags": "nope"},
        "files": [("ui/storelog.cfg", b"[StoreLog.swf]\n"), ("ui/StoreLog.swf", lcg(4, 10))],
    },
]


def make_tmod(a) -> bytes:
    return tmod.build_tmod(1, a["props"], a["files"], mod_loader="BTT", lowercase_paths=False)


def release_doc(rid, project_id, fmt, sha, size, props, extra=None):
    doc = {
        "_id": ObjectId(rid), "project_id": ObjectId(project_id), "owner_id": ObjectId("65b000000000000000000001"),
        "tag": "v1.2", "branch": "main", "title": "First", "changelog": "Fixed things",
        "title_i18n": {"fr": "Premier"}, "changelog_i18n": {},
        "source_commit_sha": None, "release_format": fmt, "tmod_sha": sha,
        "prior_tmod_shas": ["00" * 32], "tmod_size": size, "tmod_filename": "parity-v1.2." + fmt,
        "tmod_properties": props, "banner_sha": "ab" * 32, "download_count": 7, "status": "published",
        "created_at": T0, "updated_at": T0, "published_at": T1, "announced_at": None, "silent": False,
    }
    doc.update(extra or {})
    return doc


def project_doc(extra=None):
    doc = {
        "_id": ObjectId("65c000000000000000000001"), "slug": "parity-mod", "title": "Parity Mod",
        "summary": "s", "description": "long", "readme_text": "", "warnings": "",
        "title_i18n": {}, "summary_i18n": {"de": "S"}, "description_i18n": {}, "readme_i18n": {},
        "warnings_i18n": {}, "tags": ["GUI", "radar", "misc"], "owner_id": ObjectId("65b000000000000000000001"),
        "owner_username": "Parity User", "owner_handle": "parity_user", "collaborators": [],
        "is_stray": False, "stray_status": None, "author": "", "source": None, "source_id": None,
        "source_url": None, "source_author_id": None, "source_file_id": None, "source_likes": 0,
        "uploaded_on_behalf": False, "issues_enabled": True, "issue_seq": 0, "open_issue_count": 0,
        "visibility": "public", "is_beta": True, "mode": "files", "source_visibility": "public",
        "default_branch": "main", "banner_sha": None, "preview_shas": ["cd" * 32, ""],
        "discord_url": None, "website_url": "https://example.com", "donation_urls": [],
        "download_count": 12, "star_count": 3, "downloads_7d": 2, "popularity_score": 0.25,
        "hidden_release_branches": [], "branch_order": [], "forked_from_id": None,
        "forked_from_slug": "orig", "forked_from_handle": "someone", "forked_from_title": "Orig",
        "forked_from_owner": "Someone", "inspired_by_slug": None, "inspired_by_handle": None,
        "inspired_by_title": None, "inspired_by_owner": None, "fork_count": 1, "is_remix": False,
        "remix_status": "pending", "remix_requested_at": None, "remix_resolved_at": None,
        "remix_resolved_by": None, "taken_down": False, "takedown_reason": None,
        "created_at": T0, "updated_at": T1, "last_release_at": T1,
    }
    doc.update(extra or {})
    return doc


async def artifacts_section():
    out = []
    for i, a in enumerate(ARTIFACTS):
        data = make_tmod(a)
        sha, _ = await put_blob(data)
        props = {k: str(v) for k, v in tmod.read_tmod(data, metadata_only=True)["properties"].items()}
        raw = release_doc(f"65d00000000000000000000{i}", "65c000000000000000000001", "tmod", sha, len(data), props)
        r = ModRelease.model_validate(raw)

        async def no_rig(fns):
            return None, [], set()
        service._resolve_rig = no_rig
        files = {}
        for f in tmod.read_tmod(data)["files"]:
            try:
                got, name = await service.download_release_file(r, f["path"])
                files[f["path"]] = {"b64": b64(got), "name": name}
            except Exception as e:  # noqa: BLE001
                files[f["path"]] = {"error": str(e)}
        cfg_names = {}
        for f in tmod.read_tmod(data)["files"]:
            if f["path"].lower().endswith(".cfg"):
                got, name = await service.download_release_cfg(r, "  " + f["path"] + " ")
                cfg_names[f["path"]] = name
        banks = {}
        for want in ("synthetic.bnk", "AUDIO/Sounds/SYNTHETIC.bnk", "missing.bnk"):
            got = service._tmod_bank_bytes_sync(data, want)
            banks[want] = None if got is None else {"sha": hashlib.sha256(got[0]).hexdigest(), "text": got[1]}
        out.append({
            "name": a["name"], "tmod_b64": b64(data), "release": ext(raw),
            "dto": service._release_dto(r),
            "files": await service.list_release_files(r),
            "inspect": await service.inspect_release(r),
            "cfgs": await service.list_release_cfgs(r),
            "swfs": service._tmod_swfs_sync(data),
            "banks": service._tmod_banks_sync(data),
            "vfx": await service.list_release_vfx(r),
            "blueprints": await service.list_release_blueprints(r),
            "file_downloads": files,
            "cfg_names": cfg_names,
            "bank_files": banks,
        })
    return out


def zip_bytes(entries, comment=b"") -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, data in entries:
            if isinstance(name, zipfile.ZipInfo):
                z.writestr(name, data)
            else:
                z.writestr(name, data)
        z.comment = comment
    return buf.getvalue()


def zips_section():
    cases = []
    plain = zip_bytes([("a.txt", b"hello"), ("dir/", b""), ("dir/B.bin", lcg(5, 30)), ("caf\u00e9/\u00fc.txt", b"u")])
    cp = bytearray(zip_bytes([("x\x80y.txt", b"z")]))
    for i in range(len(cp) - 3):
        if cp[i:i + 4] == b"PK\x01\x02":
            cp[i + 9] &= ~0x08
    comment = zip_bytes([("n.txt", b"n")], comment=b"a comment")
    newer = bytearray(zip_bytes([("v.txt", b"v")]))
    newer[newer.index(b"PK") + 6] = 64
    samples = {
        "plain": plain, "cp437": bytes(cp), "comment": comment, "empty": zip_bytes([]),
        "tiny": b"PK\x05\x06" + b"\0" * 18, "junk": b"PK\x03\x04garbage", "short": b"PK",
        "truncated": plain[:-30], "prefixed": b"PREFIX" + plain, "newer": bytes(newer),
    }
    for name, data in samples.items():
        try:
            with zipfile.ZipFile(io.BytesIO(data)) as z:
                entries = [[i.filename, i.file_size] for i in z.infolist()]
        except (zipfile.BadZipFile, OSError, ValueError):
            entries = "bad"
        except NotImplementedError:
            entries = "unhandled"
        cases.append({"name": name, "b64": b64(data), "is_zipfile": zipfile.is_zipfile(io.BytesIO(data)),
                      "entries": entries})
    return cases


def names_section():
    names = ["My: Mod*?", "  ..dots..  ", "", "<>", "a\x00b\x1fc", "x" * 130, "caf\u00e9 \u4e2d", ". .a. .", "\u3000x\u3000"]
    safe = [[n, service._safe_filename(n)] for n in names]
    props_cases = [
        {"title": "Parity Mod"}, {"title": " Store Log "}, {"configPath": "UI\\Cfg\\Thing.CFG"},
        {"ConfigPath": "/ui/x.cfg", "title": "x"}, {}, {"title": "", "configPath": "ui/one.cfg"},
    ]
    paths = ["ui/Parity Mod.cfg", "ui/parity mod.cfg", "ui/thing.cfg", "ui/x.cfg", "deep/one.cfg", "store log.cfg"]
    cfg = [{"props": list(p.items()), "path": path, "out": service._cfg_download_name(
        ModRelease.model_construct(tmod_properties=p), path)} for p in props_cases for path in paths]
    declared = [{"props": list(p.items()), "out": service._declared_config_path(p)} for p in props_cases]
    previews = [{"props": [["previewPath", v]], "out": service._preview_path({"previewPath": v})}
                for v in ["", "  UI/A.PNG ", "ui/b.gif"]]
    flags = []
    for v in ["", "0", " 262144 ", "1_2", "-3", "+7", "abc", "1__2", "\u0663", "99999999999"]:
        parsed = tmod.read_tmod(tmod.build_tmod(1, {"flags": v}, [("ui/a", b"")]), metadata_only=True)
        flags.append({"value": v, "flags": parsed["flags"], "categories": parsed["categories"]})
    downloads = []
    for fmt, stored, props in [("tmod", "s.tmod", {"title": " T: x "}), ("tmod", "s.tmod", {"title": " "}),
                               ("zip", "z.zip", {"title": "T"}), ("tmod", "s.tmod", {})]:
        r = ModRelease.model_construct(release_format=fmt, tmod_filename=stored, tmod_properties=props)
        downloads.append({"format": fmt, "stored": stored, "props": list(props.items()),
                          "out": service.release_download_filename(r)})
    authors = [[a, service._clean_author(a, "Owner")] for a in [None, "", "  ", "Ann", " Ann ,, Bob ,", ",", "x" * 210]]
    return {"safe": safe, "cfg": cfg, "declared": declared, "previews": previews, "flags": flags,
            "downloads": downloads, "authors": authors, "thousands": [[n, f"{n:,}"] for n in [0, 999, 1000, 250001, 1234567]]}


def builds_section():
    out = []
    for title, tags, changelog, author in [
        ("Parity Mod", ["GUI", "Radar", "custom"], "  fixed  ", "Ann, Bob"),
        ("Plain", [], "", None), ("Tagged", ["nothing"], " ", " "),
    ]:
        p = ModProject.model_construct(title=title, tags=tags, owner_username="Owner", slug="My Slug!")
        props = service._release_properties(p, "v2", changelog, author=author)
        files = [("UI/Thing.swf", lcg(6, 9)), ("ui/My Slug!.png", b"old"), ("Blueprints/A.Blueprint", b"bp")]
        service.pack_preview(files, props, b"newpreview", "png", p.slug)
        err = None
        try:
            service._inject_config(files, props, b"[thing.swf]\n")
        except Exception as e:  # noqa: BLE001
            err = str(e)
        data = tmod.build_tmod(1, props, files)
        out.append({"title": title, "tags": tags, "changelog": changelog, "author": author,
                    "props": list(props.items()), "files": [[f, b64(c)] for f, c in files],
                    "config_error": err, "tmod_b64": b64(data)})
    configs = []
    for files, props, data in [
        ([], {"title": "T"}, b""), ([], {"title": "T"}, b"x" * (256 * 1024 + 1)), ([], {"title": "T"}, b"a\x00"),
        (["ui/a.cfg"], {"title": "T"}, b"x"), (["ui/a.SWF"], {"title": " "}, b"x"),
        (["ui/a.swf", "UI/T.cfg", "\\ui\\t.CFG"], {"title": " T/x "}, b"cfg"),
    ]:
        fl = [(f, b"") for f in files]
        pr = dict(props)
        try:
            service._inject_config(fl, pr, data)
            configs.append({"files": files, "props": list(props.items()), "data_len": len(data),
                            "out_files": [f for f, _ in fl], "out_props": list(pr.items())})
        except Exception as e:  # noqa: BLE001
            configs.append({"files": files, "props": list(props.items()), "data_len": len(data), "error": str(e)})
    repacks = []
    base = make_tmod(ARTIFACTS[2])
    for name, data, cfg in [("ok", base, b"[StoreLog.swf]\nk=v\n"), ("bad", b"junk-bytes-not-a-tmod", b"x"),
                            ("noswf", make_tmod(ARTIFACTS[1]), b"x")]:
        try:
            art, props = service._repack_with_config(data, cfg)
            repacks.append({"name": name, "in_b64": b64(data), "cfg": cfg.decode(), "out_b64": b64(art),
                            "props": list(props.items())})
        except Exception as e:  # noqa: BLE001
            repacks.append({"name": name, "in_b64": b64(data), "cfg": cfg.decode(), "error": str(e)})
    return {"releases": out, "configs": configs, "repacks": repacks}


def i18n_section():
    out = []
    for t in [{"fr": " Bonjour ", "de": "  ", " es ": "Hola"}, {"en": "x"}, {"xx": "y"},
              {"fr": "x" * 161}, {"zh-CN": "\u4e2d"}]:
        for base, max_len in (("title", 160), ("changelog", 20_000)):
            try:
                out.append({"in": list(t.items()), "base": base, "out": list(service._clean_i18n_map(
                    t, base=base, max_len=max_len).items())})
            except Exception as e:  # noqa: BLE001
                out.append({"in": list(t.items()), "base": base, "error": str(e)})
    return out


def event_section():
    out = []
    p_raw = project_doc()
    p = ModProject.model_validate(p_raw)
    r_raw = release_doc("65d0000000000000000000a1", "65c000000000000000000001", "tmod", "ef" * 32, 1000,
                        {"title": "Parity Mod", "modVersion": "v1.2", "author": ""})
    prev_raw = release_doc("65d0000000000000000000a0", "65c000000000000000000001", "tmod", "ef" * 32, 640,
                           {"title": "Parity Mod"}, {"tag": "v1.1", "published_at": T0, "banner_sha": None})
    r = ModRelease.model_validate(r_raw)
    prev = ModRelease.model_validate(prev_raw)
    for name, project, rel, previous, count in [("update", p, r, prev, 2), ("initial", p, r, None, 1)]:
        out.append({
            "name": name, "project": ext(p_raw), "release": ext(r_raw),
            "previous": ext(prev_raw) if previous is not None else None, "count": count,
            "event_project": service._event_project(project), "event_release": service._event_release(rel),
            "previous_release": service._event_release(previous) if previous is not None else None,
            "change": service._event_change(rel, previous, count),
            "public_mod": service.public_mod_dto(project),
        })
    return {"api_url": settings.api_url, "app_url": settings.app_url, "cases": out}


async def rig_section():
    rows = [
        ("prefabs/a.binfab", "c_m_beetle_head", "beetle", "head"),
        ("prefabs/a.binfab", "c_m_beetle_body", "beetle", "torso"),
        ("prefabs/b.binfab", "c_m_beetle_head", "other", "hat"),
        ("prefabs/c.binfab", "equipment_helm_crimefighter", "knight", "hat"),
        ("prefabs/c.binfab", "c_p_knight_torso", "knight", "torso"),
    ]
    rig_map = rig_index._build(rows)
    cases = []
    for fns in (["c_m_beetle_head", "c_m_beetle_body", "nope"],
                ["equipment_helm_crimefighter_lvl2[technoshyft]", "c_m_beetle_head"],
                ["c_p_knight_torso", "c_m_beetle_head"], ["unknown"], ["_c_m_beetle_body_lvl3_"]):
        async def fake(branch):
            return rig_map
        rig_index._rig_map = fake
        skeleton, attach = await rig_index.resolve(fns)
        cases.append({"fns": fns, "skeleton": skeleton, "attach": list(attach.items())})
    return {"rows": [list(r) for r in rows], "cases": cases}


async def vfx_section():
    a = ARTIFACTS[0]
    data = make_tmod(a)
    items, index = service._tmod_pkfx_and_index(data)
    game = {"rock.pkmm": "meshes/rock.pkmm", "path.pkan": "meshes/path.pkan"}

    async def resolver():
        def lookup(bn):
            return game.get(bn)

        async def read(path):
            return None
        return lookup, read, True
    service._game_vfx_resolver = resolver
    sha, _ = await put_blob(data)
    r = ModRelease.model_validate(release_doc("65d0000000000000000000b1", "65c000000000000000000001", "tmod",
                                              sha, len(data), {}))
    manifest = await service.get_release_vfx_manifest(r, "Particles/VFX/SPARK.pkfx")
    deps = sorted(service._VFX_DEPSET_CACHE[sha])
    return {"game": list(game.items()), "manifest": manifest, "depset": deps,
            "refs": vfx.extract_refs(PKFX), "media": [[p, vfx.media_type_for(p)] for p in
                                                     ["a/b.DDS", "x.png", "y.pkfx", "z.fbx", "noext", "d.ir/x"]]}


async def main() -> None:
    async def no_server(self):
        self._database_major_version = 7
        self._existing_collections = []

    beanie_init.Initializer._load_cached_info = no_server
    client = AsyncMongoClient("mongodb://127.0.0.1:1/", tz_aware=True, connect=False)
    await init_beanie(database=client["fixtures"], document_models=[ModRelease, ModProject, ModImageAsset],
                      skip_indexes=True)
    data = {
        "python": sys.version.split()[0],
        "artifacts": await artifacts_section(),
        "zips": zips_section(),
        "names": names_section(),
        "builds": builds_section(),
        "i18n": i18n_section(),
        "event": event_section(),
        "rig": await rig_section(),
        "vfx": await vfx_section(),
    }
    OUT.parent.mkdir(parents=True, exist_ok=True)
    OUT.write_text(json.dumps(data, indent=1, ensure_ascii=False) + "\n", encoding="utf-8")
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes)")


if __name__ == "__main__":
    asyncio.run(main())
