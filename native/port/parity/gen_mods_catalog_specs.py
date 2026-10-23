"""Writes the Mods Hub catalog parity specs (stdlib only):
    python native/port/parity/gen_mods_catalog_specs.py
mods_catalog.json: production GET/HEAD reads. mods_catalog.writes.json: the twin
sandbox run over mods_catalog.seed.js (site users signed in with fixed JWTs for
the sandbox SECRET_KEY).
"""
import base64
import json
import struct
import zlib
from pathlib import Path

HERE = Path(__file__).resolve().parent
JWT = {
    "owner": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI2NWYwMDAwMDAwMDAwMDAwMDAwMDAwYzEiLCJ2ZXIiOjAsImlhdCI6MTc5MjAwMDAwMCwiZXhwIjoxODkzNDU2MDAwLCJ0eXBlIjoiYWNjZXNzIiwia2luZCI6InNpdGUifQ.Xh8tiZWgVRgNhCeV2TIACaqRYLh8TvkHAkSDUOCcojg",
    "collab": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI2NWYwMDAwMDAwMDAwMDAwMDAwMDAwYzIiLCJ2ZXIiOjAsImlhdCI6MTc5MjAwMDAwMCwiZXhwIjoxODkzNDU2MDAwLCJ0eXBlIjoiYWNjZXNzIiwia2luZCI6InNpdGUifQ.f1X1ZjySH_MjupHZNUclS8Sf8BnMgQXh4QyMp3L1T4k",
    "stranger": "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiI2NWYwMDAwMDAwMDAwMDAwMDAwMDAwYzMiLCJ2ZXIiOjAsImlhdCI6MTc5MjAwMDAwMCwiZXhwIjoxODkzNDU2MDAwLCJ0eXBlIjoiYWNjZXNzIiwia2luZCI6InNpdGUifQ.iFTH7d16QoT2JyGJHCefnwbOUCTt6IYEMh_PWM8h9Jw",
}
TOKEN = {"Authorization": "Bearer ${KIWI_PARITY_TOKEN}"}
RL = ["x-ratelimit-limit"]
HUB = "/v1/mods/hub"
P1 = f"{HUB}/projects/paritymodder/dragon-pack"
TIMES = ["created_at", "updated_at"]


def cards(prefix, n=16):
    return [f"{prefix}[{i}].{f}" for i in range(n) for f in TIMES]


def auth(who):
    return {"Authorization": f"Bearer {JWT[who]}"}


def r(name, path, method="GET", who=None, body=None, headers=None, **extra):
    spec = {"name": name, "method": method, "path": path}
    h = dict(auth(who)) if who else {}
    h.update(headers or {})
    if h:
        spec["headers"] = h
    if body is not None:
        spec["body"] = body
    spec.update(extra)
    return spec


def png(w=2, h=1, color=b"\xff\x00\x00\xff"):
    raw = b"".join(b"\x00" + color * w for _ in range(h))

    def chunk(tag, data):
        return struct.pack(">I", len(data)) + tag + data + struct.pack(">I", zlib.crc32(tag + data) & 0xFFFFFFFF)
    return (b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", w, h, 8, 6, 0, 0, 0))
            + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))


def multipart(parts):
    out = b""
    for name, filename, ctype, data in parts:
        disp = f'form-data; name="{name}"' + (f'; filename="{filename}"' if filename else "")
        out += b"--XyZ\r\nContent-Disposition: " + disp.encode() + b"\r\n"
        if ctype:
            out += b"Content-Type: " + ctype.encode() + b"\r\n"
        out += b"\r\n" + data + b"\r\n"
    return out + b"--XyZ--\r\n"


def upload(name, path, who, parts, **extra):
    return r(name, path, "POST", who, headers={"Content-Type": "multipart/form-data; boundary=XyZ"},
             body_b64=base64.b64encode(multipart(parts)).decode(), **extra)


def reads():
    cap = {"HANDLE": "items[0].handle", "SLUG": "items[0].slug"}
    one = f"{HUB}/projects/${{HANDLE}}/${{SLUG}}"
    return [
        r("hub_list", f"{HUB}/projects", compare_headers=RL, capture=cap),
        r("hub_list_token", f"{HUB}/projects?limit=5", headers=TOKEN, compare_headers=RL),
        r("hub_list_head", f"{HUB}/projects?limit=2", "HEAD"),
        r("hub_list_downloads", f"{HUB}/projects?sort=downloads&limit=10"),
        r("hub_list_stars", f"{HUB}/projects?sort=stars&limit=10&offset=3"),
        r("hub_list_new", f"{HUB}/projects?sort=new&limit=7"),
        r("hub_list_title", f"{HUB}/projects?sort=title&limit=7"),
        r("hub_list_unknown_sort", f"{HUB}/projects?sort=bogus&limit=7"),
        r("hub_list_q", f"{HUB}/projects?q=dragon"),
        r("hub_list_q_regex", f"{HUB}/projects?q=a.b%2B(c"),
        r("hub_list_q_space", f"{HUB}/projects?q=%20skin%20"),
        r("hub_list_tag", f"{HUB}/projects?tag=GUI&limit=5"),
        r("hub_list_author", f"{HUB}/projects?author=${{HANDLE}}"),
        r("hub_list_far", f"{HUB}/projects?offset=100000"),
        r("hub_list_422", f"{HUB}/projects?limit=0&offset=-1&q=" + "x" * 121),
        r("hub_list_422_int", f"{HUB}/projects?limit=abc"),
        r("hub_tags", f"{HUB}/tags", compare_headers=RL),
        r("hub_profile", f"{HUB}/profile/${{HANDLE}}"),
        r("hub_profile_upper", f"{HUB}/profile/%20${{HANDLE}}%20"),
        r("hub_profile_unknown", f"{HUB}/profile/no-such-modder-zz"),
        r("hub_project", one, compare_headers=RL),
        r("hub_project_releases", f"{one}/releases"),
        r("hub_project_forks", f"{one}/forks"),
        r("hub_project_unknown", f"{HUB}/projects/${{HANDLE}}/no-such-slug-zz"),
        r("hub_project_stray_unknown", f"{HUB}/projects/STRAY/no-such-slug-zz"),
        r("hub_fallthrough", f"{HUB}/no-such-hub-route"),
        r("hub_fallthrough_me", f"{HUB}/me"),
        r("hub_me_noauth", f"{HUB}/me/projects"),
        r("hub_me_starred_noauth", f"{HUB}/me/starred"),
        r("hub_me_token_not_linked", f"{HUB}/me/projects", headers=TOKEN),
        r("hub_me_token_website_only", f"{HUB}/me/starred", headers=TOKEN),
        r("hub_me_bad_bearer", f"{HUB}/me/projects", headers={"Authorization": "Bearer nope"}),
        r("hub_me_portal_jwt", f"{HUB}/me/projects", headers={"Authorization": "Bearer ${KIWI_PARITY_JWT}"}),
        r("creator_links_defer", f"{HUB}/creator-links"),
        r("v1_list", "/v1/mods", compare_headers=RL),
        r("v1_list_q", "/v1/mods?q=Dragon&sort=title"),
        r("v1_list_tag", "/v1/mods?tag=dragons&limit=3"),
        r("v1_list_new", "/v1/mods?sort=new&limit=4&offset=1"),
        r("v1_list_422", "/v1/mods?limit=101"),
        r("v1_popular", "/v1/mods/popular?limit=5"),
        r("v1_popular_422", "/v1/mods/popular?limit=26"),
        r("v1_list_popular", "/v1/mods?sort=popular&limit=5"),
        r("v1_categories", "/v1/mods/categories", compare_headers=RL),
        r("v1_detail", "/v1/mods/${HANDLE}/${SLUG}", compare_headers=RL),
        r("v1_detail_unknown", "/v1/mods/${HANDLE}/no-such-slug-zz"),
        r("v1_detail_hub", "/v1/mods/hub/zz"),
        r("site_projects", "/site/mods/projects?limit=5"),
        r("site_projects_422", "/site/mods/projects?limit=500"),
        r("site_tags", "/site/mods/tags"),
        r("site_modders", "/site/mods/modders"),
        r("site_profile", "/site/mods/profile/${HANDLE}"),
        r("site_profile_unknown", "/site/mods/profile/no-such-modder-zz"),
        r("site_me_anon", "/site/mods/me/projects"),
        r("site_project", "/site/mods/projects/${HANDLE}/${SLUG}"),
        r("site_project_releases", "/site/mods/projects/${HANDLE}/${SLUG}/releases"),
        r("site_project_forks", "/site/mods/projects/${HANDLE}/${SLUG}/forks"),
        r("site_project_unknown", "/site/mods/projects/${HANDLE}/no-such-slug-zz"),
        r("site_project_bad_cookie", "/site/mods/projects/${HANDLE}/${SLUG}",
          headers={"Cookie": "kiwi_site_access=garbage", "Sec-Fetch-Site": "same-origin"}),
    ]


def writes():
    po = "/site/mods/projects/paritymodder"
    lookup = ["1" * 64, "2" * 64, "3" * 64, "4" * 64, "5" * 64, "zz"]
    detail_mask = TIMES + ["releases[0].created_at"]
    new_mask = ["created_at", "updated_at"]
    out = [
        # --- anonymous reads over the seed
        r("list", f"{HUB}/projects"),
        r("list_popular", f"{HUB}/projects?sort=popular"),
        r("list_downloads", f"{HUB}/projects?sort=downloads"),
        r("list_stars", f"{HUB}/projects?sort=stars"),
        r("list_new", f"{HUB}/projects?sort=new"),
        r("list_title", f"{HUB}/projects?sort=title"),
        r("list_bogus_sort", f"{HUB}/projects?sort=bogus"),
        r("list_q", f"{HUB}/projects?q=drag"),
        r("list_q_author", f"{HUB}/projects?q=old%20author"),
        r("list_tag", f"{HUB}/projects?tag=%20GUI%20"),
        r("list_author", f"{HUB}/projects?author=PARITY%20MODDER"),
        r("list_page", f"{HUB}/projects?sort=title&limit=1&offset=1"),
        r("tags", f"{HUB}/tags"),
        r("profile", f"{HUB}/profile/paritymodder"),
        r("profile_upper", f"{HUB}/profile/PARITYMODDER"),
        r("profile_remixer", f"{HUB}/profile/paritystranger"),
        r("profile_no_public", f"{HUB}/profile/paritycollab"),
        r("detail", P1),
        r("detail_draft", f"{HUB}/projects/paritymodder/secret-draft"),
        r("detail_taken_down", f"{HUB}/projects/paritymodder/gone"),
        r("detail_unlisted", f"{HUB}/projects/paritymodder/unlisted-one"),
        r("detail_web_only", f"{HUB}/projects/paritymodder/web-only"),
        r("detail_stray", f"{HUB}/projects/STRAY/old-stray"),
        r("detail_remix_old", f"{HUB}/projects/paritystranger/dragon-remix"),
        r("detail_remix_fresh", f"{HUB}/projects/paritystranger/fresh-remix"),
        r("releases", f"{P1}/releases"),
        r("forks", f"{P1}/forks"),
        r("v1_list", "/v1/mods"),
        r("v1_list_title", "/v1/mods?sort=title&q=a"),
        r("v1_popular", "/v1/mods/popular?limit=2"),
        r("v1_list_popular", "/v1/mods?sort=popular"),
        r("v1_categories", "/v1/mods/categories"),
        r("v1_detail", "/v1/mods/paritymodder/dragon-pack"),
        r("v1_detail_unlisted", "/v1/mods/paritymodder/unlisted-one"),
        r("v1_detail_web_only", "/v1/mods/paritymodder/web-only"),
        r("v1_detail_draft", "/v1/mods/paritymodder/secret-draft"),
        r("lookup", "/v1/mods/lookup", "POST", body={"hashes": [h.upper() for h in lookup] + [" " + lookup[0], ""]}),
        r("lookup_latest", "/v1/mods/lookup", "POST", body={"hashes": lookup, "include_releases": True}),
        r("lookup_all", "/v1/mods/lookup", "POST", body={"hashes": lookup, "releases": "all"}),
        r("lookup_none_wins", "/v1/mods/lookup", "POST", body={"hashes": lookup, "releases": "none", "include_releases": True}),
        r("lookup_empty", "/v1/mods/lookup", "POST", body={"hashes": []}),
        r("lookup_bad", "/v1/mods/lookup", "POST", body={"hashes": "x", "releases": "most"}),
        r("lookup_bad_json", "/v1/mods/lookup", "POST", body="{\"hashes\": [", headers={"Content-Type": "application/json"}),
        r("lookup_get_405", "/v1/mods/lookup"),
        r("tags_post_405", f"{HUB}/tags", "POST"),
        r("project_put_405", P1, "PUT"),
        r("fallthrough", f"{HUB}/xyz"),
        r("site_list", "/site/mods/projects?sort=new"),
        r("site_tags", "/site/mods/tags"),
        r("site_modders", "/site/mods/modders"),
        r("site_profile_anon", "/site/mods/profile/paritymodder"),
        r("site_profile_owner", "/site/mods/profile/paritymodder", who="owner"),
        r("site_profile_unknown", "/site/mods/profile/nobody-zz"),
        r("site_me_anon", "/site/mods/me/projects"),
        r("site_me_owner", "/site/mods/me/projects", who="owner"),
        r("site_draft_owner", f"{po}/secret-draft", who="owner"),
        r("site_draft_collab", f"{po}/secret-draft", who="collab"),
        r("site_taken_down_owner", f"{po}/gone", who="owner"),
        r("site_detail_collab", f"{po}/dragon-pack", who="collab"),
        r("site_releases_owner", f"{po}/dragon-pack/releases", who="owner"),
        r("site_releases_collab", f"{po}/dragon-pack/releases", who="collab"),
        r("site_forks", f"{po}/dragon-pack/forks"),
        # --- the creator's own reads
        r("me_projects", f"{HUB}/me/projects", who="owner"),
        r("me_projects_collab", f"{HUB}/me/projects", who="collab"),
        r("me_project", f"{HUB}/me/projects/paritymodder/dragon-pack", who="owner"),
        r("me_project_collab", f"{HUB}/me/projects/paritymodder/secret-draft", who="collab"),
        r("me_project_stranger", f"{HUB}/me/projects/paritymodder/dragon-pack", who="stranger"),
        r("me_releases", f"{HUB}/me/projects/paritymodder/dragon-pack/releases", who="owner"),
        r("me_starred", f"{HUB}/me/starred", who="collab"),
        r("me_stats", f"{HUB}/me/stats?days=7", who="owner", mask=["since"]),
        r("me_stats_422", f"{HUB}/me/stats?days=3", who="owner"),
        r("me_stats_noauth", f"{HUB}/me/stats?days=3"),
        r("me_token_website_only", f"{HUB}/me/starred", headers=TOKEN),
        r("me_token_not_linked", f"{HUB}/me/projects", headers=TOKEN),
        # --- create
        r("create_bad_json", f"{HUB}/projects", "POST", body="{\"title\": ", headers={"Content-Type": "application/json"}),
        r("create_noauth", f"{HUB}/projects", "POST", body={"title": "x"}),
        r("create_invalid", f"{HUB}/projects", "POST", "owner", body={"title": "", "tags": [1], "visibility": "secret"}),
        r("create_token_not_linked", f"{HUB}/projects", "POST", body={"title": "x"}, headers=TOKEN),
        r("create", f"{HUB}/projects", "POST", "owner", mask=new_mask,
          body={"title": "  My New Mod  ", "summary": " s ", "tags": ["Dragons", " dragons ", "Wings"],
                "visibility": "draft", "inspired_by": "/paritymodder/dragon-pack/"}),
        r("create_dupe_slug", f"{HUB}/projects", "POST", "owner", mask=new_mask, body={"title": "my new mod!"}),
        r("create_on_behalf_no_author", f"{HUB}/projects", "POST", "owner", body={"title": "Theirs", "on_behalf": True}),
        r("create_on_behalf", f"{HUB}/projects", "POST", "owner", mask=new_mask,
          body={"title": "Theirs", "on_behalf": True, "credited_author": "  Real Maker ", "mode": "files",
                "inspired_by": "paritymodder/dragon-pack"}),
        r("create_bad_credit", f"{HUB}/projects", "POST", "owner", body={"title": "x", "inspired_by": "nope"}),
        r("create_hidden_credit", f"{HUB}/projects", "POST", "stranger",
          body={"title": "x", "inspired_by": "paritymodder/secret-draft"}),
        # --- update
        r("update_stranger", P1, "PATCH", "stranger", body={"summary": "x"}),
        r("update_noauth", P1, "PATCH", body={"summary": "x"}),
        r("update_invalid", P1, "PATCH", "owner", body={"title": "", "tags": "x", "donation_urls": ["u"] * 6}),
        r("update_i18n_en", P1, "PATCH", "owner", body={"summary_i18n": {"en": "no"}}),
        r("update_i18n_unknown", P1, "PATCH", "owner", body={"readme_i18n": {"xx": "no"}}),
        r("update_i18n_long", P1, "PATCH", "owner", body={"title_i18n": {"de": "x" * 121}}),
        r("update_bad_url", P1, "PATCH", "owner", body={"discord_url": "discord.gg/x"}),
        r("update_bad_donation", P1, "PATCH", "owner", body={"donation_urls": ["https://ok.io", "nope"]}),
        r("update_bad_credit", P1, "PATCH", "owner", body={"inspired_by": "paritymodder/nope"}),
        r("update", P1, "PATCH", "owner", mask=detail_mask,
          body={"summary": "  Dragons, again  ", "title_i18n": {" de ": " Drachen ", "fr": "", "ja": "ドラゴン"},
                "tags": ["Dragons", "GUI", "GUI"], "website_url": " https://parity.example/x ",
                "donation_urls": ["https://a.io", "https://a.io", "", "https://b.io"],
                "branch_order": [" alt ", "main", "alt", ""], "issues_enabled": False, "title": None}),
        r("update_reveal_alt", P1, "PATCH", "collab", mask=detail_mask, body={"hidden_release_branches": []}),
        r("update_publish_draft", f"{HUB}/projects/paritymodder/secret-draft", "PATCH", "owner", mask=detail_mask,
          body={"visibility": "public", "inspired_by": ""}),
        r("update_on_behalf_mode", f"{HUB}/projects/paritymodder/theirs", "PATCH", "owner", body={"mode": "files"}),
        r("update_on_behalf_links", f"{HUB}/projects/paritymodder/theirs", "PATCH", "owner", mask=detail_mask,
          body={"discord_url": "https://discord.gg/x", "summary": "shared"}),
        r("update_remix_public", f"{HUB}/projects/paritystranger/fresh-remix", "PATCH", "stranger", mask=detail_mask,
          body={"visibility": "unlisted"}),
        # --- fork, stars, collaborators, claims, remixes
        r("fork_locked", f"{HUB}/projects/paritymodder/unlisted-one/fork", "POST", "stranger"),
        r("fork_draft", f"{HUB}/projects/paritymodder/secret-draft-zz/fork", "POST", "stranger"),
        r("fork", f"{P1}/fork", "POST", "stranger", mask=new_mask),
        r("fork_again", f"{P1}/fork", "POST", "stranger", mask=new_mask),
        r("star", f"{P1}/star", "POST", "stranger"),
        r("star_again", f"{P1}/star", "POST", "stranger"),
        r("unstar", f"{P1}/star", "DELETE", "stranger"),
        r("unstar_again", f"{P1}/star", "DELETE", "stranger"),
        r("star_hidden", f"{HUB}/projects/paritymodder/gone/star", "POST", "stranger"),
        r("star_noauth", f"{P1}/star", "POST"),
        r("starred_after", f"{HUB}/me/starred", who="stranger", mask=cards("items")),
        r("collab_add_by_collab", f"{P1}/collaborators", "POST", "collab", body={"username": "paritystranger"}),
        r("collab_add_owner", f"{P1}/collaborators", "POST", "owner", body={"username": "@ParityModder"}),
        r("collab_add_unknown", f"{P1}/collaborators", "POST", "owner", body={"username": "nobody-zz"}),
        r("collab_add_blank", f"{P1}/collaborators", "POST", "owner", body={"username": " @ "}),
        r("collab_add", f"{P1}/collaborators", "POST", "owner", body={"username": " @ParityStranger "}, mask=detail_mask),
        r("collab_add_again", f"{P1}/collaborators", "POST", "owner", body={"username": "paritystranger"}, mask=detail_mask),
        r("collab_remove", f"{P1}/collaborators/65f0000000000000000000c3", "DELETE", "owner", mask=detail_mask),
        r("collab_remove_bad_id", f"{P1}/collaborators/not-an-id", "DELETE", "owner", mask=detail_mask),
        r("claim_stray", f"{HUB}/projects/stray/old-stray/claim", "POST", "stranger", body={"message": "mine " * 3},
          mask=["id", "created_at"]),
        r("claim_stray_again", f"{HUB}/projects/stray/old-stray/claim", "POST", "stranger", body={},
          mask=["id", "created_at"]),
        r("claim_not_stray", f"{P1}/claim", "POST", "stranger", body={}),
        r("claim_invalid", f"{P1}/claim", "POST", "stranger", body={"message": "m" * 2001}),
        r("remixes", f"{P1}/remixes", who="owner", mask=cards("items")),
        r("remixes_stranger", f"{P1}/remixes", who="stranger"),
        r("remix_status_bad_id", f"{P1}/remixes/zz/status", "POST", "owner", body={"status": "approved"}),
        r("remix_status_invalid", f"{P1}/remixes/66f0000000000000000000d7/status", "POST", "owner", body={"status": "x"}),
        r("remix_status", f"{P1}/remixes/66f0000000000000000000d7/status", "POST", "owner", body={"status": "denied"},
          mask=TIMES),
        r("remix_claim_self", f"{P1}/remix-claim", "POST", "owner",
          body={"target_handle": "paritymodder", "target_slug": "dragon-pack"}),
        r("remix_claim_unknown", f"{P1}/remix-claim", "POST", "owner",
          body={"target_handle": "paritystranger", "target_slug": "zz"}),
        r("remix_claim", f"{P1}/remix-claim", "POST", "owner", mask=["id", "created_at"],
          body={"target_handle": "paritystranger", "target_slug": "dragon-remix", "message": "credit me"}),
        r("remix_claim_again", f"{P1}/remix-claim", "POST", "owner", mask=["id", "created_at"],
          body={"target_handle": "paritystranger", "target_slug": "dragon-remix"}),
        # --- images
        upload("avatar_missing", f"{HUB}/me/profile/avatar", "owner", [("other", None, None, b"x")]),
        upload("avatar_not_image", f"{HUB}/me/profile/avatar", "owner", [("file", "a.txt", "text/plain", b"hello " * 10)]),
        upload("avatar", f"{HUB}/me/profile/avatar", "owner", [("file", "a.png", "image/png", png())],
               mask=cards("mods") + ["featured.created_at", "featured.updated_at"]),
        upload("avatar_noauth", f"{HUB}/me/profile/avatar", None, [("file", "a.png", "image/png", png())]),
        upload("profile_banner", f"{HUB}/me/profile/banner", "owner", [("file", "b.png", "image/png", png(3, 2))],
               mask=cards("mods") + ["featured.created_at", "featured.updated_at"]),
        upload("profile_banner_no_public_mods", f"{HUB}/me/profile/banner", "collab",
               [("file", "b.png", "image/png", png(3, 2))]),
        r("avatar_bad_multipart", f"{HUB}/me/profile/avatar", "POST", "owner",
          headers={"Content-Type": "multipart/form-data; boundary=XyZ"}, body="garbage"),
        upload("banner", f"{P1}/banner", "owner", [("file", "b.png", "image/png", png(4, 4))]),
        upload("banner_stranger", f"{P1}/banner", "stranger", [("file", "b.png", "image/png", png(4, 4))]),
        r("banner_delete", f"{P1}/banner", "DELETE", "owner"),
        r("banner_delete_again", f"{P1}/banner", "DELETE", "owner"),
        upload("previews", f"{P1}/previews", "collab",
               [("files", "1.png", "image/png", png(1, 1)), ("files", "2.png", "image/png", png(2, 2)),
                ("files", "1b.png", "image/png", png(1, 1))]),
        upload("previews_missing", f"{P1}/previews", "owner", [("file", "1.png", "image/png", png(1, 1))]),
        upload("previews_text", f"{P1}/previews", "owner", [("files", None, None, b"text")]),
        r("preview_delete", f"{P1}/previews/" + "c" * 64, "DELETE", "owner"),
        r("preview_delete_missing", f"{P1}/previews/" + "0" * 64, "DELETE", "owner"),
        # --- profile
        r("profile_update_invalid", f"{HUB}/me/profile", "PATCH", "owner", body={"tagline": "t" * 161}),
        r("profile_update_i18n", f"{HUB}/me/profile", "PATCH", "owner", body={"tagline_i18n": {"en": "x"}}),
        r("profile_update_url", f"{HUB}/me/profile", "PATCH", "owner", body={"website_url": "ftp://x"}),
        r("profile_update", f"{HUB}/me/profile", "PATCH", "owner",
          body={"display_name": "  Modder Prime  ", "tagline": " New tagline ", "tagline_i18n": {"de": " Neu "},
                "mod_order": [" dragon-pack ", "dragon-pack", "", "my-new-mod"], "featured_slug": " unlisted-one ",
                "donation_urls": ["https://x.io"], "discord_url": ""},
          mask=cards("mods") + ["featured.created_at", "featured.updated_at"]),
        r("profile_update_unowned_feature", f"{HUB}/me/profile", "PATCH", "owner", body={"featured_slug": "dragon-remix"},
          mask=cards("mods")),
        r("profile_update_no_public_mods", f"{HUB}/me/profile", "PATCH", "collab", body={"tagline": "hi"}),
        r("profile_after", f"{HUB}/profile/paritymodder", mask=cards("mods") + ["featured.created_at", "featured.updated_at"]),
        # --- delete
        r("delete_stranger", f"{HUB}/projects/paritymodder/gone", "DELETE", "stranger"),
        r("delete_collab", f"{P1}", "DELETE", "collab"),
        r("delete_token", f"{P1}", "DELETE", headers=TOKEN),
        r("delete_taken_down", f"{HUB}/projects/paritymodder/gone", "DELETE", "owner"),
        r("delete_again", f"{HUB}/projects/paritymodder/gone", "DELETE", "owner"),
        r("lookup_after_delete", "/v1/mods/lookup", "POST", body={"hashes": ["3" * 64, "5" * 64, "1" * 64]},
          mask=["results." + "1" * 64 + ".mod.updated_at"]),
        r("list_after", f"{HUB}/projects?sort=title", mask=cards("items")),
        r("me_projects_after", f"{HUB}/me/projects", who="owner", mask=cards("items")),
        r("v1_detail_after", "/v1/mods/paritymodder/dragon-pack", mask=TIMES),
        r("site_releases_after", "/site/mods/projects/paritymodder/dragon-pack/releases", who="owner"),
    ]
    return out


def main():
    (HERE / "mods_catalog.json").write_text(json.dumps(reads(), indent=1) + "\n", encoding="utf-8")
    (HERE / "mods_catalog.writes.json").write_text(json.dumps(writes(), indent=1) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
