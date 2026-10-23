"""Request bodies for ocr.writes.json and tools.writes.json, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_tools_parity_bodies.py
Writes native/port/parity/data/{ocr,tools}/*.bin (multipart bodies use the
boundary kiwiParityBoundary).
"""
import os
import sys
from pathlib import Path

sys.path.insert(0, os.getcwd())

from app.trove import tmod  # noqa: E402

HERE = Path(__file__).resolve().parent
IMAGES = HERE.parents[1] / "kiwi-server" / "testdata" / "ocr"
B = "kiwiParityBoundary"


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


def main():
    ocr = HERE / "data" / "ocr"
    tools = HERE / "data" / "tools"
    ocr.mkdir(parents=True, exist_ok=True)
    tools.mkdir(parents=True, exist_ok=True)
    for name, ctype in [("tiny.png", "image/png"), ("wide_strip.png", "image/png"), ("fr_sheet.png", "image/png"),
                        ("en_sheet.jpg", "image/jpeg")]:
        data = (IMAGES / name).read_bytes()
        (ocr / (name.replace(".", "_") + ".bin")).write_bytes(multipart([("file", name, ctype, data)]))
    tiny = (IMAGES / "tiny.png").read_bytes()
    extra = {
        "no_ctype.bin": multipart([("file", "tiny.png", None, tiny)]),
        "text_plain.bin": multipart([("file", "notes.txt", "text/plain", b"hello")]),
        "empty_file.bin": multipart([("file", "empty.png", "image/png", b"")]),
        "garbage.bin": multipart([("file", "x.png", "image/png", b"not an image at all")]),
        "missing.bin": multipart([("other", "tiny.png", "image/png", tiny)]),
        "text_field.bin": multipart([("file", None, None, b"just text")]),
        "params_ctype.bin": multipart([("file", "tiny.png", "image/PNG; charset=x", tiny)]),
    }
    for name, data in extra.items():
        (ocr / name).write_bytes(data)
    mod = tmod.build_tmod(1, {"title": "Parity Mod", "author": "Kiwi", "tags": "Dragons,GUI"},
                          [("ui/hud.swf", b"FWS\x08swfdata"), ("blueprints/x.blueprint", bytes(range(200))),
                           ("Mods/Parity Mod.cfg", b"[cfg]\n")])
    (tools / "mod.tmod").write_bytes(mod)
    (tools / "broken.tmod").write_bytes(mod[:40])
    exe = b"MZ\x90\x00" + b"\x00" * 60 + b"\x7c\x39\x68\xe0\x02\x00\x00" + b"rest-of-binary"
    (tools / "unlock_ok.bin").write_bytes(multipart([("trove_exe", "Trove.exe", "application/octet-stream", exe)]))
    (tools / "unlock_not_mz.bin").write_bytes(multipart([("trove_exe", "Trove.exe", None, b"ZM" + exe[2:])]))
    (tools / "unlock_no_seq.bin").write_bytes(multipart([("trove_exe", "Trove.exe", None, b"MZ" + b"\x00" * 30)]))
    (tools / "unlock_missing.bin").write_bytes(multipart([("other", "Trove.exe", None, exe)]))
    print("wrote", sorted(p.name for p in ocr.iterdir()), sorted(p.name for p in tools.iterdir()))


if __name__ == "__main__":
    main()
