"""Tiny local dev server for previewing the BetterTroveTools showcase site
WITHOUT spinning up the full FastAPI stack (which needs MongoDB + Redis +
env config).

Serves:
- /                        → site/templates/index.html
- /commands                → site/templates/commands.html
- /leaderboards            → site/templates/leaderboards.html
- /static/*                → site/static/*
- /site/leaderboards/*     → stub JSON responses with realistic shapes

Run via the "site" config in .claude/launch.json; access at
http://localhost:8913/leaderboards.
"""
from __future__ import annotations

import base64
import json
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse, parse_qs, unquote

# An 8x8 slate PNG so stubbed image routes (banners/previews) render something
# instead of leaving the <img> request hanging.
_PLACEHOLDER_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mOwcYrCihiGlgQAEoM2AX8snL0AAAAASUVORK5CYII="
)

ROOT = Path(__file__).resolve().parent.parent
SITE_DIR = ROOT / "site"
TEMPLATES = SITE_DIR / "templates"
STATIC = SITE_DIR / "static"

# Stub data shaped to match what app/trove/leaderboards/service.py returns,
# so the page renders end-to-end without a backend.
#
# Anchors are computed at module-import time relative to "now" so the
# 7-day picker on the page shows recent trove-days. We seed FIVE of the
# last 7 days so two slots render as "No data" - exercising the empty
# state. Each anchor is real UTC 11:00 of its trove-day (matches the
# current API's daily-anchor model).
import time as _time
_DAY = 86400
_TROVE_OFFSET = 11 * 3600
_now = int(_time.time())
_today_trove_key = (_now - _TROVE_OFFSET) // _DAY
# Days 0 (today), 1 (yesterday), 2, 4, 6 ago - skip 3 and 5 so the
# picker visibly shows two "No data" gaps.
_seeded_days = {0, 1, 2, 4, 6}
_anchors = sorted(
    [(_today_trove_key - d) * _DAY + _TROVE_OFFSET for d in _seeded_days],
    reverse=True,
)
STUB_ANCHOR = _anchors[0]
STUB_TIMESTAMPS = _anchors

STUB_BOARDS = [
    {"uuid": 1012, "name_id": "Leaderboard_GlyphKicker", "name": "GLYPH KICKER",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "daily", "reset_kind": "daily", "player_board": True},
    {"uuid": 20, "name_id": "Leaderboard_GeodeMetaExperience", "name": "GEODE MASTERY POINTS",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "default", "player_board": True},
    {"uuid": 33001, "name_id": "Leaderboard_HartsReceived", "name": "HART-A-PHONES RECEIVED",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 2004, "name_id": "Leaderboard_DelveDepth_Challenge", "name": "CHALLENGE: Deepest (WEEKLY)",
     "category_id": "Leaderboard_Category_Contests", "category": "CONTESTS",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 5001, "name_id": "Leaderboard_ClubExp", "name": "CLUB MASTERY",
     "category_id": "Leaderboard_Category_Clubs", "category": "CLUBS",
     "contest_type": None, "reset_kind": "default", "player_board": False},
]
STUB_ENTRIES = [
    {"player_name": "Skill", "rank": 1, "score": 59731.0},
    {"player_name": "noa00__00", "rank": 1, "score": 59731.0},
    {"player_name": "MaxOG", "rank": 3, "score": 59629.0},
    {"player_name": "Bae", "rank": 4, "score": 59560.0},
    {"player_name": "VatsanT", "rank": 5, "score": 59511.0},
    {"player_name": "Aallyn", "rank": 6, "score": 58032.0},
    {"player_name": "TestPlayer", "rank": 7, "score": 57000.0},
]

# The 18 Trove classes in classes.json order (= Effort/Paragon board offset
# order), for the Class Activity page stubs.
_STUB_CLASSES = [
    "Bard", "Boomeranger", "Candy Barbarian", "Chloromancer", "Dino Tamer",
    "Dracolyte", "Fae Trickster", "Gunslinger", "Ice Sage", "Knight",
    "Lunar Lancer", "Neon Ninja", "Pirate Captain", "Revenant", "Shadow Hunter",
    "Solarion", "Tomb Raiser", "Vanguardian",
]
# qualified_name per class (classes.json order) → self-hosted icon path.
_STUB_CLASS_QN = [
    "bard", "adventurer", "candybarbarian", "chloromancer", "dinotamer",
    "dracolyte", "faetrickster", "gunslinger", "icemage", "knight",
    "lunarlancer", "neonninja", "piratelord", "spirittank", "shadowhunter",
    "solarion", "tombraiser", "crimefighter",
]
def _stub_icon(ci):
    return f"/static/class-icons/{_STUB_CLASS_QN[ci]}.png"

# Mods Hub stub cards for the /mods + /mods/{slug} local preview.
_STUB_MODS = [
    {"slug": "neon-hud", "title": "Neon HUD Overhaul",
     "summary": "A clean, high-contrast HUD retexture with neon accents.",
     "tags": ["GUI", "Reskin", "hud"], "owner_username": "Aallyn",
     "visibility": "public", "banner_sha": None, "download_count": 1280,
     "updated_at": None, "created_at": None},
    {"slug": "tiny-mounts", "title": "Tiny Mounts",
     "summary": "Shrinks every mount to adorable proportions.",
     "tags": ["mounts", "fun"], "owner_username": "Skill",
     "visibility": "public", "banner_sha": None, "download_count": 642,
     "updated_at": None, "created_at": None},
    {"slug": "quiet-ui", "title": "Quiet UI",
     "summary": "Removes screen clutter for cleaner screenshots.",
     "tags": ["ui", "minimal"], "owner_username": "Bae",
     "visibility": "public", "banner_sha": None, "download_count": 318,
     "updated_at": None, "created_at": None,
     "forked_from": {"slug": "neon-hud", "handle": "aallyn", "title": "Neon HUD Overhaul", "owner": "Aallyn"},
     "inspired_by": None, "fork_count": 0},
]
for _m in _STUB_MODS:
    _m.setdefault("forked_from", None)
    _m.setdefault("inspired_by", None)
    _m.setdefault("fork_count", 0)
    _m.setdefault("mode", "files")
    _m.setdefault("star_count", 0)
    _m.setdefault("preview_sha", None)
    _m.setdefault("handle", _m["owner_username"].lower())   # /mods/<handle>/<slug>
_STUB_MODS[0]["preview_sha"] = "prevsha1"   # neon-hud: no banner -> card uses first preview
_STUB_MODS[1]["mode"] = "releases"   # tiny-mounts is a releases-only mod
_STUB_MODS[0]["star_count"] = 87
_STUB_MODS[1]["star_count"] = 34
_STUB_MODS[2]["star_count"] = 12
_STUB_MODS[0]["fork_count"] = 1   # neon-hud has one fork (quiet-ui)

# --- Modpack stubs (/site/modpacks/*) --------------------------------------
_STUB_PACKS = [
    {"slug": "starter-pack", "title": "Aallyn's Starter Pack", "handle": "aallyn",
     "summary": "A clean HUD + quality-of-life bundle to get started.",
     "tags": ["hud", "qol"], "owner_username": "Aallyn", "visibility": "public",
     "banner_sha": None, "preview_sha": None, "download_count": 412, "star_count": 57,
     "variant_count": 2, "mod_count": 2, "total_entries": 3,
     "updated_at": None, "created_at": None},
    {"slug": "pvp-pack", "title": "PvP Essentials", "handle": "skill",
     "summary": "Minimal UI tuned for the arena.", "tags": ["pvp", "minimal"],
     "owner_username": "Skill", "visibility": "public", "banner_sha": None,
     "preview_sha": None, "download_count": 88, "star_count": 9, "variant_count": 1,
     "mod_count": 1, "total_entries": 1, "updated_at": None, "created_at": None},
]


def _stub_entry(handle, slug, title, branch="main", version=None, locked=False,
                available=True, reason=None, author=None):
    return {"handle": handle, "slug": slug, "title": title, "custom": False,
            "author": author or handle.title(), "branch": branch,
            "version": version, "version_locked": locked,
            "locked_tag": version if locked else None,
            "available": available, "reason": reason}


def _stub_custom_entry(title, author):
    return {"custom": True, "custom_sha": "deadbeef", "custom_filename": f"{title}.tmod",
            "handle": "", "slug": "", "title": title, "author": author, "branch": "",
            "version": None, "version_locked": False, "locked_tag": None,
            "available": True, "reason": None}


def _stub_pack_detail(handle, slug):
    base = next((p for p in _STUB_PACKS if p["slug"] == slug), _STUB_PACKS[0])
    variants = [
        {"name": "default", "label": "Default", "mod_count": 4, "available_count": 3,
         "entries": [
             _stub_entry("aallyn", "neon-hud", "Neon HUD Overhaul", "main", "v1.2.0", author="Aallyn"),
             _stub_entry("skill", "tiny-mounts", "Tiny Mounts", "main", "v0.9", locked=True, author="Skill"),
             _stub_entry("bae", "quiet-ui", "Quiet UI", "main", available=False, reason="no build", author="Bae"),
             _stub_custom_entry("My Custom HUD", "LocalArtist"),
         ]},
        {"name": "lite", "label": "Lite", "mod_count": 1, "available_count": 1,
         "entries": [
             _stub_entry("aallyn", "neon-hud", "Neon HUD Overhaul", "main", "v1.2.0", author="Aallyn"),
         ]},
    ]
    return {
        **base, "handle": handle, "starred": False,
        "description": "A **sample** modpack for local preview.\n\n"
        "Pick mods, group them into variants and lock versions. Downloads as a "
        "`.zip` (web) or `.tpack` (API).",
        "warnings": "Back up your mods folder first.<br>Some mods need the latest game build.",
        "preview_shas": [], "discord_url": "https://discord.gg/example",
        "website_url": "https://example.com", "donation_urls": [],
        "default_variant": "default", "variants": variants,
        "taken_down": False, "takedown_reason": None,
        "is_owner": True,   # dev: show the owner editor controls
        "is_primary_owner": True,
        "collaborators": [{"id": "collab1", "username": "skill"}],
    }


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        path = url.path

        # Page routes.
        if path == "/":
            return self._send_file(TEMPLATES / "index.html", "text/html")
        if path == "/commands":
            return self._send_file(TEMPLATES / "commands.html", "text/html")
        if path == "/leaderboards":
            return self._send_file(TEMPLATES / "leaderboards.html", "text/html")
        if path == "/updates":
            return self._send_file(TEMPLATES / "updates.html", "text/html")
        if path == "/status":
            return self._send_file(TEMPLATES / "status.html", "text/html")
        if path == "/server-time":
            return self._send_file(TEMPLATES / "server-time.html", "text/html")
        if path == "/giveaways":
            return self._send_file(TEMPLATES / "giveaways.html", "text/html")
        if path == "/clubs":
            return self._send_file(TEMPLATES / "clubs.html", "text/html")
        if path == "/activity":
            return self._send_file(TEMPLATES / "activity.html", "text/html")
        if path == "/market":
            return self._send_file(TEMPLATES / "market.html", "text/html")
        if path == "/support":
            return self._send_file(TEMPLATES / "support.html", "text/html")
        if path == "/documentation":
            return self._send_file(TEMPLATES / "docs.html", "text/html")
        if path == "/swf-docs":
            return self._send_file(TEMPLATES / "swf-docs.html", "text/html")
        if path == "/terms":
            return self._send_file(TEMPLATES / "terms.html", "text/html")
        if path == "/privacy":
            return self._send_file(TEMPLATES / "privacy.html", "text/html")
        if path == "/class-activity":
            return self._send_file(TEMPLATES / "class-activity.html", "text/html")
        if path == "/dashboard":
            return self._send_file(TEMPLATES / "dashboard.html", "text/html")
        if path == "/mods":
            return self._send_file(TEMPLATES / "mods.html", "text/html")
        if path == "/mods/why":
            return self._send_file(TEMPLATES / "mods_why.html", "text/html")
        if path.startswith("/mods/"):
            # /mods/<handle> = modder profile; /mods/<handle>/<slug> = a mod.
            segs = [s for s in path[len("/mods/"):].split("/") if s]
            page = "mods_profile.html" if len(segs) == 1 else "mods_project.html"
            return self._send_file(TEMPLATES / page, "text/html")
        if path == "/modpacks":
            return self._send_file(TEMPLATES / "modpacks.html", "text/html")
        if path.startswith("/modpacks/"):
            # /modpacks/<handle>/<slug> = a single modpack.
            return self._send_file(TEMPLATES / "modpacks_project.html", "text/html")

        # Static. Templates reference the minified bundles (built by deploy.sh),
        # which don't exist locally for brand-new pages. Fall back to the
        # unminified source so local preview works without running the minifier.
        if path.startswith("/static/"):
            rel = path[len("/static/"):]
            target = STATIC / rel
            if not target.exists() and ".min." in rel:
                source = STATIC / rel.replace(".min.", ".", 1)
                if source.exists():
                    target = source
            return self._send_file(target, None)

        # Stub JSON endpoints.
        # --- Modpacks (/site/modpacks/*) -----------------------------------
        if path.startswith("/site/modpacks/for-mod/"):
            # Which modpacks include this mod (the mod-page backlink). Pretend the
            # first stub pack always includes whatever mod is asked about.
            return self._send_json({"items": _STUB_PACKS[:1], "count": 1})
        if path == "/site/modpacks/me/projects":
            return self._send_json({"items": _STUB_PACKS})
        if path == "/site/modpacks/projects":
            return self._send_json({"items": _STUB_PACKS, "count": len(_STUB_PACKS),
                                    "total": len(_STUB_PACKS)})
        if path.startswith("/site/modpacks/projects/"):
            rest = path[len("/site/modpacks/projects/"):]
            parts = [s for s in rest.split("/") if s]
            handle = parts[0] if parts else "aallyn"
            slug = parts[1] if len(parts) > 1 else "starter-pack"
            return self._send_json(_stub_pack_detail(handle, slug))

        # --- Mods Hub (/site/mods/*) ---------------------------------------
        if path == "/site/mods/me/projects":
            return self._send_json({"items": []})
        if path.startswith("/site/mods/profile/"):
            handle = path[len("/site/mods/profile/"):]
            mods = list(_STUB_MODS)   # several mods so reorder/highlight is testable
            return self._send_json({
                "handle": handle or "aallyn",
                "display_name": "Aallyn",
                "tagline": "Trove modder · HUD & retexture artist",
                "readme": "## Hey!\n\nI make **clean HUD** mods. Check out my work below.\n\n"
                "[![ko-fi](https://img.shields.io/badge/support-ko--fi-ff5e5b)](https://ko-fi.com/example)",
                "avatar_url": "/site/mods/image/avatarsha",
                "avatar_sha": "avatarsha",
                "banner_url": "/site/mods/image/bannersha",
                "banner_sha": "bannersha",
                "discord_url": "https://discord.gg/example",
                "website_url": "https://example.com",
                "donation_urls": ["https://ko-fi.com/example"],
                "is_owner": False,
                "joined_at": "2025-01-15T00:00:00+00:00",
                "page_url": f"https://trove.aallyn.net/mods/{handle}",
                "mod_count": len(mods),
                "mod_order": [m["slug"] for m in mods],
                "featured_slug": mods[0]["slug"],
                "featured": mods[0],
                "mods": mods,
            })
        if path == "/site/mods/tags":
            return self._send_json({
                "categories": [{"tag": "GUI", "count": 7}, {"tag": "Dragons", "count": 3},
                               {"tag": "Reskin", "count": 5}, {"tag": "Mounts", "count": 2}],
                "custom": [{"tag": "retexture", "count": 4}, {"tag": "hud", "count": 2},
                           {"tag": "fun", "count": 1}, {"tag": "minimal", "count": 1}],
            })
        if path == "/site/mods/projects":
            return self._send_json({"items": _STUB_MODS, "count": len(_STUB_MODS),
                                    "total": len(_STUB_MODS)})
        if path.startswith("/site/mods/image/"):
            return self._send_bytes(_PLACEHOLDER_PNG, "image/png")
        # Stub the blueprint list so the page's collapsible "3D models" renders in dev
        # (the real decode endpoint needs an actual .tmod and isn't stubbed).
        if path.startswith("/site/mods/releases/") and path.endswith("/blueprints"):
            return self._send_json({
                "items": [
                    {"path": "blueprints/c_head.blueprint", "size": 980, "assembled": True},
                    {"path": "blueprints/c_l_eye.blueprint", "size": 60, "assembled": True},
                    {"path": "blueprints/c_r_eye.blueprint", "size": 60, "assembled": True},
                    {"path": "blueprints/c_l_hand.blueprint", "size": 220, "assembled": True},
                    {"path": "blueprints/c_sword.blueprint", "size": 310, "assembled": False},
                ],
                "rig": "companion_spidermonkey",   # so the "assembled creature" button shows
                "animations": ["unarmed_idle", "unarmed_walk_forward", "unarmed_run_forward",
                               "unarmed_dance", "unarmed_idle_1"],
            })
        # Serve the pre-baked assembled spider so the model viewer can be previewed.
        if path.startswith("/site/mods/releases/") and path.endswith("/assembled"):
            mp = STATIC / "models" / "companion_spidermonkey.model.json"
            if mp.exists():
                return self._send_file(mp, "application/json")
            return self._send_json({"error": {"message": "no model"}})
        # Lazily-loaded rig animation frames: /site/rigs/<skeleton>/anim/<name>
        if path.startswith("/site/rigs/") and "/anim/" in path:
            import re as _re
            m = _re.match(r"^/site/rigs/([a-z0-9_]+)/anim/([a-z0-9_]+)$", path)
            if m:
                ap = (ROOT / "app" / "trove" / "mods_hub" / "rigs" / "anim"
                      / m.group(1) / (m.group(2) + ".json"))
                if ap.exists():
                    return self._send_file(ap, "application/json")
            return self._send_json({"error": {"message": "no animation"}})
        if path.startswith("/site/mods/projects/"):
            rest = path[len("/site/mods/projects/"):]
            _parts = rest.split("/")
            handle = _parts[0] if _parts else ""
            slug = _parts[1] if len(_parts) > 1 else ""
            sub = "/".join(_parts[2:])
            base = next((m for m in _STUB_MODS if m["slug"] == slug), _STUB_MODS[0])
            if (sub == "" or sub is None) and handle == "stray":
                # Imported, unclaimed "stray" mod (uploaded via contributions).
                return self._send_json({
                    "slug": slug, "handle": "stray", "title": "Stray HUD Pack",
                    "summary": "A community-contributed mod, not yet claimed.",
                    "description": "Uploaded via contributions. The original author retains credit.",
                    "readme_text": "", "warnings": "", "tags": ["interface", "hud"],
                    "owner_username": "SomeModder", "author": "SomeModder",
                    "visibility": "public", "mode": "releases", "source_visibility": "public",
                    "banner_sha": None, "preview_sha": None, "preview_shas": [],
                    "download_count": 102216, "star_count": 4,
                    "is_stray": True,
                    "taken_down": False, "takedown_reason": None, "is_owner": False,
                    "starred": False, "discord_url": None, "website_url": None,
                    "donation_urls": [], "default_branch": "main", "source_visible": False,
                    "commit_count": 0, "clone_url": None, "branches": [],
                    "hidden_release_branches": [], "branch_order": [],
                    "forked_from": None, "inspired_by": None, "fork_count": 0,
                    "releases": [
                        {"id": "rel-stray", "tag": "12", "branch": "main", "title": "",
                         "changelog": "", "status": "published", "tmod_filename": "Imported HUD Pack.tmod",
                         "tmod_size": 51200, "download_count": 102216, "format": "tmod",
                         "banner_sha": None, "published_at": None, "created_at": None},
                    ],
                })
            if sub == "" or sub is None:
                sv = base.get("mode", "files") == "files"   # public-source files mod
                return self._send_json({
                    **base, "description": "A **sample** mod for local preview.\n\n"
                    "Files, branches and releases are stubbed by the dev server.",
                    "readme_text": "## Sample README\n\n"
                    "[![badge](https://img.shields.io/badge/build-passing-brightgreen)](https://example.com)\n\n"
                    "<div align=\"center\">Centered HTML works.</div>\n\n"
                    "| A | B |\n|---|---|\n| 1 | 2 |\n\n"
                    "- bullet one\n- bullet two\n\n"
                    "Saved on the project (releases-only mode). Normal **bold** + `code`.",
                    "warnings": "Requires the latest game build.<br>Back up your mods folder first.",
                    "default_branch": "main", "preview_shas": ["prevsha1", "prevsha2"], "taken_down": False,
                    "takedown_reason": None, "is_owner": False, "starred": False,
                    "discord_url": "https://discord.gg/example",
                    "website_url": "https://example.com",
                    "donation_urls": ["https://ko-fi.com/example", "https://paypal.me/example"],
                    "handle": handle,
                    "mode": base.get("mode", "files"), "source_visibility": "public",
                    "source_visible": sv,
                    "commit_count": 2 if sv else 0,
                    "clone_url": f"https://api.aallyn.net/git/mods/{handle}/{slug}.git" if sv else None,
                    "branches": [{"name": "main", "head_commit_id": "stub", "updated_at": None}] if sv else [],
                    "hidden_release_branches": [], "branch_order": ["experimental", "main"],
                    "releases": [
                        {"id": "rel-main", "tag": "v1.2.0", "branch": "main",
                         "title": "Stable", "changelog": "Latest stable build.",
                         "status": "published", "tmod_filename": f"{slug}-v1.2.0.tmod",
                         "tmod_size": 20480, "download_count": 142, "format": "tmod",
                         "banner_sha": None, "published_at": None, "created_at": None},
                        {"id": "rel-exp", "tag": "v1.3.0-beta", "branch": "experimental",
                         "title": "Beta variant", "changelog": "Experimental features.",
                         "status": "published", "tmod_filename": f"{slug}-v1.3.0.zip",
                         "tmod_size": 30720, "download_count": 12, "format": "zip",
                         "banner_sha": None, "published_at": None, "created_at": None},
                        {"id": "rel-exp-tmod", "tag": "v1.3.0", "branch": "experimental",
                         "title": "Experimental build", "changelog": "Bleeding-edge .tmod.",
                         "status": "published", "tmod_filename": f"{slug}-v1.3.0.tmod",
                         "tmod_size": 22000, "download_count": 30, "format": "tmod",
                         "banner_sha": None, "published_at": None, "created_at": None},
                    ],
                })
            if sub == "branches":
                return self._send_json({"items": [
                    {"name": "main", "head_commit_id": "stub", "updated_at": None}]})
            if sub == "releases":
                return self._send_json({"items": []})
            if sub == "forks":
                forks = [m for m in _STUB_MODS
                         if (m.get("forked_from") or {}).get("slug") == slug]
                return self._send_json({"items": forks})
            if sub == "tree":
                return self._send_json({"commit": {"id": "stub", "seq": 2}, "entries": [
                    {"path": "readme.md", "blob_sha": "stub", "size": 180},
                    {"path": "config/default.cfg", "blob_sha": "stub", "size": 128},
                    {"path": "ui/icon.png", "blob_sha": "stub", "size": 4096}]})
            if sub.startswith("raw/") and sub.lower().endswith("readme.md"):
                return self._send_text(
                    "# Sample Mod\n\nThis README is **rendered** from the repo's "
                    "`readme.md`, GitHub-style.\n\n## Features\n\n- One cool thing\n"
                    "- Another thing\n\n```\ninstall: drop the .tmod in your mods folder\n"
                    "```\n\nSee the [Mods Hub](https://trove.aallyn.net/mods).\n")
            if sub == "placement":
                return self._send_json({
                    "commit": {"id": "stub", "seq": 2}, "total": 5, "compilable_count": 2,
                    "skipped": [
                        {"path": "readme.txt",
                         "reason": "root file (only files inside a Trove folder are compiled)"},
                        {"path": "bin/tool.exe", "reason": "'bin' is not a Trove folder"},
                    ],
                    "misplaced": [
                        {"path": "blueprints/foo.blueprint",
                         "expected": "blueprints/equipment/foo.blueprint"},
                    ],
                    "fix_available": True, "game_index_available": True,
                })
            if sub == "commits":
                return self._send_json({"items": [
                    {"id": "c2", "seq": 2, "branch": "main", "author_username": "tester",
                     "message": "Add icon", "file_count": 2, "created_at": None},
                    {"id": "c1", "seq": 1, "branch": "main", "author_username": "tester",
                     "message": "Initial commit", "file_count": 1, "created_at": None}],
                    "count": 2, "total": 2})
            return self._send_json({"items": []})

        if path == "/site/leaderboards/config":
            # Return a non-3 value so the subtitle change is visible in
            # the local preview (prod would normally serve 3 from the
            # runtime_config default).
            return self._send_json({"hot_retention_days": 5})
        if path == "/site/server-time":
            # Authoritative Trove time for the /server-time clock. Trove's day
            # rolls at 11:00 UTC (UTC-11), so daily reset = next 11:00 UTC.
            import time as _t
            from datetime import datetime, timezone, timedelta
            now = datetime.now(timezone.utc)
            day_reset = now.replace(hour=11, minute=0, second=0, microsecond=0)
            if now >= day_reset:
                day_reset += timedelta(days=1)
            # Weekly reset: just pick the next Sunday 11:00 UTC for the stub.
            wk = day_reset
            while wk.weekday() != 6:
                wk += timedelta(days=1)
            trove_now = now - timedelta(hours=11)
            return self._send_json({
                "now_unix": int(_t.time()),
                "now_iso": now.isoformat(),
                "trove_day": trove_now.strftime("%A"),
                "daily_reset_at": int(day_reset.timestamp()),
                "weekly_reset_at": int(wk.timestamp()),
            })
        if path == "/site/trove-status":
            # Multi-env stub (eu/us/pts, binary online/down): EU down, US + PTS
            # online - a partial outage, matching the real-world state observed.
            import time as _t
            return self._send_json({
                "overall": "down",
                "auth": {"online": True, "http_status": 405, "latency_ms": 120.0, "error": None},
                "environments": {
                    "eu": {"status": "down", "online": False,
                           "game": {"online": False, "host": "ams-c12-b05.ams.triongames.com", "port": 6560, "latency_ms": 3000.0, "error": "glsserver_dropped"}},
                    "us": {"status": "online", "online": True,
                           "game": {"online": True, "host": "dal-c35-b05.dal.triongames.com", "port": 6560, "latency_ms": 88.0, "error": None}},
                    "pts": {"status": "online", "online": True,
                            "game": {"online": True, "host": "auth-pcpts01.trovegame.com", "port": 6560, "latency_ms": 95.0, "error": None}},
                },
                "checked_at": int(_t.time()),
            })
        if path == "/site/trove-status/history":
            # Synthetic 7-day timeline with a couple of outages so the
            # graphic + outage log render in preview.
            import time as _t
            now = int(_t.time()); day = 86400
            qs = parse_qs(url.query); env = (qs.get("env", ["live"])[0])
            start = now - 7 * day
            segs = [
                {"status": "online", "online": True, "started_at": start, "ended_at": now - 3*day, "duration_seconds": 4*day},
                {"status": "down", "online": False, "started_at": now - 3*day, "ended_at": now - 3*day + 5400, "duration_seconds": 5400},
                {"status": "online", "online": True, "started_at": now - 3*day + 5400, "ended_at": now - 18*3600, "duration_seconds": 3*day - 5400 - 18*3600},
            ]
            if env == "live":
                segs.append({"status": "down", "online": False, "started_at": now - 18*3600, "ended_at": None, "duration_seconds": 18*3600})
            else:
                segs.append({"status": "online", "online": True, "started_at": now - 18*3600, "ended_at": None, "duration_seconds": 18*3600})
            covered = sum(s["duration_seconds"] for s in segs)
            up = sum(s["duration_seconds"] for s in segs if s["status"] == "online")
            outages = [{"status": s["status"], "started_at": s["started_at"], "ended_at": s["ended_at"], "duration_seconds": s["duration_seconds"]} for s in segs if s["status"] != "online"]
            return self._send_json({
                "env": env, "days": 7, "window_start": start, "window_end": now,
                "uptime": round(up/covered, 5), "covered_seconds": covered,
                "segments": segs, "outages": outages,
            })
        if path == "/site/leaderboards/activity":
            # 1.0h window with a realistic count so the hero pill renders.
            return self._send_json({
                "window_start": STUB_ANCHOR - 3600,
                "window_end": STUB_ANCHOR,
                "duration_hours": 1.0,
                "estimate": 4231,
                "estimate_24h": 18764,
                "estimate_7d": 52310,
                "window_24h_start": STUB_ANCHOR - 86400,
                "window_7d_start": STUB_ANCHOR - 7 * 86400,
                "span_24h_hours": 24.0,
                "span_7d_hours": 168.0,
                "by_board": [
                    {"uuid": 10, "name": "FLUX EARNED",      "category": "STATS", "active_players": 2847},
                    {"uuid": 15, "name": "LOOT COLLECTED",   "category": "STATS", "active_players": 2143},
                    {"uuid": 3,  "name": "ENEMIES DEFEATED", "category": "STATS", "active_players": 1908},
                ],
                "boards_analyzed": 11,
                "methodology": "Distinct top-5000 leaderboard players whose score increased on at least one lifetime-accumulating board between the two most recent captures.",
                "computed_at": 1780890000,
            })
        if path == "/site/leaderboards/activity/series":
            # Synthetic bucketed series so the /activity charts render. The
            # period sets bucket size + count; a diurnal sine + slow drift
            # gives a believable shape with a clear peak / quiet trough.
            import math
            qs = parse_qs(url.query)
            period = (qs.get("period", ["7d"])[0]).lower()
            # /activity only exposes up to 1 month (longer ranges removed).
            spec = {
                "1d":  (3600, 24), "7d": (3 * 3600, 56), "1m": (86400, 30),
            }.get(period, (3 * 3600, 56))
            bucket, count = spec
            end = STUB_ANCHOR
            start = end - bucket * count
            points = []
            for i in range(count):
                ti = start + i * bucket
                base = 3500 + 1500 * math.sin(i / 3.0) + (i * 12)
                active = max(180, round(base))
                points.append({"t": ti, "active": float(active),
                               "peak": float(active + 420), "samples": 1})
            peak = max(points, key=lambda p: p["active"])
            avg = round(sum(p["active"] for p in points) / len(points), 1)
            return self._send_json({
                "period": period, "bucket_seconds": bucket,
                "window_start": start, "window_end": end,
                "points": points,
                "peak": {"t": peak["t"], "active": peak["active"]},
                "average": avg, "latest": points[-1]["active"],
                "methodology": "stub series",
            })
        if path == "/site/leaderboards/class-activity/series":
            # Synthetic per-class multi-line series: shared buckets + one values[]
            # per class (a diurnal sine, phase-shifted per class so the lines
            # spread out), with occasional nulls to mimic the weekly-reset gap.
            import math
            qs = parse_qs(url.query)
            period = (qs.get("period", ["7d"])[0]).lower()
            spec = {
                "1d":  (3600, 24), "7d": (3 * 3600, 56), "1m": (86400, 30),
                "3m":  (86400, 90), "6m": (2 * 86400, 90), "1y": (7 * 86400, 52),
                "all": (7 * 86400, 80),
            }.get(period, (3 * 3600, 56))
            bucket, count = spec
            end = STUB_ANCHOR
            start = end - bucket * count
            buckets = [start + i * bucket for i in range(count)]
            classes = []
            for ci, name in enumerate(_STUB_CLASSES):
                vals, vals_clean = [], []
                for i in range(count):
                    base = 120 + 70 * math.sin(i / 3.0 + ci * 0.5) + (18 - ci) * 6
                    # a couple of synthetic gaps (weekly reset look)
                    raw = None if (i % 28 == 13) else max(2.0, round(base, 1))
                    vals.append(raw)
                    # "clean" view ≈ 55-65% of raw, with an extra synthetic gap
                    # so it exercises the per-line null handling independently.
                    if raw is None or (i % 23 == 7):
                        vals_clean.append(None)
                    else:
                        vals_clean.append(max(1.0, round(raw * (0.55 + 0.1 * ((ci % 3) / 2)), 1)))
                classes.append({"class_index": ci, "name": name, "icon": _stub_icon(ci),
                                "values": vals, "values_clean": vals_clean})
            return self._send_json({
                "period": period, "bucket_seconds": bucket,
                "window_start": start, "window_end": end,
                "power_rank_threshold": 25000,
                "buckets": buckets, "classes": classes,
                "methodology": "stub class series",
            })
        if path == "/site/leaderboards/class-activity/current":
            # Synthetic per-class counts + sum-normalized share for the donut.
            counts = [max(3, 320 - ci * 15 - (ci % 3) * 8) for ci in range(len(_STUB_CLASSES))]
            clean = [max(1, int(counts[ci] * (0.55 + 0.1 * ((ci % 3) / 2)))) for ci in range(len(_STUB_CLASSES))]
            total = sum(counts)
            total_clean = sum(clean)
            classes = [
                {"class_index": ci, "name": _STUB_CLASSES[ci], "icon": _stub_icon(ci),
                 "active_players": counts[ci], "share": round(counts[ci] / total, 4),
                 "active_players_clean": clean[ci],
                 "share_clean": round(clean[ci] / total_clean, 4)}
                for ci in range(len(_STUB_CLASSES))
            ]
            classes.sort(key=lambda c: -c["active_players_clean"])
            return self._send_json({
                "window_start": STUB_ANCHOR - 3600, "window_end": STUB_ANCHOR,
                "duration_hours": 1.0, "total_active": total, "total_active_clean": total_clean,
                "power_rank_threshold": 25000, "classes": classes,
                "methodology": "stub class current", "computed_at": STUB_ANCHOR,
            })
        if path == "/site/leaderboards/cheaters":
            # Three synthetic flagged players spanning the confidence
            # range - so the page's filter slider has something to
            # show + hide as the threshold moves. Confidence values
            # mirror what the real detection module would compute.
            return self._send_json({
                "players": [
                    {
                        "player_name": "Cheater1",
                        "confidence": 0.998,  # 2 strong boards, noisy-OR
                        "leaderboards": [
                            {
                                "uuid": 1012, "name": "GLYPH KICKER",
                                "category": "CONTESTS", "contest_type": "daily",
                                "rank": 1, "score": 99999, "confidence": 0.99,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 99,999 is 42.0 robust z-scores above this board's median of 50,000 (threshold: 3.5). The MAD-based check is resistant to cheaters inflating their own baseline.",
                                        "measurements": {
                                            "player_score": 99999, "peer_median": 50000,
                                            "peer_mad": 1200, "modified_z_score": 42.0,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 5000,
                                        },
                                        "confidence": 0.99,
                                    },
                                    {
                                        "type": "rank_gap",
                                        "summary": "Rank-1 score 99,999 is 27x the typical between-rank gap on this board (49% vs typical 1.8%). Next-rank score: 51,000.",
                                        "measurements": {
                                            "player_rank": 1, "player_score": 99999,
                                            "next_rank": 2, "next_rank_score": 51000,
                                            "gap_fraction": 0.49, "typical_gap_fraction": 0.018,
                                            "gap_multiplier": 27.2,
                                            "threshold_multiplier": 10.0,
                                        },
                                        "confidence": 0.94,
                                    },
                                ],
                            },
                            {
                                "uuid": 20, "name": "GEODE MASTERY POINTS",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 1, "score": 875000, "confidence": 0.99,
                                "evidence": [
                                    {
                                        "type": "velocity_outlier",
                                        "summary": "Score gained 800,000 in 1.0h (rate 800,000/h). This board's peer p95 rate is 500/h - this player is 1,600x faster.",
                                        "measurements": {
                                            "score_delta": 800000, "duration_hours": 1.0,
                                            "rate_per_hour": 800000, "peer_p95_rate_per_hour": 500,
                                            "rate_multiplier": 1600,
                                            "threshold_multiplier": 10.0,
                                            "previous_anchor": 1780743600,
                                            "previous_score": 75000,
                                        },
                                        "confidence": 0.99,
                                    },
                                ],
                            },
                        ],
                    },
                    {
                        "player_name": "noa00__00",
                        "confidence": 0.94,  # score-outlier + weekly-uptime (2 check types)
                        "leaderboards": [
                            {
                                "uuid": 2004, "name": "CHALLENGE: Deepest (WEEKLY)",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 1, "score": 9999, "confidence": 0.94,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 9,999 is 7.0 robust z-scores above this board's median of 142 (threshold: 3.5).",
                                        "measurements": {
                                            "player_score": 9999, "peer_median": 142,
                                            "peer_mad": 18, "modified_z_score": 7.0,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 250,
                                        },
                                        "confidence": 0.816,
                                    },
                                    {
                                        "type": "sustained_velocity",
                                        "summary": "Score rose in 158 of the last 162 hourly captures since the weekly reset (98% uptime). No human plays 85%+ of every hour for days - this account essentially never stops, the signature of a no-sleep bot. Invisible to the per-hour check (each hour looks normal alone).",
                                        "measurements": {
                                            "active_hours": 158, "captures_since_reset": 162,
                                            "uptime_fraction": 0.975, "threshold_fraction": 0.85,
                                        },
                                        "confidence": 0.94,
                                    },
                                ],
                            },
                        ],
                    },
                    {
                        # Borderline player - flagged but right at the
                        # threshold. Confidence 0.5; default filter
                        # (0.9) should hide this one.
                        "player_name": "BorderlineBob",
                        "confidence": 0.5,
                        "leaderboards": [
                            {
                                "uuid": 33001, "name": "HART-A-PHONES RECEIVED",
                                "category": "CONTESTS", "contest_type": "weekly",
                                "rank": 4, "score": 800, "confidence": 0.5,
                                "evidence": [
                                    {
                                        "type": "score_outlier",
                                        "summary": "Score 800 is 3.5 robust z-scores above this board's median of 220 (threshold: 3.5).",
                                        "measurements": {
                                            "player_score": 800, "peer_median": 220,
                                            "peer_mad": 110, "modified_z_score": 3.55,
                                            "threshold": 3.5, "higher_is_better": True,
                                            "board_size": 80,
                                        },
                                        "confidence": 0.5,
                                    },
                                ],
                            },
                        ],
                    },
                ],
                # Alt-clusters (group-shaped detection). Two clear families
                # mirroring the real anana1..20 / Aan_1..7 pattern, plus a
                # borderline trio below the default 0.9 filter.
                "clusters": [
                    {
                        "stem": "anana", "label": "anana*",
                        "member_count": 20,
                        "members": [f"anana{i}" for i in range(1, 21)],
                        "members_truncated": 0,
                        "board_count": 2,
                        "boards": [
                            {
                                "uuid": 4001, "name": "GEODE TOPSIDE U9",
                                "category": "GEODE", "contest_type": None,
                                "members": 20, "score_min": 240.79, "score_max": 240.8,
                                "spread": 0.000042, "rank_min": 2, "rank_max": 17,
                            },
                            {
                                "uuid": 4002, "name": "DELVE DEPTH",
                                "category": "DELVE", "contest_type": "daily",
                                "members": 14, "score_min": 1880, "score_max": 1881,
                                "spread": 0.00053, "rank_min": 3, "rank_max": 22,
                            },
                        ],
                        "confidence": 0.948,
                        "summary": "20 similarly-named accounts (anana1, anana10, anana11, … anana9) cluster within 0.0042% of each other on 2 board(s). Coordinated multi-account ('alt army') pattern: near-identical scores under a shared name stem 'anana'.",
                        "measurements": {
                            "member_count": 20, "board_count": 2,
                            "tightest_spread_pct": 0.0042, "score_band_pct": 2.0,
                            "closeness": 0.998, "size_term": 1.0, "board_term": 0.75,
                            "ceiling": 0.95,
                        },
                    },
                    {
                        "stem": "aan", "label": "aan*",
                        "member_count": 7,
                        "members": [f"Aan_{i}" for i in range(1, 8)],
                        "members_truncated": 0,
                        "board_count": 1,
                        "boards": [
                            {
                                "uuid": 4001, "name": "GEODE TOPSIDE U9",
                                "category": "GEODE", "contest_type": None,
                                "members": 7, "score_min": 240.78, "score_max": 240.79,
                                "spread": 0.000042, "rank_min": 8, "rank_max": 18,
                            },
                        ],
                        "confidence": 0.904,
                        "summary": "7 similarly-named accounts (Aan_1, Aan_2, Aan_3, … Aan_7) cluster within 0.0042% of each other on 1 board(s). Coordinated multi-account ('alt army') pattern: near-identical scores under a shared name stem 'aan'.",
                        "measurements": {
                            "member_count": 7, "board_count": 1,
                            "tightest_spread_pct": 0.0042, "score_band_pct": 2.0,
                            "closeness": 0.998, "size_term": 0.8, "board_term": 0.5,
                            "ceiling": 0.95,
                        },
                    },
                    {
                        # Borderline: 3 accounts, loose band - below the 0.9
                        # default filter so the slider has a cluster to hide.
                        "stem": "dragon", "label": "dragon*",
                        "member_count": 3,
                        "members": ["Dragon", "Dragon2", "Dragon3"],
                        "members_truncated": 0,
                        "board_count": 1,
                        "boards": [
                            {
                                "uuid": 5001, "name": "U10 POWER RANK",
                                "category": "CLASS", "contest_type": None,
                                "members": 3, "score_min": 30100, "score_max": 30600,
                                "spread": 0.0163, "rank_min": 41, "rank_max": 58,
                            },
                        ],
                        "confidence": 0.62,
                        "summary": "3 similarly-named accounts (Dragon, Dragon2, Dragon3) cluster within 1.63% of each other on 1 board(s). Coordinated multi-account ('alt army') pattern: near-identical scores under a shared name stem 'dragon'.",
                        "measurements": {
                            "member_count": 3, "board_count": 1,
                            "tightest_spread_pct": 1.63, "score_band_pct": 2.0,
                            "closeness": 0.185, "size_term": 0.0, "board_term": 0.5,
                            "ceiling": 0.95,
                        },
                    },
                    {
                        # Co-movement (primary, name-agnostic): distinct names, lockstep gains.
                        "stem": "", "label": "xX_Reaper_Xx +2", "method": "co_movement",
                        "corroborated_by": ["co_movement", "footprint"],
                        "member_count": 3,
                        "members": ["xX_Reaper_Xx", "moonpie42", "Vortex_Prime"],
                        "members_truncated": 0,
                        "board_count": 2,
                        "boards": [
                            {
                                "uuid": 20, "name": "GEODE MASTERY POINTS",
                                "category": "GEODE", "contest_type": None,
                                "members": 3, "matching_hours": 6, "avg_hourly_gain": 48500000,
                                "member_names": ["xX_Reaper_Xx", "moonpie42", "Vortex_Prime"],
                            },
                            {
                                "uuid": 4002, "name": "DELVE DEPTH",
                                "category": "DELVE", "contest_type": "daily",
                                "members": 3, "matching_hours": 4, "avg_hourly_gain": 1920,
                                "member_names": ["xX_Reaper_Xx", "moonpie42", "Vortex_Prime"],
                            },
                        ],
                        "confidence": 0.93,
                        "summary": "3 accounts gained in LOCKSTEP - matching hourly score deltas across 6 hour(s) since the weekly reset on 2 board(s) (Vortex_Prime, moonpie42, xX_Reaper_Xx). Avg matched gain ~290,000,000/hr. Coordinated alts/bots progress together regardless of name.",
                        "measurements": {
                            "matching_hours": 6, "group_size": 3, "board_count": 2,
                            "avg_hourly_gain": 290000000, "name_corroborated": False,
                            "name_stem": None, "matching_term": 0.87, "size_term": 0.75,
                            "ceiling": 0.97,
                        },
                    },
                    {
                        # Co-movement + shared name stem -> method "both" (top confidence).
                        "stem": "grindbot", "label": "grindbot*", "method": "both",
                        "corroborated_by": ["co_movement", "schedule", "name_stem", "footprint"],
                        "member_count": 5,
                        "members": ["grindbot1", "grindbot2", "grindbot3", "grindbot4", "grindbot5"],
                        "members_truncated": 0,
                        "board_count": 1,
                        "boards": [
                            {
                                "uuid": 20, "name": "GEODE MASTERY POINTS",
                                "category": "GEODE", "contest_type": None,
                                "members": 5, "matching_hours": 9, "avg_hourly_gain": 51000000,
                            },
                        ],
                        "confidence": 0.97,
                        "summary": "5 accounts gained in LOCKSTEP - matching hourly score deltas across 9 hour(s) since the weekly reset on 1 board(s) (grindbot1, grindbot2, grindbot3, … grindbot5). Avg matched gain ~459,000,000/hr. Coordinated alts/bots progress together regardless of name; this group also shares the name stem 'grindbot'.",
                        "measurements": {
                            "matching_hours": 9, "group_size": 5, "board_count": 1,
                            "avg_hourly_gain": 459000000, "name_corroborated": True,
                            "name_stem": "grindbot", "matching_term": 0.95, "size_term": 0.94,
                            "ceiling": 0.97,
                        },
                    },
                ],
                "computed_at": 1780870000,
                "anchor": STUB_ANCHOR,
                "method": "Four independent checks: Modified Z-score (MAD-based, Iglewicz & Hoaglin 1993), rank-gap ratio, and velocity vs peer p95 flag individual outliers; alt-cluster detection groups similarly-named accounts at near-identical scores.",
                "config": {
                    "z_threshold": 3.5,
                    "velocity_multiplier": 10.0,
                    "min_board_size": 20,
                },
                "total_flagged": 3,
                "boards_analyzed": 25,
                "clusters_boards_scanned": 80,
            })
        if path == "/site/giveaways":
            import time as _t
            from datetime import datetime, timezone
            def _iso(off):
                return datetime.fromtimestamp(int(_t.time()) + off, tz=timezone.utc).isoformat()
            return self._send_json({"items": [
                {"id": "1", "title": "Holiday code drop", "description": "A shiny Trove mount code for one lucky winner.",
                 "prize_name": "Trove Mount Code", "status": "open",
                 "starts_at": _iso(-86400), "ends_at": _iso(3 * 86400), "entry_count": 42, "winner_username": None},
                {"id": "2", "title": "Next week's giveaway", "description": "Coming soon - a mystery Trove prize.",
                 "prize_name": "Mystery Prize", "status": "scheduled",
                 "starts_at": _iso(2 * 86400), "ends_at": _iso(5 * 86400), "entry_count": 0, "winner_username": None},
                {"id": "3", "title": "Last month's draw", "description": "Already drawn + the code is on its way.",
                 "prize_name": "Glim Pack", "status": "drawn",
                 "starts_at": _iso(-10 * 86400), "ends_at": _iso(-3 * 86400), "entry_count": 118, "winner_username": "Aallyn"},
                {"id": "4", "title": "Quiet one", "description": None,
                 "prize_name": "Costume Code", "status": "closed",
                 "starts_at": _iso(-20 * 86400), "ends_at": _iso(-15 * 86400), "entry_count": 0, "winner_username": None},
            ]})
        if path == "/site/leaderboards/timestamps":
            return self._send_json({"items": STUB_TIMESTAMPS, "count": len(STUB_TIMESTAMPS)})
        if path == "/site/leaderboards/boards":
            qs = parse_qs(url.query)
            created_at = int(qs.get("created_at", [str(STUB_ANCHOR)])[0])
            return self._send_json({
                "created_at": created_at, "items": STUB_BOARDS,
                "count": len(STUB_BOARDS),
            })
        if path.startswith("/site/leaderboards/") and path.endswith("/entries"):
            parts = path.split("/")
            uuid = int(parts[3])
            qs = parse_qs(url.query)
            created_at = int(qs.get("created_at", [str(STUB_ANCHOR)])[0])
            limit = int(qs.get("limit", ["100"])[0])
            offset = int(qs.get("offset", ["0"])[0])
            page = STUB_ENTRIES[offset:offset + limit]
            return self._send_json({
                "uuid": uuid, "created_at": created_at,
                "items": page, "count": len(page), "total": len(STUB_ENTRIES),
            })
        if path.startswith("/site/leaderboards/players/") and path.endswith("/history"):
            queried = unquote(path.split("/")[4])
            # Mirror the prod case-insensitive match (service.py uses
            # an anchored ``$regex`` with ``i``). Return rows ONLY for
            # the small set of stub players, regardless of input casing.
            known = {e["player_name"].lower(): e["player_name"] for e in STUB_ENTRIES}
            canonical = known.get(queried.lower())
            if canonical is None:
                return self._send_json({
                    "player_name": queried, "items": [], "count": 0,
                })
            return self._send_json({
                "player_name": canonical,
                "items": [
                    {"player_name": canonical, "rank": 1, "score": 59731.0,
                     "leaderboard": 1012, "created_at": STUB_ANCHOR},
                    {"player_name": canonical, "rank": 4, "score": 12500.0,
                     "leaderboard": 20, "created_at": STUB_ANCHOR},
                ],
                "count": 2,
            })

        if path.startswith("/site/leaderboards/players/") and path.endswith("/series"):
            # Synthetic per-board score series so the cluster progress chart
            # renders. Members of the alt cluster (same gain rate, slight base
            # offset) come out as parallel rising lines = visible lockstep.
            name = unquote(path.split("/")[4])
            _HR = 3600
            sanchors = [STUB_ANCHOR - (11 - i) * _HR for i in range(12)]
            seed = sum(ord(c) for c in name)

            def _pts(base, rate):
                out, cur = [], float(base)
                for a in sanchors:
                    out.append({"created_at": a, "score": round(cur, 2),
                                "rank": 1 + (seed % 6), "synthetic": False})
                    cur += rate
                return out

            return self._send_json({
                "player_name": name, "canonical_name": name, "days": 7,
                "anchors": sanchors,
                "series": [
                    {"uuid": 20, "name": "GEODE MASTERY POINTS",
                     "points": _pts(1_000_000_000 + seed * 1000, 48_500_000)},
                    {"uuid": 4002, "name": "DELVE DEPTH",
                     "points": _pts(1880 + (seed % 10), 1920)},
                ],
            })

        self.send_error(404)

    def _send_file(self, p: Path, content_type: str | None):
        if not p.exists():
            return self.send_error(404)
        if content_type is None:
            suffix = p.suffix.lower()
            content_type = {
                ".html": "text/html", ".css": "text/css", ".js": "application/javascript",
                ".json": "application/json", ".png": "image/png", ".jpg": "image/jpeg",
                ".jpeg": "image/jpeg", ".gif": "image/gif", ".webp": "image/webp",
                ".svg": "image/svg+xml", ".ico": "image/x-icon",
            }.get(suffix, "application/octet-stream")
        data = p.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _send_json(self, obj):
        data = json.dumps(obj).encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _send_bytes(self, data: bytes, content_type: str):
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _send_text(self, text):
        data = text.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def log_message(self, fmt, *args):
        print(f"[{self.address_string()}] {fmt % args}")


if __name__ == "__main__":
    print("[site-dev] http://localhost:8913/leaderboards")
    # ThreadingHTTPServer (not the single-threaded HTTPServer): the browser
    # preview opens several keep-alive connections at once, which would wedge a
    # single-threaded server mid-request. Each request is independent stub data,
    # so threading is safe here.
    ThreadingHTTPServer(("127.0.0.1", 8913), Handler).serve_forever()
