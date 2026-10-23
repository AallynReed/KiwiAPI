"""Request bodies and site tokens for the modpacks twin spec, from the repo root:
    S:/Desktop/Projects/.venvs/trove-prod/Scripts/python.exe native/port/parity/gen_modpacks_parity_data.py
Writes native/port/parity/modpacks.writes.json and data/modpacks/*.bin
(multipart uploads of small .tmod builds and a banner), and prints the hub
build's sha256 (the seeded release's tmod_sha) and the sandbox site JWTs.
"""
import base64
import hashlib
import hmac
import json
import os
import struct
import sys
import zlib
from pathlib import Path

sys.path.insert(0, os.getcwd())
os.environ.setdefault("SECRET_KEY", "x")

from app.trove import tmod  # noqa: E402

OUT = Path(__file__).resolve().parent / "data" / "modpacks"
BOUNDARY = "kiwiparityModpacks7Q"
SANDBOX_SECRET = b"parity-sandbox-secret-not-for-production"
USERS = {"OWNER": "65c100000000000000000001", "COLLAB": "65c100000000000000000002",
         "OTHER": "65c100000000000000000003"}

SWF = b"FWS\x0a" + bytes(range(32))
TMODS = {
    "custom": tmod.build_tmod(1, {"title": "Parity Custom", "author": "Tester"},
                              [("ui/parity.swf", SWF), ("ModCfgs/Parity Custom.cfg", b"[parity.swf]\nshow=1\n")],
                              lowercase_paths=False),
    "hub": tmod.build_tmod(1, {"title": "Parity Hub Mod", "author": "Modder", "previewPath": "preview.png"},
                           [("blocks/parity.blueprint", b"\x01\x02\x03"), ("preview.png", b"png")]),
    "conflict": tmod.build_tmod(1, {"title": " parity custom ", "author": "Copycat"},
                                [("ui/other.swf", SWF)], lowercase_paths=False),
    "shared": tmod.build_tmod(1, {"title": "Parity Shared Files"}, [("blocks/parity.blueprint", b"\x09")]),
}


def multipart(name: str, filename: str, data: bytes, ctype: str = "application/octet-stream") -> bytes:
    return (f"--{BOUNDARY}\r\nContent-Disposition: form-data; name=\"{name}\"; filename=\"{filename}\"\r\n"
            f"Content-Type: {ctype}\r\n\r\n").encode() + data + f"\r\n--{BOUNDARY}--\r\n".encode()


def png_1x1() -> bytes:
    def chunk(kind, body):
        return struct.pack(">I", len(body)) + kind + body + struct.pack(">I", zlib.crc32(kind + body))
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 3, 2, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(b"\x00" + b"\xff\x00\x00\xff" * 3 + b"\x00" + b"\x00\xff\x00\x80" * 3))
            + chunk(b"IEND", b""))


def b64url(b: bytes) -> str:
    return base64.urlsafe_b64encode(b).rstrip(b"=").decode()


def site_jwt(sub: str) -> str:
    head = b64url(json.dumps({"alg": "HS256", "typ": "JWT"}, separators=(",", ":")).encode())
    body = b64url(json.dumps({"sub": sub, "ver": 0, "iat": 1792000000, "exp": 1893456000, "type": "access",
                              "kind": "site"}, separators=(",", ":")).encode())
    sig = hmac.new(SANDBOX_SECRET, f"{head}.{body}".encode(), hashlib.sha256).digest()
    return f"{head}.{body}.{b64url(sig)}"


MP = "multipart/form-data; boundary=" + BOUNDARY
HUB = "/v1/modpacks/hub/projects"
STAMPS = ["created_at", "updated_at"]


def writes_spec() -> list[dict]:
    tok = {k: "Bearer " + site_jwt(v) for k, v in USERS.items()}
    admin = {"Authorization": "Bearer ${KIWI_PARITY_JWT}"}
    out: list[dict] = []

    def req(name, path, method="GET", who=None, body=None, file=None, **extra):
        r = {"name": name, "method": method, "path": path}
        headers = dict(extra.pop("headers", {}))
        if who == "ADMIN":
            headers.update(admin)
        elif who:
            headers["Authorization"] = tok[who]
        if file:
            r["body_file"] = f"data/modpacks/{file}"
            headers.setdefault("Content-Type", MP)
        elif body is not None:
            r["body"] = body
        if headers:
            r["headers"] = headers
        r.update(extra)
        out.append(r)

    dl = {"compare_headers": ["content-disposition", "content-length"]}
    zipdiff = {"allow_diff": ["body", "header:content-length"],
               "reason": "zip entries carry the wall-clock DOS time of each stack's request"}
    pub, mine = f"{HUB}/parityowner/seeded-public", f"{HUB}/parityowner/my-new-pack"

    # Seeded reads.
    for q in ("", "?sort=downloads", "?sort=stars", "?sort=new", "?sort=title", "?sort=bogus", "?tag=%20QOL%20",
              "?author=parity%20OWNER", "?q=PACK", "?q=(", "?limit=2&offset=1", "?limit=0", "?q=" + "x" * 121):
        req(f"hub_list{q}", HUB + q)
        req(f"catalog_list{q}", "/v1/modpacks" + q)
    req("site_list_title", "/site/modpacks/projects?sort=title")
    req("site_list_bad", "/site/modpacks/projects?offset=-1")
    for who in (None, "OWNER"):
        req(f"hub_get_public_{who}", pub, who=who)
    for path in ("ParityOwner/seeded-public", "parityowner/Seeded-Public", "parityowner/unlisted-pack",
                 "parityowner/draft-pack", "parityowner/taken-pack", "parityother/legacy", "nobody/seeded-public"):
        req(f"hub_get {path}", f"{HUB}/{path}")
        req(f"catalog_get {path}", f"/v1/modpacks/{path}")
    for who, path in ((None, "draft-pack"), ("OWNER", "draft-pack"), ("COLLAB", "seeded-public"),
                      ("OWNER", "taken-pack"), ("OTHER", "taken-pack"), ("OWNER", "seeded-public")):
        req(f"site_get {who} {path}", f"/site/modpacks/projects/parityowner/{path}", who=who)
    for path in ("paritymodder/parity-hub-mod", "stray/lost-mod", "%20STRAY%20/lost-mod", "nobody/x",
                 "paritymodder/secret-mod"):
        req(f"catalog_for_mod {path}", f"/v1/modpacks/for-mod/{path}")
        req(f"site_for_mod {path}", f"/site/modpacks/for-mod/{path}")

    # Downloads before any build is in the store.
    for q in ("", "?format=zip", "?format=configs", "?format=nope", "?variant=" + "v" * 81):
        req(f"download_empty{q}", f"/v1/modpacks/parityowner/seeded-public/download{q}", **dl)
    req("download_no_variants", "/v1/modpacks/parityother/legacy/download", **dl)
    req("download_draft_anon", "/site/modpacks/projects/parityowner/draft-pack/download", **dl)
    req("download_draft_owner", "/site/modpacks/projects/parityowner/draft-pack/download", who="OWNER", **dl)

    # Owner listings (the first resyncs the drifted handle).
    for who in (None, "OWNER", "COLLAB"):
        req(f"hub_mine_{who}", f"{HUB.rsplit('/', 1)[0]}/me/projects", who=who)
        req(f"site_mine_{who}", "/site/modpacks/me/projects", who=who)

    # Create and edit.
    req("create_anon", HUB, "POST", body={"title": "x"})
    req("create", HUB, "POST", "OWNER", body={"title": "  My New Pack  ", "summary": " s ",
                                              "tags": ["  Big   Tag ", "big tag", "UI"], "visibility": "public"},
        mask=STAMPS)
    req("create_again", HUB, "POST", "OWNER", body={"title": "My New Pack"}, mask=STAMPS)
    req("create_symbols", HUB, "POST", "OWNER", body={"title": "!!!", "visibility": "unlisted"}, mask=STAMPS)
    req("create_empty_title", HUB, "POST", "OWNER", body={"title": ""})
    req("create_bad_visibility", HUB, "POST", "OWNER", body={"title": "x", "visibility": "secret"})
    req("create_bad_json", HUB, "POST", "OWNER", body="{", headers={"Content-Type": "application/json"})
    req("update", mine, "PATCH", "OWNER", body={"discord_url": " https://discord.gg/x ", "website_url": "",
                                                 "donation_urls": ["https://a.test", "", "HTTP://b.test"],
                                                 "warnings": "w", "description": "d"}, mask=STAMPS)
    req("update_bad_url", mine, "PATCH", "OWNER", body={"website_url": "ftp://x"})
    req("update_bad_default", mine, "PATCH", "OWNER", body={"default_variant": "nope"})
    req("update_other", mine, "PATCH", "OTHER", body={"summary": "x"})
    req("update_anon", mine, "PATCH", body={"summary": "x"})

    # Variants.
    req("variant_add", f"{mine}/variants", "POST", "OWNER", body={"name": " Lite Edition "}, mask=STAMPS)
    req("variant_add_dup", f"{mine}/variants", "POST", "OWNER", body={"name": "lite edition"})
    req("variant_add_copy", f"{mine}/variants", "POST", "OWNER", body={"name": "Heavy", "copy_from": "default"},
        mask=STAMPS)
    req("variant_label", f"{mine}/variants/lite-edition", "PATCH", "OWNER", body={"label": "  Lite  "}, mask=STAMPS)
    req("variant_label_404", f"{mine}/variants/nope", "PATCH", "OWNER", body={"label": "x"})
    req("variant_delete", f"{mine}/variants/heavy", "DELETE", "OWNER", mask=STAMPS)
    req("variant_default_order", mine, "PATCH", "OWNER",
        body={"default_variant": "lite-edition", "variant_order": ["lite-edition", "default"]}, mask=STAMPS)
    req("variant_delete_default", f"{mine}/variants/lite-edition", "DELETE", "OWNER", mask=STAMPS)
    req("variant_delete_last", f"{mine}/variants/default", "DELETE", "OWNER")

    # Entries.
    req("entries", f"{mine}/variants/default/entries", "PUT", "OWNER", body={"entries": [
        {"handle": "paritymodder", "slug": "parity-hub-mod"},
        {"handle": "paritymodder", "slug": "parity-hub-mod", "branch": " "},
        {"handle": "paritymodder", "slug": "beta-mod", "branch": "dev", "version_locked": True, "locked_tag": "b1"},
        {"handle": "stray", "slug": "lost-mod", "locked_tag": "zzz"},
    ]}, mask=STAMPS)
    req("entries_draft_mod", f"{mine}/variants/default/entries", "PUT", "OWNER",
        body={"entries": [{"handle": "paritymodder", "slug": "secret-mod"}]})
    req("entries_missing_custom", f"{mine}/variants/default/entries", "PUT", "OWNER",
        body={"entries": [{"custom_sha": "f" * 64, "title": "Gone"}]})
    req("entries_unknown_variant", f"{mine}/variants/nope/entries", "PUT", "OWNER", body={"entries": []})
    req("entries_bad_body", f"{mine}/variants/default/entries", "PUT", "OWNER", body={"entries": [{"branch": 3}]})

    # Uploads.
    up = f"{mine}/variants/default/upload"
    req("upload_custom", up, "POST", "OWNER", file="upload_custom.bin", mask=STAMPS)
    req("upload_custom_again", up, "POST", "OWNER", file="upload_custom.bin", mask=STAMPS)
    req("upload_hub_present", up, "POST", "OWNER", file="upload_hub.bin", mask=STAMPS)
    req("upload_conflict", up, "POST", "OWNER", file="upload_conflict.bin")
    req("upload_shared", up, "POST", "OWNER", file="upload_shared.bin")
    req("upload_garbage", up, "POST", "OWNER", file="upload_garbage.bin")
    req("upload_empty", up, "POST", "OWNER", file="upload_empty.bin")
    req("upload_unknown_variant", f"{mine}/variants/nope/upload", "POST", "OWNER", file="upload_custom.bin")
    req("upload_anon", up, "POST", file="upload_custom.bin")
    req("upload_other", up, "POST", "OTHER", file="upload_custom.bin")
    req("upload_not_multipart", up, "POST", "OWNER", body={"file": "x"})
    req("upload_hub_new", f"{HUB}/parityowner/my-new-pack-2/variants/default/upload", "POST", "OWNER",
        file="upload_hub.bin", mask=STAMPS)
    req("entries_with_custom", f"{HUB}/parityowner/my-new-pack-2/variants/default/entries", "PUT", "OWNER",
        body={"entries": [{"custom_sha": "ecb4a45c103297d427557de5f12483c24158182b5f9301dda406d2dcde30f50d",
                           "custom_filename": "Parity Custom.tmod", "title": "", "author": "T"},
                          {"handle": "paritymodder", "slug": "parity-hub-mod"}]}, mask=STAMPS)
    req("entries_conflict", f"{HUB}/parityowner/my-new-pack-2/variants/default/entries", "PUT", "OWNER",
        body={"entries": [{"handle": "paritymodder", "slug": "parity-hub-mod"},
                          {"custom_sha": "b3f353888e329a59b365afd9e14e51db3c292d4b03a7d9b37b437e0e47aeb348",
                           "title": "Copy"}]})

    # Downloads with builds in the store.
    for base in (f"{HUB}/parityowner/my-new-pack", "/v1/modpacks/parityowner/seeded-public",
                 "/v1/modpacks/parityowner/my-new-pack-2"):
        req(f"download_tpack {base}", f"{base}/download", **dl)
        req(f"download_zip {base}", f"{base}/download?format=zip", **dl, **zipdiff)
        req(f"download_configs {base}", f"{base}/download?format=configs", **dl, **zipdiff)
    req("download_lite", "/v1/modpacks/parityowner/seeded-public/download?variant=lite", **dl)
    req("download_unknown_variant", "/v1/modpacks/parityowner/seeded-public/download?variant=zzz", **dl)
    req("download_head", "/v1/modpacks/parityowner/seeded-public/download", "HEAD", **dl)
    req("site_download", "/site/modpacks/projects/parityowner/seeded-public/download", **dl, **zipdiff)
    req("site_download_tpack", "/site/modpacks/projects/parityowner/my-new-pack/download?format=tpack", "GET",
        "OWNER", **dl)
    req("hub_get_after", pub, sleep=1)
    req("catalog_get_after", "/v1/modpacks/parityowner/seeded-public")

    # Banner.
    req("banner", f"{mine}/banner", "POST", "OWNER", file="banner.bin")
    req("banner_again", f"{mine}/banner", "POST", "OWNER", file="banner.bin")
    req("banner_text", f"{mine}/banner", "POST", "OWNER", file="banner_text.bin")
    req("banner_other", f"{mine}/banner", "POST", "OTHER", file="banner.bin")

    # Collaborators.
    co = f"{mine}/collaborators"
    req("collab_add", co, "POST", "OWNER", body={"username": " @ParityCollab "}, mask=STAMPS)
    req("collab_add_again", co, "POST", "OWNER", body={"username": "paritycollab"}, mask=STAMPS)
    req("collab_add_owner", co, "POST", "OWNER", body={"username": "parityowner"})
    req("collab_add_unknown", co, "POST", "OWNER", body={"username": "Nobody"})
    req("collab_add_blank", co, "POST", "OWNER", body={"username": "@"})
    req("collab_by_collab", co, "POST", "COLLAB", body={"username": "parityother"})
    req("collab_edits", mine, "PATCH", "COLLAB", body={"summary": "collab edit", "tags": []}, mask=STAMPS)
    req("collab_delete_pack", mine, "DELETE", "COLLAB")
    req("collab_remove_bad_id", f"{co}/notanid", "DELETE", "OWNER", mask=STAMPS)
    req("collab_remove", f"{co}/65c100000000000000000002", "DELETE", "OWNER", mask=STAMPS)

    # Stars.
    for name, method, who, path in (("star", "POST", "OTHER", pub), ("star_again", "POST", "OTHER", pub),
                                    ("unstar", "DELETE", "OTHER", pub), ("unstar_again", "DELETE", "OTHER", pub),
                                    ("star_draft", "POST", "OTHER", f"{HUB}/parityowner/draft-pack"),
                                    ("star_owner_draft", "POST", "OWNER", f"{HUB}/parityowner/draft-pack"),
                                    ("star_anon", "POST", None, pub)):
        req(name, f"{path}/star", method, who)

    # Master moderation.
    req("admin_list_anon", "/admin/modpacks")
    req("admin_list_seeded", "/admin/modpacks?visibility=draft", "GET", "ADMIN")
    req("admin_list_bad", "/admin/modpacks?limit=201", "GET", "ADMIN")
    req("admin_list_new", "/admin/modpacks?q=my%20new&owner=Parity%20Owner&limit=1", "GET", "ADMIN",
        capture={"PACK_ID": "items[0].id"}, mask=["items[0].id", "items[0].updated_at", "items[0].created_at"])
    req("admin_takedown_blank", "/admin/modpacks/${PACK_ID}/takedown", "POST", "ADMIN", body={"reason": "  "})
    req("hub_get_taken", mine)
    req("site_get_taken_owner", "/site/modpacks/projects/parityowner/my-new-pack", "GET", "OWNER", mask=STAMPS)
    req("admin_restore", "/admin/modpacks/${PACK_ID}/restore", "POST", "ADMIN")
    req("admin_restore_bad", "/admin/modpacks/zzz/restore", "POST", "ADMIN")
    req("admin_takedown_collab_pack", "/admin/modpacks/65c400000000000000000006/takedown", "POST", "ADMIN",
        body={"reason": " Spam pack "})
    req("admin_takedown_seeded", "/admin/modpacks/65c400000000000000000001/takedown", "POST", "ADMIN",
        body={"reason": "Policy"})
    req("admin_takedown_missing", "/admin/modpacks/65c4000000000000000000ff/takedown", "POST", "ADMIN",
        body={"reason": "x"})
    req("admin_takedown_bad_body", "/admin/modpacks/65c400000000000000000001/takedown", "POST", "ADMIN",
        body={"reason": "r" * 2001})
    req("admin_delete", "/admin/modpacks/65c400000000000000000003", "DELETE", "ADMIN")
    req("admin_delete_again", "/admin/modpacks/65c400000000000000000003", "DELETE", "ADMIN")
    req("delete_pack", f"{HUB}/parityowner/my-new-pack-2", "DELETE", "OWNER")
    req("delete_pack_again", f"{HUB}/parityowner/my-new-pack-2", "DELETE", "OWNER")
    req("delete_seeded_public", pub, "DELETE", "OWNER")

    # 405s.
    for name, method, path in (("hub_list_put", "PUT", HUB), ("catalog_post", "POST", "/v1/modpacks"),
                               ("hub_pack_put", "PUT", pub), ("star_put", "PUT", f"{pub}/star"),
                               ("variant_get", "GET", f"{pub}/variants/default"),
                               ("variants_get", "GET", f"{pub}/variants"),
                               ("site_list_post", "POST", "/site/modpacks/projects"),
                               ("admin_list_post", "POST", "/admin/modpacks"),
                               ("admin_pack_get", "GET", "/admin/modpacks/65c400000000000000000001"),
                               ("download_post", "POST", "/v1/modpacks/parityowner/x/download")):
        req(f"405 {name}", path, method, "ADMIN" if path.startswith("/admin") else "OWNER")
    return out


def main():
    (Path(__file__).resolve().parent / "modpacks.writes.json").write_text(
        "[\n" + ",\n".join(" " + json.dumps(r, ensure_ascii=False) for r in writes_spec()) + "\n]\n",
        encoding="utf-8")
    OUT.mkdir(parents=True, exist_ok=True)
    files = {
        "upload_custom.bin": multipart("file", "Parity Custom.tmod", TMODS["custom"]),
        "upload_hub.bin": multipart("file", "whatever.tmod", TMODS["hub"]),
        "upload_conflict.bin": multipart("file", "c.tmod", TMODS["conflict"]),
        "upload_shared.bin": multipart("file", "s.tmod", TMODS["shared"]),
        "upload_garbage.bin": multipart("file", "g.tmod", b"not a tmod at all"),
        "upload_empty.bin": multipart("file", "e.tmod", b""),
        "banner.bin": multipart("file", "b.png", png_1x1(), "image/png"),
        "banner_text.bin": multipart("file", "b.txt", b"hello there, not an image at all", "text/plain"),
    }
    for name, data in files.items():
        (OUT / name).write_bytes(data)
    print("boundary", BOUNDARY)
    for k, v in TMODS.items():
        print(f"sha256 {k}", hashlib.sha256(v).hexdigest())
    for k, v in USERS.items():
        print(f"jwt {k}", site_jwt(v))


if __name__ == "__main__":
    main()
