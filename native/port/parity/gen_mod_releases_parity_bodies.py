"""Request bodies for mod_releases.writes.json, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_mod_releases_parity_bodies.py
Writes native/port/parity/data/mod_releases/*.bin (multipart, boundary
kiwiParityBoundary).
"""
import io
import os
import sys
import zipfile
from pathlib import Path

sys.path.insert(0, os.getcwd())

from app.trove import tmod  # noqa: E402

HERE = Path(__file__).resolve().parent
TESTDATA = HERE.parents[1] / "kiwi-server" / "testdata"
OUT = HERE / "data" / "mod_releases"
B = "kiwiParityBoundary"

PKFX = (
    'CParticleEffect $LOCAL$/Resource\n{\n\tOnSpawn = "$LOCAL$/Spawner";\n}\n'
    'CParticleRenderer_Billboard $LOCAL$/R1\n{\n\tDiffuse = "Textures/FX/Spark.dds";\n'
    '\tChild = "Particles/VFX/Sub/Child.pkfx";\n\tMesh = "Meshes/Rock.pkmm";\n}\n'
)
CHILD = 'CParticleEffect $LOCAL$/Resource\n{\n\tDiffuse = "Textures/FX/Glow.png";\n}\n'


def multipart(parts):
    out = bytearray()
    for name, filename, ctype, data in parts:
        out += f"--{B}\r\n".encode()
        disp = f'form-data; name="{name}"' + (f'; filename="{filename}"' if filename is not None else "")
        out += f"Content-Disposition: {disp}\r\n".encode()
        if ctype:
            out += f"Content-Type: {ctype}\r\n".encode()
        out += b"\r\n" + data + b"\r\n"
    out += f"--{B}--\r\n".encode()
    return bytes(out)


def field(name, value):
    return (name, None, None, value.encode())


def main():
    OUT.mkdir(parents=True, exist_ok=True)
    bp = TESTDATA / "blueprints" / "bp"
    files = [
        ("ui/parity.swf", (TESTDATA / "media" / "controllernotification.swf").read_bytes()),
        ("ui/Parity Mod.cfg", b"[parity.swf]\nkey=1\n"),
        ("Blueprints/C_M_GoldenBeetle_Head.blueprint", (bp / "c_m_goldenbeetle_head.blueprint").read_bytes()),
        ("blueprints/blank.blueprint", (bp / "blank.blueprint").read_bytes()),
        ("audio/sounds/Synthetic.bnk", (TESTDATA / "media" / "synthetic.bnk").read_bytes()),
        ("audio/sounds/synthetic.txt", (TESTDATA / "media" / "synthetic.txt").read_bytes()),
        ("particles/vfx/Spark.pkfx", PKFX.encode()),
        ("particles/vfx/sub/child.pkfx", CHILD.encode()),
        ("textures/fx/spark.dds", b"DDS " + bytes(range(60))),
        ("ui/parity_preview.png", b"\x89PNG\r\n\x1a\nnot-really"),
    ]
    props = {"title": "Parity Mod", "author": "Parity Creator", "modVersion": "1.0", "tags": "GUI,Radar",
             "previewPath": "UI/Parity_Preview.PNG", "configPath": "ui/Parity Mod.cfg"}
    mod = tmod.build_tmod(1, props, files, mod_loader="BTT", lowercase_paths=False)
    plain = tmod.build_tmod(1, {"title": "No Flash"}, [("blueprints/x.blueprint", b"bp")], mod_loader="BTT")
    zbuf = io.BytesIO()
    with zipfile.ZipFile(zbuf, "w", zipfile.ZIP_DEFLATED) as z:
        z.writestr(zipfile.ZipInfo("ui/readme.txt", (2026, 1, 2, 3, 4, 6)), b"zip release")
        z.writestr(zipfile.ZipInfo("dir/", (2026, 1, 2, 3, 4, 6)), b"")
    zipped = zbuf.getvalue()
    cfg = b"[parity.swf]\nkey=2\nextra=yes\n"
    oct_ = "application/octet-stream"
    bodies = {
        "upload_v1.bin": [field("tag", " v1.0 "), field("title", " First build "), field("changelog", "Initial\n"),
                          ("file", "Parity Mod.tmod", oct_, mod)],
        "upload_v1_again.bin": [field("tag", "v1.0"), ("file", "Parity Mod.tmod", oct_, mod)],
        "upload_v11_config.bin": [field("tag", "v1.1"), field("branch", "beta"), ("file", "parity.tmod", oct_, mod),
                                  ("config", "settings.cfg", "text/plain", cfg)],
        "upload_draft_silent.bin": [field("tag", "v0.9"), field("status", "draft"), field("silent", "true"),
                                    ("file", "draft.tmod", oct_, plain)],
        "upload_zip.bin": [field("tag", "z1"), field("branch", "alt"), ("file", "bundle.zip", "application/zip", zipped)],
        "upload_zip_config.bin": [field("tag", "z2"), ("file", "bundle.zip", "application/zip", zipped),
                                  ("config", "c.cfg", "text/plain", cfg)],
        "upload_bad_tmod.bin": [field("tag", "bad"), ("file", "bad.tmod", oct_, b"\x01\x02junk-not-a-tmod")],
        "upload_bad_zip.bin": [field("tag", "badzip"), ("file", "bad.zip", oct_, b"PK\x03\x04garbage")],
        "upload_bad_status.bin": [field("tag", "v9"), field("status", "live"), ("file", "p.tmod", oct_, plain)],
        "upload_noswf_config.bin": [field("tag", "v2"), ("file", "p.tmod", oct_, plain),
                                    ("config", "c.cfg", "text/plain", cfg)],
        "upload_missing_file.bin": [field("tag", "v3")],
        "upload_text_file.bin": [field("tag", "v4"), field("file", "not a file")],
        "upload_missing_tag.bin": [("file", "p.tmod", oct_, plain)],
        "config_attach.bin": [("file", "settings.cfg", "text/plain", cfg)],
        "config_attach_other.bin": [("file", "settings.cfg", "text/plain", b"[parity.swf]\nkey=3\n")],
        "config_attach_binary.bin": [("file", "x.cfg", oct_, b"a\x00b")],
        "config_attach_missing.bin": [field("other", "x")],
    }
    for name, parts in bodies.items():
        (OUT / name).write_bytes(multipart(parts))
    print(f"wrote {len(bodies)} bodies to {OUT}")


if __name__ == "__main__":
    main()
