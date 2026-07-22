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
import re
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlparse, parse_qs, unquote

# Board-icon name guard (mirrors the prod endpoint's path-traversal guard).
_re_icon = re.compile(r"^[a-z0-9_]+$")

# An 8x8 slate PNG so stubbed image routes (banners/previews) render something
# instead of leaving the <img> request hanging.
_PLACEHOLDER_PNG = base64.b64decode(
    "iVBORw0KGgoAAAANSUhEUgAAAAgAAAAICAIAAABLbSncAAAAEUlEQVR42mOwcYrCihiGlgQAEoM2AX8snL0AAAAASUVORK5CYII="
)

ROOT = Path(__file__).resolve().parent.parent
SITE_DIR = ROOT / "site"
TEMPLATES = SITE_DIR / "templates"
STATIC = SITE_DIR / "static"


def _under(base: Path, *parts: str) -> Path | None:
    """Resolve ``base / *parts`` and return it ONLY if it stays inside ``base``.

    Both an absolute part and a ``..`` escape resolve to a path outside ``base``,
    so this rejects every traversal (dev-only static preview hardening)."""
    root = base.resolve()
    candidate = (root / Path(*parts)).resolve()
    if candidate == root or root in candidate.parents:
        return candidate
    return None

# The gem tools (Evaluator / Builds / Calculators star-chart preview) run the
# real service layer - it's dependency-light (stdlib + gamedata JSON + pydantic
# schemas), so we import it here and answer the /site/gems/* proxies with the
# same maths production serves. Guarded so the rest of the dev server still boots
# if the app package can't be imported in a bare interpreter.
import sys as _sys
if str(ROOT) not in _sys.path:
    _sys.path.insert(0, str(ROOT))
try:
    from app.trove.gems import builds as _gem_builds
    from app.trove.gems import evaluator as _gem_evaluator
    from app.trove.gems.model import gem_lookups as _gem_lookups
    _GEMS_OK = True
except Exception as _e:  # noqa: BLE001 - dev-only, degrade gracefully
    print(f"[site-dev] gem tools unavailable ({_e}); /site/gems/* will 503")
    _GEMS_OK = False

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

# The 18 Trove classes in the boards' class RELEASE order (= the Power
# Rank/Effort/Paragon board offset order; matches stats._BOARD_CLASS_ORDER),
# for the Class Activity page stubs.
_STUB_CLASSES = [
    "Knight", "Gunslinger", "Fae Trickster", "Dracolyte", "Neon Ninja",
    "Candy Barbarian", "Ice Sage", "Shadow Hunter", "Pirate Captain",
    "Boomeranger", "Tomb Raiser", "Lunar Lancer", "Revenant", "Chloromancer",
    "Dino Tamer", "Vanguardian", "Bard", "Solarion",
]
# qualified_name per class (same release order) → self-hosted icon path.
_STUB_CLASS_QN = [
    "knight", "gunslinger", "faetrickster", "dracolyte", "neonninja",
    "candybarbarian", "icemage", "shadowhunter", "piratelord",
    "adventurer", "tombraiser", "lunarlancer", "spirittank", "chloromancer",
    "dinotamer", "crimefighter", "bard", "solarion",
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
    {"slug": "shared-cursor", "title": "Shared Cursor Pack",
     "summary": "A great mod someone else made, shared here for the community.",
     "tags": ["ui", "cursor"], "owner_username": "Aallyn",
     "visibility": "public", "banner_sha": None, "download_count": 940,
     "updated_at": None, "created_at": None,
     "uploaded_on_behalf": True, "author": "OriginalCreator", "mode": "releases"},
]
for _m in _STUB_MODS:
    _m.setdefault("forked_from", None)
    _m.setdefault("inspired_by", None)
    _m.setdefault("fork_count", 0)
    _m.setdefault("mode", "files")
    _m.setdefault("star_count", 0)
    _m.setdefault("preview_sha", None)
    _m.setdefault("uploaded_on_behalf", False)
    _m.setdefault("author", "")
    _m.setdefault("is_stray", False)
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


# Synthetic file tree for the /updates archive browser. Deliberately has files
# buried a few directories deep so the full-tree search (which must surface
# matches inside un-expanded folders) can be exercised locally.
_UPDATE_PATHS = [
    "prefabs/dungeons/cursed_vale/boss.blueprint",
    "prefabs/dungeons/cursed_vale/minion.blueprint",
    "prefabs/dungeons/frozen_peak/boss.blueprint",
    "prefabs/equipment/sword_epic.blueprint",
    "textures/ui/health_bar.dds",
    "textures/blocks/stone_diffuse.dds",
    "ui/hud/health_bar.png",
    "ui/hud/mana_bar.png",
    "scripts/combat/damage.lua",
    "scripts/combat/healing.lua",
    "languages/en/strings.json",
    "readme.md",
] + [f"blueprints/model_{i:04d}.blueprint" for i in range(500)]   # exercise sidebar "load more"

# Synthetic "last modified" data so the /updates last-modified sort is exercisable:
# a handful of paths were touched in the newest version (ordinal 2), the rest in v1.
_UPDATE_VERSION_DATES = {2: "2026-07-15T12:00:00+00:00", 1: "2026-06-01T09:30:00+00:00"}
_UPDATE_TOUCHED_V2 = {
    "prefabs/dungeons/cursed_vale/boss.blueprint",
    "prefabs/dungeons/cursed_vale/minion.blueprint",
    "textures/ui/health_bar.dds",
    "textures/blocks/stone_diffuse.dds",
    "ui/hud/health_bar.png",
    "scripts/combat/damage.lua",
}


def _last_ordinal_for(path: str) -> int:
    return 2 if path in _UPDATE_TOUCHED_V2 else 1


def _stub_dds(w=16, h=16):
    """A tiny uncompressed 32-bit BGRA DDS (red-x / green-y gradient) so the DDS
    preview + 'download as PNG' path can be exercised locally."""
    import struct
    hdr = bytearray(128)
    hdr[0:4] = b"DDS "
    struct.pack_into("<I", hdr, 4, 124)                       # dwSize
    struct.pack_into("<I", hdr, 8, 0x1 | 0x2 | 0x4 | 0x8 | 0x1000)  # caps/h/w/pitch/pf
    struct.pack_into("<I", hdr, 12, h)
    struct.pack_into("<I", hdr, 16, w)
    struct.pack_into("<I", hdr, 20, w * 4)                    # pitch
    struct.pack_into("<I", hdr, 76, 32)                       # ddspf size
    struct.pack_into("<I", hdr, 80, 0x41)                     # DDPF_RGB|DDPF_ALPHAPIXELS
    struct.pack_into("<I", hdr, 88, 32)                       # bit count
    struct.pack_into("<I", hdr, 92, 0x00FF0000)               # R mask
    struct.pack_into("<I", hdr, 96, 0x0000FF00)               # G mask
    struct.pack_into("<I", hdr, 100, 0x000000FF)              # B mask
    struct.pack_into("<I", hdr, 104, 0xFF000000)              # A mask
    struct.pack_into("<I", hdr, 108, 0x1000)                  # DDSCAPS_TEXTURE
    px = bytearray()
    for y in range(h):
        for x in range(w):
            r = int(255 * x / (w - 1)); g = int(255 * y / (h - 1)); b = 128
            px += bytes((b, g, r, 255))                       # BGRA byte order
    return bytes(hdr) + bytes(px)


def _update_dir_listing(prefix):
    """One ls-level of children for `prefix`, mirroring read.directory_listing."""
    children: dict[str, dict] = {}
    plen = len(prefix)
    for path in _UPDATE_PATHS:
        if not path.startswith(prefix):
            continue
        rest = path[plen:]
        if not rest:
            continue
        slash = rest.find("/")
        name = rest if slash == -1 else rest[:slash]
        c = children.setdefault(name, {
            "name": name, "path": prefix + name,
            "is_dir": slash != -1, "file_count": 0, "size": 0, "last_ordinal": 0})
        if slash != -1:
            c["is_dir"] = True
            c["path"] = prefix + name + "/"
        c["file_count"] += 1
        c["size"] += 512
        c["last_ordinal"] = max(c["last_ordinal"], _last_ordinal_for(path))
    for c in children.values():
        c["last_modified_at"] = _UPDATE_VERSION_DATES.get(c["last_ordinal"])
    return sorted(children.values(), key=lambda c: (not c["is_dir"], c["name"]))


# ── Recipe Cost Calculator (/codexes/crafting) synthetic data ──────────────
# A small nested recipe graph: "Radiant Sovereign" needs Radiant Shards (itself
# craftable) + Golden Thread + one Enchanted Ember that is deliberately NOT priced
# so the "no market data" path is exercised. Mirrors the real build_tree output.
_CRAFT_PRICES = {
    "Radiant Sovereign": {"median_each": 45000.0, "count": 6},
    "Radiant Shard": {"median_each": 4200.0, "count": 9},
    "Sunlight Bulb": {"median_each": 120.0, "count": 40},
    "Golden Thread": {"median_each": 850.0, "count": 22},
    # "Enchanted Ember" intentionally absent → price-unknown leaf.
}
_CRAFT_RECIPES = {
    "item/craft/radiant_sovereign": {
        "source_path": "prefabs/recipes/recipe_radiant_sovereign",
        "output": {"path": "item/craft/radiant_sovereign", "name": "Radiant Sovereign", "amount": 1},
        "ingredients": [
            {"path": "item/craft/radiant_shard", "name": "Radiant Shard", "amount": 4},
            {"path": "item/craft/golden_thread", "name": "Golden Thread", "amount": 8},
            {"path": "item/craft/enchanted_ember", "name": "Enchanted Ember", "amount": 1},
        ],
    },
    "item/craft/radiant_shard": {
        "source_path": "prefabs/recipes/recipe_radiant_shard",
        "output": {"path": "item/craft/radiant_shard", "name": "Radiant Shard", "amount": 2},
        "ingredients": [
            {"path": "item/craft/sunlight_bulb", "name": "Sunlight Bulb", "amount": 10},
            {"path": "item/craft/golden_thread", "name": "Golden Thread", "amount": 2},
        ],
    },
}
_CRAFT_SOURCE_TO_OUTPUT = {r["source_path"]: out for out, r in _CRAFT_RECIPES.items()}
_CRAFT_SEARCH = [
    {"type": "recipe", "path": r["source_path"], "name": r["output"]["name"],
     "category": "Mount" if "sovereign" in out else "Crafting", "tradable": None,
     "data": {"recipe": {"output": r["output"]}}}
    for out, r in _CRAFT_RECIPES.items()
]


def _craft_raw(path, name, need, depth, stack, counter, names):
    import math
    counter[0] += 1
    names.add(name)
    node = {"path": path, "name": name, "need": need,
            "craftable": False, "crafts": None, "output_amount": None, "children": []}
    rec = _CRAFT_RECIPES.get(path)
    if rec and depth < 20 and path not in stack and counter[0] < 600:
        out_amt = rec["output"]["amount"] or 1
        crafts = max(1, math.ceil(need / out_amt))
        node["craftable"] = True
        node["crafts"] = crafts
        node["output_amount"] = out_amt
        deeper = stack | {path}
        for ing in rec["ingredients"]:
            node["children"].append(
                _craft_raw(ing["path"], ing["name"], ing["amount"] * crafts,
                           depth + 1, deeper, counter, names))
    return node


def _craft_annotate(node):
    price = _CRAFT_PRICES.get(node["name"])
    unit = price["median_each"] if price else None
    node["market_price_each"] = unit
    node["market_count"] = price["count"] if price else 0
    node["buy_cost"] = round(unit * node["need"], 2) if unit is not None else None
    if node["craftable"] and node["children"]:
        known = 0.0
        all_known = True
        unpriced = 0
        for c in node["children"]:
            _craft_annotate(c)
            if c["best_cost"] is None:
                all_known = False
            else:
                known += c["best_cost"]
            unpriced += c["unpriced_count"]
        node["craft_cost_partial"] = round(known, 2)
        node["craft_cost"] = round(known, 2) if all_known else None
        node["unpriced_count"] = unpriced
    else:
        node["craft_cost_partial"] = None
        node["craft_cost"] = None
        node["unpriced_count"] = 0 if unit is not None else 1
    buy, craft = node["buy_cost"], node["craft_cost"]
    opts = [x for x in (buy, craft) if x is not None]
    if opts:
        node["best_cost"] = min(opts)
        node["recommendation"] = "craft" if (craft is not None and (buy is None or craft <= buy)) else "buy"
    else:
        node["best_cost"] = None
        node["recommendation"] = "unknown"
    return node


def _craft_build(source_path):
    out_path = _CRAFT_SOURCE_TO_OUTPUT.get(source_path)
    if out_path is None:
        return None
    rec = _CRAFT_RECIPES[out_path]
    out = rec["output"]
    counter = [0]
    names = set()
    raw = _craft_raw(out["path"], out["name"], out["amount"], 0, frozenset(), counter, names)
    tree = _craft_annotate(raw)
    return {"branch": "live-us", "recipe_path": source_path, "output": out,
            "category": "Mount", "root": tree, "node_count": counter[0],
            "priced_items": sum(1 for n in names if n in _CRAFT_PRICES),
            "truncated": False}


# ── /market Analytics tab synthetic data ───────────────────────────────────
_MARKET_ITEMS = ["Radiant Sovereign", "Radiant Shard", "Golden Thread",
                 "Sunlight Bulb", "Shadow Key", "Glim", "Flux Capacitor"]

# Admin-defined sidebar groups (prod: market_item_categories via
# /admin/market/categories). "Ancient Relic" is deliberately not in
# _MARKET_ITEMS - exercises the "categorized but not currently trading"
# path (client intersects, so it just doesn't render). Glim + Flux
# Capacitor land in the "Other" fallback group.
_MARKET_CATEGORIES = [
    {"name": "Currency", "items": ["Radiant Sovereign", "Golden Thread"]},
    {"name": "Crafting", "items": ["Radiant Shard", "Sunlight Bulb", "Ancient Relic"]},
    {"name": "Keys & Chests", "items": ["Shadow Key"]},
]

# Names with stored listings but off the scan allow-list (prod: distinct
# listing names minus MarketInterestItem). Shadow Key is ALSO categorized
# above - exercises the "untracked wins over category" display path (its
# Keys & Chests group goes empty and disappears); Flux Capacitor exercises
# the uncategorized-untracked path.
_MARKET_UNTRACKED = ["Shadow Key", "Flux Capacitor"]


# ── /store (Trove Store History) synthetic catalog ────────────────────────
# Enough variety to exercise the page: live + delisted packs, multi-run
# availability, price history (for the graph), TWC/TWP/real-money currencies,
# sales, deals, and a couple of categories. Anchors are relative to now so
# "live now" + the availability/today marker land correctly.
_DAY = 86400
_STORE_NOW = int(_time.time() // _DAY * _DAY)   # snap to a day boundary


def _store_stub_products():
    now = _STORE_NOW
    return [
        {
            "code": "ITEM_SKIN_KNIGHT_ELYSIAN", "kind": "product",
            "name": "Elysian Guardian", "image": "ui/skins/ui_skin_knight_elysian.dds",
            "info": "A costume for the Knight.\n\nThe purity of the guardians.",
            "informational": False, "tradable": False,
            "prices": [{"currency": "TWC", "cost": 750, "can_purchase": True, "monthly": 0, "sale": ""}],
            "price_string": None, "price_string_currency": None, "price_string_sale": None,
            "promo": None, "deal_expires_at": None,
            "interact_label": None, "interact_enabled": False, "trial_limits": None,
            "class_level": None, "class_power_rank": None, "class_sub_name": None, "class_icon": None,
            "textures": [], "loot_title": None, "loot_body": None,
            "categories": [7], "first_seen": now - 120 * _DAY, "last_seen": now, "active": True,
        },
        {
            "code": "DEAL_DAILY_EMPOWERED_COSMIC_1", "kind": "starter",
            "name": "Cosmic Daily Deal", "image": "ui/store/ui_store_daily_deal_greengembox.dds",
            "info": "An incredible limited-time deal on 1 Empowered Cosmic Gem Box!",
            "informational": False, "tradable": False,
            "prices": [{"currency": "TWC", "cost": 3500, "can_purchase": True, "monthly": 0, "sale": "SALE30"}],
            "price_string": "€1.49", "price_string_currency": "EUR", "price_string_sale": "SALE30",
            "promo": None, "deal_expires_at": now + 12000,
            "interact_label": None, "interact_enabled": False, "trial_limits": None,
            "class_level": None, "class_power_rank": None, "class_sub_name": None, "class_icon": None,
            "textures": [], "loot_title": None, "loot_body": None,
            "categories": [0], "first_seen": now - 3 * _DAY, "last_seen": now, "active": True,
        },
        {
            "code": "TROVE_PATRON_15_CREDITS_NOTRADE", "kind": "patron",
            "name": "15 Day Patron Pass", "image": "ui/store/ui_store_patron_15.dds",
            "info": "An untradable coin which unlocks Patron status for 15 days.",
            "informational": False, "tradable": False,
            "prices": [{"currency": "TWC", "cost": 850, "can_purchase": True, "monthly": 0, "sale": ""}],
            "price_string": None, "price_string_currency": None, "price_string_sale": None,
            "promo": None, "deal_expires_at": None,
            "interact_label": None, "interact_enabled": False, "trial_limits": None,
            "class_level": None, "class_power_rank": None, "class_sub_name": None, "class_icon": None,
            "textures": [], "loot_title": None, "loot_body": None,
            "categories": [3], "first_seen": now - 300 * _DAY, "last_seen": now, "active": True,
        },
        {
            "code": "ITEM_SKIN_TOMBRAISER_PHARAOH", "kind": "product",
            "name": "Funereal Pharaoh", "image": "ui/skins/ui_skin_ice_tombraiser_pharaoh.dds",
            "info": "A costume for the Tomb Raiser.\n\nRise and haunt the night!",
            "informational": False, "tradable": False,
            "prices": [{"currency": "TWP", "cost": 500, "can_purchase": True, "monthly": 0, "sale": ""}],
            "price_string": None, "price_string_currency": None, "price_string_sale": None,
            "promo": None, "deal_expires_at": None,
            "interact_label": None, "interact_enabled": False, "trial_limits": None,
            "class_level": None, "class_power_rank": None, "class_sub_name": None, "class_icon": None,
            "textures": [], "loot_title": None, "loot_body": None,
            # Seasonal: gone right now (last seen 40 days ago).
            "categories": [7], "first_seen": now - 400 * _DAY, "last_seen": now - 40 * _DAY, "active": False,
        },
        {
            "code": "TROVE_BAMBOODRAGON_PACK", "kind": "starter",
            "name": "Bamboo Dragon Pack", "image": "ui/store/ui_store_megapack_bamboo.dds",
            "info": "A mega pack.",
            "informational": False, "tradable": False,
            # Real-money currency in prices[] with no pre-formatted string:
            # cost is in cents (1499 -> €14.99), exercises the fmtCash path.
            "prices": [{"currency": "EUR", "cost": 1499, "can_purchase": True, "monthly": 0, "sale": ""}],
            "price_string": None, "price_string_currency": None, "price_string_sale": None,
            "promo": None, "deal_expires_at": None,
            "interact_label": None, "interact_enabled": False, "trial_limits": None,
            "class_level": None, "class_power_rank": None, "class_sub_name": None, "class_icon": None,
            "textures": [], "loot_title": None, "loot_body": None,
            "categories": [0, 8], "first_seen": now - 200 * _DAY, "last_seen": now - 90 * _DAY, "active": False,
        },
    ]


_STORE_CATEGORIES = [
    {"index": 0, "label": "$StoreCategory_Deals", "icon": None},
    {"index": 3, "label": "$StoreCategory_Patron", "icon": None},
    {"index": 7, "label": "$StoreCategory_Style", "icon": None},
    {"index": 8, "label": "$StoreCategory_More", "icon": None},
]


def _store_availability(p):
    """Synthetic availability intervals for a stub product (multi-run for the
    seasonal skin so the timeline shows gaps)."""
    now = _STORE_NOW
    if p["code"] == "ITEM_SKIN_TOMBRAISER_PHARAOH":
        return [[now - 400 * _DAY, now - 380 * _DAY],
                [now - 120 * _DAY, now - 100 * _DAY],
                [now - 60 * _DAY, now - 40 * _DAY]]
    return [[p["first_seen"], p["last_seen"]]]


def _store_price_history(p):
    """A couple of price points so the detail graph renders."""
    fs = p["first_seen"]
    base = p["prices"][0] if p["prices"] else {"currency": "TWC", "cost": 1000, "can_purchase": True, "monthly": 0, "sale": ""}
    hi = dict(base, cost=base["cost"] * 4 // 3)   # was pricier earlier
    return [
        {"ts": fs, "prices": [hi], "price_string": ""},
        {"ts": fs + 30 * _DAY, "prices": [base], "price_string": p.get("price_string") or ""},
    ]


def _store_records(p):
    av = _store_availability(p)
    total = sum(max(1, round((e - s) / _DAY) + 1) for s, e in av)
    longest = max((max(1, round((e - s) / _DAY) + 1) for s, e in av), default=0)
    lows, highs = {}, {}
    for pt in _store_price_history(p):
        for pr in pt["prices"]:
            lows[pr["currency"]] = min(lows.get(pr["currency"], pr["cost"]), pr["cost"])
            highs[pr["currency"]] = max(highs.get(pr["currency"], pr["cost"]), pr["cost"])
    return {
        "times_available": len(av), "returns": max(0, len(av) - 1),
        "total_days_seen": total, "longest_run_days": longest,
        "first_seen": p["first_seen"], "last_seen": p["last_seen"],
        "currently_active": p["active"],
        "gap_days": None if p["active"] else max(0, (_STORE_NOW - p["last_seen"]) // _DAY),
        "price_low": lows or None, "price_high": highs or None,
        "price_changes": len(_store_price_history(p)),
    }


def _market_movers():
    import time
    return {"days": 14, "now": int(time.time()),
            "risers": [
                {"name": "Shadow Key", "recent_med": 5200.0, "prior_med": 4000.0, "recent_n": 40, "change": 0.30},
                {"name": "Golden Thread", "recent_med": 950.0, "prior_med": 850.0, "recent_n": 22, "change": 0.1176}],
            "fallers": [
                {"name": "Sunlight Bulb", "recent_med": 95.0, "prior_med": 130.0, "recent_n": 50, "change": -0.2692},
                {"name": "Radiant Sovereign", "recent_med": 41000.0, "prior_med": 47000.0, "recent_n": 6, "change": -0.1277}]}


def _market_deals():
    import time
    now = int(time.time())
    return {"min_discount": 0.25, "days": 14, "items": [
        {"id": "uuid-1", "name": "Shadow Key", "stack": 20, "price": 80000, "price_each": 4000.0,
         "median_each": 5200.0, "sample_size": 40, "discount": 0.2308, "created_at": now - 3600, "last_seen": now - 600},
        {"id": "uuid-2", "name": "Golden Thread", "stack": 100, "price": 68000, "price_each": 680.0,
         "median_each": 950.0, "sample_size": 22, "discount": 0.2842, "created_at": now - 7200, "last_seen": now - 1200}],
        "count": 2}


def _market_overview(days):
    import time
    now = int(time.time())
    return {
        "active_listings": 1843, "active_items": 62, "total_value": 418_500_000,
        "total_units": 92_400, "days": days, "now": now,
        "top_mover": {"name": "Shadow Key", "recent_med": 5200.0, "prior_med": 4000.0,
                      "recent_n": 40, "change": 0.30},
        "top_traded": {"name": "Glim", "listings": 210, "units": 41000,
                       "total_value": 61_500_000, "median_each": 1.5},
    }


def _market_liquidity(days):
    import time
    now = int(time.time())
    h = 3600
    return {"days": days, "now": now, "ttl_seconds": 7 * 86400, "items": [
        {"name": "Shadow Key", "concluded": 120, "sold": 108, "expired": 12,
         "sell_through": 0.90, "median_time_to_sell": 9 * h},
        {"name": "Golden Thread", "concluded": 64, "sold": 44, "expired": 20,
         "sell_through": 0.6875, "median_time_to_sell": 26 * h},
        {"name": "Sunlight Bulb", "concluded": 200, "sold": 96, "expired": 104,
         "sell_through": 0.48, "median_time_to_sell": 40 * h},
        {"name": "Radiant Sovereign", "concluded": 30, "sold": 6, "expired": 24,
         "sell_through": 0.20, "median_time_to_sell": 55 * h},
        {"name": "Flux Capacitor", "concluded": 18, "sold": 2, "expired": 16,
         "sell_through": 0.1111, "median_time_to_sell": None}]}


def _market_volume(days):
    import time
    return {"days": days, "now": int(time.time()), "items": [
        {"name": "Glim", "listings": 210, "units": 41000, "total_value": 61_500_000, "median_each": 1.5},
        {"name": "Sunlight Bulb", "listings": 150, "units": 12000, "total_value": 1_140_000, "median_each": 95.0},
        {"name": "Shadow Key", "listings": 88, "units": 1760, "total_value": 9_152_000, "median_each": 5200.0},
        {"name": "Golden Thread", "listings": 54, "units": 5400, "total_value": 5_130_000, "median_each": 950.0},
        {"name": "Radiant Sovereign", "listings": 14, "units": 28, "total_value": 1_148_000, "median_each": 41000.0}]}


def _market_timeline(name, days):
    import math
    import time
    now = int(time.time())
    day = 86400
    base = 1200 + (len(name) * 37 % 800)
    pts = []
    for i in range(days):
        bucket = ((now - (days - 1 - i) * day) // day) * day
        wob = 1 + 0.15 * math.sin(i / 2.0) + ((i * 7 % 5) - 2) * 0.02
        p50 = round(base * wob, 2)
        pts.append({"bucket": bucket, "listings": 8 + (i * 13 % 20), "stack": 100 + (i * 29 % 400),
                    "p50": p50, "p25": round(p50 * 0.85, 2), "p75": round(p50 * 1.2, 2)})
    events = [{"name": "Fluxion", "kind": "merchant", "starts_at": now - 11 * day, "ends_at": now - 8 * day},
              {"name": "Corruxion", "kind": "merchant", "starts_at": now - 6 * day, "ends_at": now - 3 * day}]
    return {"name": name, "days": days, "bucket_hours": 24, "points": pts, "events": events, "now": now}


class Handler(SimpleHTTPRequestHandler):
    def do_GET(self):
        url = urlparse(self.path)
        path = url.path

        # Dev-only static preview: reject any path traversal before a request
        # path is ever turned into a filesystem path, so nothing below can escape
        # the site directories.
        if ".." in path or "\x00" in path:
            return self.send_error(400)

        # Page routes.
        if path == "/":
            return self._send_file(TEMPLATES / "index.html", "text/html")
        if path == "/commands":
            return self._send_file(TEMPLATES / "commands.html", "text/html")
        if path == "/leaderboards":
            return self._send_file(TEMPLATES / "leaderboards.html", "text/html")
        if path == "/updates":
            return self._send_file(TEMPLATES / "updates.html", "text/html")
        if path == "/codexes":
            return self._send_file(TEMPLATES / "codexes.html", "text/html")
        if path == "/codexes/crafting":
            return self._send_file(TEMPLATES / "codexes-crafting.html", "text/html")
        if path == "/status":
            return self._send_file(TEMPLATES / "status.html", "text/html")
        if path == "/server-time":
            return self._send_file(TEMPLATES / "server-time.html", "text/html")
        if path == "/calendar":
            return self._send_file(TEMPLATES / "calendar.html", "text/html")
        if path == "/streams":
            return self._send_file(TEMPLATES / "streams.html", "text/html")
        if path == "/releases":
            return self._send_file(TEMPLATES / "releases.html", "text/html")
        if path == "/classes":
            return self._send_file(TEMPLATES / "classes.html", "text/html")
        if path == "/gems-guide":
            return self._send_file(TEMPLATES / "gems-guide.html", "text/html")
        if path == "/gem-simulator":
            return self._send_file(TEMPLATES / "gem-simulator.html", "text/html")
        if path == "/gem-evaluator":
            return self._send_file(TEMPLATES / "gem-evaluator.html", "text/html")
        if path == "/gem-builds":
            return self._send_file(TEMPLATES / "gem-builds.html", "text/html")
        if path == "/calculators":
            return self._send_file(TEMPLATES / "calculators.html", "text/html")
        if path == "/app":
            return self._send_file(TEMPLATES / "app.html", "text/html")
        if path == "/star-chart":
            return self._send_file(TEMPLATES / "star-chart.html", "text/html")
        if path == "/login":
            return self._send_file(TEMPLATES / "login.html", "text/html")
        if path.startswith("/player/"):
            # Public player profile shell. data-player is stripped to empty by the
            # {{ }} emulation, but player.js falls back to the URL path segment.
            return self._send_file(TEMPLATES / "player.html", "text/html")

        # Gem tool proxies (real service layer).
        if path == "/site/gems/lookups":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            return self._send_json(_gem_lookups())
        if path == "/site/gems/builds/options":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            return self._send_json(_gem_builds.build_options())
        if path == "/site/gems/stat-range":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            q = parse_qs(url.query)
            try:
                out = _gem_evaluator.gem_stat_range(
                    int(q.get("tier", [4])[0]), int(q.get("type", [1])[0]),
                    int(q.get("stat_type", [1])[0]), int(q.get("level", [1])[0]),
                    int(q.get("extra_containers", [0])[0]),
                    int(q["element"][0]) if q.get("element") else None,
                )
            except (ValueError, KeyError) as e:
                return self._send_json({"detail": str(e)}, 400)
            return self._send_json(out)
        if path == "/site/gems/parse-star-chart":
            if not _GEMS_OK:
                return self._send_json({"stats": {}, "abilities": [], "paths_count": 0})
            q = parse_qs(url.query)
            code = (q.get("code", [""])[0] or "")
            try:
                return self._send_json(_gem_builds.parse_star_chart(code))
            except Exception:  # noqa: BLE001
                return self._send_json({"stats": {}, "abilities": [], "paths_count": 0})
        if path == "/giveaways":
            return self._send_file(TEMPLATES / "giveaways.html", "text/html")
        if path == "/clubs":
            return self._send_file(TEMPLATES / "clubs.html", "text/html")
        if path == "/activity":
            return self._send_file(TEMPLATES / "activity.html", "text/html")
        if path == "/market":
            return self._send_file(TEMPLATES / "market.html", "text/html")
        if path == "/store":
            return self._send_file(TEMPLATES / "store.html", "text/html")
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
        if path == "/changelog":
            return self._send_file(TEMPLATES / "changelog.html", "text/html")
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
            # Dev: always prefer the un-minified source over its .min build
            # artifact so local edits show up without running minify_static.py
            # (the templates hard-code .min.js/.min.css for production). Every
            # candidate goes through _under, so no raw path is built from input.
            if ".min." in rel:
                unmin = _under(STATIC, rel.replace(".min.", ".", 1))
                if unmin is not None and unmin.exists():
                    rel = rel.replace(".min.", ".", 1)
            target = _under(STATIC, rel)
            if target is None:
                return self.send_error(404)
            return self._send_file(target, None)

        # Stub JSON endpoints.
        # --- Website changelog (/site/changelog) ---------------------------
        if path == "/site/changelog":
            return self._send_json({
                "repo": "AallynReed/KiwiAPI",
                "repo_url": "https://github.com/AallynReed/KiwiAPI",
                "rate_limited": False,
                "groups": [
                    {"version": "Unreleased", "commits": [
                        {"sha": "a" * 40, "short_sha": "aaaaaaa", "type": "feat",
                         "message": "feat: add website changelog page for transparency",
                         "url": "https://github.com/AallynReed/KiwiAPI/commit/" + "a" * 40},
                        {"sha": "b" * 40, "short_sha": "bbbbbbb", "type": "fix",
                         "message": "fix(market): correct sell-through rounding",
                         "url": "https://github.com/AallynReed/KiwiAPI/commit/" + "b" * 40},
                        {"sha": "c" * 40, "short_sha": "ccccccc", "type": None,
                         "message": "Add new leaderboard icons for Paragon Vanguardian",
                         "url": "https://github.com/AallynReed/KiwiAPI/commit/" + "c" * 40},
                    ]},
                    {"version": "v1.4.0", "commits": [
                        {"sha": "d" * 40, "short_sha": "ddddddd", "type": "refactor",
                         "message": "refactor: split the website into its own container",
                         "url": "https://github.com/AallynReed/KiwiAPI/commit/" + "d" * 40},
                        {"sha": "e" * 40, "short_sha": "eeeeeee", "type": "docs",
                         "message": "docs: document the cutover runbook",
                         "url": "https://github.com/AallynReed/KiwiAPI/commit/" + "e" * 40},
                    ]},
                ],
            })
        # --- Store History (/site/store/*) ---------------------------------
        if path == "/site/store/categories":
            items = [dict(c, codes=[p["code"] for p in _store_stub_products()
                                    if c["index"] in p["categories"]],
                          count=sum(1 for p in _store_stub_products() if c["index"] in p["categories"]),
                          active=True) for c in _STORE_CATEGORIES]
            return self._send_json({"items": items, "count": len(items), "anchor": _STORE_NOW})
        if path == "/site/store/products":
            qs = parse_qs(url.query)
            prods = _store_stub_products()
            if qs.get("active", ["true"])[0] == "true":
                prods = [p for p in prods if p["active"]]
            if qs.get("on_sale", ["false"])[0] == "true":
                prods = [p for p in prods if any(x["sale"] for x in p["prices"]) or p["price_string_sale"]]
            kind = qs.get("kind", [""])[0]
            if kind:
                prods = [p for p in prods if p["kind"] == kind]
            q = qs.get("q", [""])[0].lower()
            if q:
                prods = [p for p in prods if q in p["name"].lower()]
            return self._send_json({"items": prods, "count": len(prods),
                                    "total": len(prods), "anchor": _STORE_NOW})
        if path == "/site/store/timeline":
            kind = parse_qs(url.query).get("kind", [""])[0]
            src = [p for p in _store_stub_products() if not kind or p["kind"] == kind]
            items = [{"code": p["code"], "name": p["name"], "kind": p["kind"],
                      "image": p["image"], "availability": _store_availability(p),
                      "first_seen": p["first_seen"], "last_seen": p["last_seen"],
                      "active": p["active"]} for p in src]
            start = min((p["first_seen"] for p in src), default=_STORE_NOW)
            return self._send_json({"anchor": _STORE_NOW,
                                    "span": {"start": start, "end": _STORE_NOW},
                                    "items": items, "count": len(items)})
        if path.startswith("/site/store/products/"):
            code = unquote(path.rsplit("/", 1)[-1])
            p = next((x for x in _store_stub_products() if x["code"] == code), None)
            if p is None:
                return self._send_json({"detail": "not found"}, 404)
            detail = dict(p, price_history=_store_price_history(p),
                          availability=_store_availability(p), records=_store_records(p))
            return self._send_json(detail)
        if path == "/site/store/texture":
            p = parse_qs(url.query).get("path", [""])[0]
            ext = p.rsplit(".", 1)[-1].lower() if "." in p.rsplit("/", 1)[-1] else ""
            if ext == "dds":
                return self._send_bytes(_stub_dds(), "application/octet-stream")
            if ext in ("png", "jpg", "jpeg", "gif", "webp"):
                fav = STATIC / "assets" / "favicon.png"
                if fav.exists():
                    return self._send_file(fav, "image/png")
            return self._send_json({"detail": "not found"}, 404)
        if path.startswith("/site/leaderboards/board-icon/"):
            # Prod serves these from the updates CAS; locally there's no CAS, so
            # serve from the game-file reference copy, falling back to the favicon
            # so tiles still show SOMETHING if the reference folder is absent.
            name = unquote(path[len("/site/leaderboards/board-icon/"):])
            if _re_icon.match(name):
                ref = _under(Path("S:/Downloads/ui/leaderboard_icons"), f"{name}.png")
                if ref is not None and ref.is_file():
                    return self._send_file(ref, "image/png")
            fav = STATIC / "assets" / "favicon.png"
            if fav.exists():
                return self._send_file(fav, "image/png")
            return self._send_json({"detail": "not found"}, 404)

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
        # Stub a release's file list (preview excluded) + per-file download for preview.
        if path.startswith("/site/mods/releases/") and path.endswith("/files"):
            return self._send_json({"items": [
                {"path": "blueprints/c_p_knight_lvl3_torso.blueprint", "size": 1820},
                {"path": "blueprints/c_p_knight_lvl3_l_hand.blueprint", "size": 540},
                {"path": "blueprints/c_p_knight_lvl3_ui.blueprint", "size": 260},
            ]})
        if path.startswith("/site/mods/releases/") and path.endswith("/file"):
            return self._send_bytes(b"kiwib stub file contents\n", "application/octet-stream")
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
                ap = _under(ROOT / "app" / "trove" / "mods_hub" / "rigs" / "anim",
                            m.group(1), m.group(2) + ".json")
                if ap is not None and ap.exists():
                    return self._send_file(ap, "application/json")
            return self._send_json({"error": {"message": "no animation"}})
        if path.startswith("/site/mods/projects/"):
            rest = path[len("/site/mods/projects/"):]
            _parts = rest.split("/")
            handle = _parts[0] if _parts else ""
            slug = _parts[1] if len(_parts) > 1 else ""
            sub = "/".join(_parts[2:])
            base = next((m for m in _STUB_MODS if m["slug"] == slug), _STUB_MODS[0])
            if (sub == "" or sub is None) and slug == "secret-draft":
                # A real draft a non-owner can't see yet: distinct not_public code so
                # the page says "not public yet" instead of "not found".
                return self._send_json(
                    {"error": {"code": "not_public", "message": "This mod isn't public yet."}},
                    status=404,
                )
            if (sub == "" or sub is None) and slug == "ghost-mod":
                return self._send_json(
                    {"error": {"code": "not_found", "message": "Mod project not found"}},
                    status=404,
                )
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

        # Raw file download (tokenless in prod). Images point their <img> here;
        # the hex viewer fetches these bytes. Serve a real PNG for image paths and
        # a small deterministic blob for everything else so both previews render.
        if path.startswith("/v1/updates/") and path.endswith("/file"):
            p = parse_qs(url.query).get("path", [""])[0]
            ext = p.rsplit(".", 1)[-1].lower() if "." in p.rsplit("/", 1)[-1] else ""
            if ext in ("png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"):
                fav = STATIC / "assets" / "favicon.png"
                if fav.exists():
                    return self._send_file(fav, "application/octet-stream")
            if ext == "dds":
                return self._send_bytes(_stub_dds(), "application/octet-stream")
            blob = b"KIWI\x00\x01\x02\x03blueprint stub bytes for " + p.encode() + b"\n"
            blob += bytes(range(256)) * 2   # a full byte range so the hex gutter is lively
            return self._send_bytes(blob, "application/octet-stream")

        # ── /updates archive browser (synthetic tree) ─────────────────────
        if path == "/site/updates/branches":
            return self._send_json({"items": [
                {"branch": "live-us", "current_version": "1.0.stub",
                 "current_ordinal": 2, "last_probe_at": None,
                 "status": "idle", "file_count": len(_UPDATE_PATHS)}]})
        if path.startswith("/site/updates/"):
            rest = path[len("/site/updates/"):]
            branch, _, sub = rest.partition("/")
            qs = parse_qs(url.query)
            if sub == "versions":
                return self._send_json({"items": [
                    {"branch": branch, "ordinal": 2, "version_tag": "1.0.stub",
                     "captured_at": None, "completed_at": None,
                     "files_added": 3, "files_modified": 1, "files_removed": 0,
                     "bytes_added": 4096},
                    {"branch": branch, "ordinal": 1, "version_tag": "0.9.stub",
                     "captured_at": None, "completed_at": None,
                     "files_added": 6, "files_modified": 0, "files_removed": 0,
                     "bytes_added": 8192}], "count": 2, "total": 2})
            if sub == "tree":
                prefix = qs.get("prefix", [""])[0] or ""
                if prefix and not prefix.endswith("/"):
                    prefix += "/"
                entries = _update_dir_listing(prefix)
                return self._send_json({"branch": branch, "prefix": prefix,
                                        "entries": entries, "count": len(entries)})
            if sub == "changes":
                # A nested change-set so the folder-tree view is exercisable.
                ch = [
                    ("prefabs/dungeons/cursed_vale/boss.blueprint", "modified"),
                    ("prefabs/dungeons/cursed_vale/minion.blueprint", "added"),
                    ("prefabs/dungeons/frozen_peak/boss.blueprint", "modified"),
                    ("prefabs/equipment/sword_epic.blueprint", "added"),
                    ("prefabs/plant/oak_tree.blueprint", "modified"),
                    ("prefabs/plant/red_flower.blueprint", "added"),
                    ("prefabs/plant/fungi/glow_cap.blueprint", "modified"),
                    ("textures/ui/health_bar.dds", "modified"),
                    ("textures/blocks/stone_diffuse.dds", "added"),
                    ("ui/hud/health_bar.png", "modified"),
                    ("ui/hud/mana_bar.png", "removed"),
                    ("scripts/combat/damage.lua", "modified"),
                    ("scripts/combat/healing.lua", "added"),
                    ("languages/en/strings.json", "modified"),
                ]
                entries = [{"path": p, "type": ty, "content_sha256": "stub", "size": 512}
                           for p, ty in ch]
                counts = {"added": sum(1 for _, t in ch if t == "added"),
                          "modified": sum(1 for _, t in ch if t == "modified"),
                          "removed": sum(1 for _, t in ch if t == "removed")}
                return self._send_json({"branch": branch, "ordinal": 2,
                    "version_tag": "1.0.stub", "entries": entries,
                    "count": len(entries), "total": len(entries),
                    "files_added": counts["added"], "files_modified": counts["modified"],
                    "files_removed": counts["removed"]})
            if sub == "file/blueprint":
                # A small synthetic voxel model (a hollow-ish 6x6x6 block with a
                # glowing core) so the 3D viewer can be previewed without a real .tmod.
                xs = []; ys = []; zs = []; rgb = []; kind = []; level = []
                for x in range(6):
                    for y in range(6):
                        for z in range(6):
                            shell = x in (0, 5) or y in (0, 5) or z in (0, 5)
                            core = 2 <= x <= 3 and 2 <= y <= 3 and 2 <= z <= 3
                            if not (shell or core):
                                continue
                            xs.append(x); ys.append(y); zs.append(z)
                            rgb.append(0x66ccff if core else (0x3a4a5a if shell else 0))
                            kind.append(2 if core else 0)  # glow core, solid shell
                            level.append(255)
                return self._send_json({"path": qs.get("path", [""])[0], "count": len(xs),
                    "size": [6, 6, 6], "x": xs, "y": ys, "z": zs,
                    "rgb": rgb, "kind": kind, "level": level})
            if sub == "search":
                q = (qs.get("q", [""])[0] or "").strip().lower()
                hits = sorted(p for p in _UPDATE_PATHS if q in p.lower())
                entries = [{"path": p, "name": p.rsplit("/", 1)[-1],
                            "size": 512, "is_dir": False,
                            "last_ordinal": _last_ordinal_for(p),
                            "last_modified_at": _UPDATE_VERSION_DATES.get(_last_ordinal_for(p))}
                           for p in hits]
                return self._send_json({"branch": branch, "query": q,
                    "entries": entries, "count": len(entries), "total": len(entries)})
            if sub == "file/meta":
                p = qs.get("path", [""])[0]
                return self._send_json({"branch": branch, "path": p,
                    "content_sha256": "stub", "size": 512,
                    "archive": None, "archive_index": None})
            if sub == "file/view":
                p = qs.get("path", [""])[0]
                ext = p.rsplit(".", 1)[-1].lower() if "." in p.rsplit("/", 1)[-1] else ""
                base = {"branch": branch, "path": p, "content_sha256": "stub",
                        "truncated": False, "text": None}
                if ext in ("png", "jpg", "jpeg", "gif", "webp", "bmp", "ico"):
                    return self._send_json({**base, "size": 4096, "viewable": False,
                                            "kind": "image", "reason": "image"})
                if ext == "dds":
                    return self._send_json({**base, "size": 4096, "viewable": False,
                                            "kind": "dds", "reason": "dds"})
                if ext == "blueprint":
                    return self._send_json({**base, "size": 2048, "viewable": False,
                                            "kind": "blueprint", "reason": "blueprint"})
                if ext in ("binfab", "tfa", "tex"):
                    return self._send_json({**base, "size": 512, "viewable": False,
                                            "kind": "binary", "reason": "binary"})
                return self._send_json({**base, "size": 42, "viewable": True,
                    "kind": "text", "reason": None,
                    "text": f"-- stub contents of {p}\nprint('hello')\n"})
            if sub == "file/compare":
                # Synthetic diff so the compare-tab renderer (intra-line
                # highlighting + shareable from/to restore) is exercisable
                # without a real CAS. Mimics app/trove/updates/compare.make_hunks.
                p = qs.get("path", [""])[0]
                frm = int(qs.get("from", ["1"])[0])
                to = int(qs.get("to", ["2"])[0])
                long_a = ("id=42;name=\"Frostbite Blade\";dmg=100;"
                          "tags=[ice,epic,weapon];level=15;value=2500")
                long_b = ("id=42;name=\"Frostbite Blade\";dmg=140;"
                          "tags=[ice,legendary,weapon];level=15;value=2500")
                hunks = [{
                    "left_start": 1, "right_start": 1,
                    "lines": [
                        {"kind": "equal", "left": 1, "right": 1, "text": "-- header block (unchanged)"},
                        {"kind": "remove", "left": 2, "right": None, "text": long_a},
                        {"kind": "add", "left": None, "right": 2, "text": long_b},
                        {"kind": "equal", "left": 3, "right": 3, "text": "rarity=common"},
                        {"kind": "remove", "left": 4, "right": None, "text": "cooldown=5"},
                        {"kind": "add", "left": None, "right": 4, "text": "cooldown=3"},
                        {"kind": "add", "left": None, "right": 5, "text": "-- new trailing line (pure insert)"},
                        {"kind": "equal", "left": 5, "right": 6, "text": "-- footer block (unchanged)"},
                    ],
                }]
                return self._send_json({
                    "branch": branch, "path": p,
                    "from": {"ordinal": frm, "version_tag": f"0.{frm}.stub",
                             "content_sha256": "stubaaaa", "size": 512, "captured_at": None},
                    "to": {"ordinal": to, "version_tag": f"1.{to}.stub",
                           "content_sha256": "stubbbbb", "size": 520, "captured_at": None},
                    "identical": False, "is_text": True, "hunks": hunks})
            if sub == "file/history":
                p = qs.get("path", [""])[0]
                return self._send_json({"branch": branch, "path": p, "items": [
                    {"ordinal": 2, "version_tag": "1.0.stub", "captured_at": None,
                     "type": "modified", "content_sha256": "stub", "size": 512}],
                    "count": 1})
            return self._send_json({"items": []})

        # ── /market Analytics tab ─────────────────────────────────────────
        if path == "/site/market/items":
            return self._send_json({"items": _MARKET_ITEMS, "count": len(_MARKET_ITEMS),
                                    "categories": _MARKET_CATEGORIES,
                                    "untracked": _MARKET_UNTRACKED})
        if path == "/site/market/item-images":
            # In prod these map to real codex blueprints; locally we hand back a
            # stub path per item so the thumbnail layout is previewable (the
            # /site/codexes/render stub below serves a placeholder PNG for any).
            imgs = {n: f"stub/{n.lower().replace(' ', '_')}" for n in _MARKET_ITEMS}
            return self._send_json({"images": imgs, "branch": "live-us", "count": len(imgs)})
        if path == "/site/market/analytics/movers":
            return self._send_json(_market_movers())
        if path == "/site/market/analytics/deals":
            return self._send_json(_market_deals())
        if path in ("/site/market/analytics/overview", "/site/market/analytics/liquidity",
                    "/site/market/analytics/volume"):
            qs = parse_qs(url.query)
            try:
                days = int(qs.get("days", ["14"])[0])
            except ValueError:
                days = 14
            if path.endswith("/overview"):
                return self._send_json(_market_overview(days))
            if path.endswith("/liquidity"):
                return self._send_json(_market_liquidity(days))
            return self._send_json(_market_volume(days))
        if path == "/site/market/analytics/timeline":
            qs = parse_qs(url.query)
            name = qs.get("name", ["Item"])[0]
            try:
                days = int(qs.get("days", ["14"])[0])
            except ValueError:
                days = 14
            return self._send_json(_market_timeline(name, days))

        # ── /codexes/crafting Recipe Cost Calculator ──────────────────────
        if path == "/site/codexes/render":
            # Blueprint→PNG render (real rasterizer in prod). Serve a placeholder
            # so codex/market thumbnails have something to show in local preview.
            return self._send_bytes(_PLACEHOLDER_PNG, "image/png")
        if path == "/site/codexes/search":
            qs = parse_qs(url.query)
            q = (qs.get("q", [""])[0] or "").strip().lower()
            items = [r for r in _CRAFT_SEARCH if q in r["name"].lower()] if q else list(_CRAFT_SEARCH)
            return self._send_json({"branch": "live-us", "type": "recipe", "query": q,
                                    "items": items, "count": len(items), "total": len(items)})
        if path == "/site/codexes/crafting":
            qs = parse_qs(url.query)
            tree = _craft_build(qs.get("path", [""])[0])
            if tree is None:
                return self._send_json({"detail": "No such recipe"}, status=404)
            return self._send_json(tree)

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
        if path == "/site/stats/classes":
            # Serve the real gamedata classes.json (mapped to the cleaned shape:
            # qualified_name -> tech_name) so the /classes page renders end-to-end.
            cj = ROOT / "app" / "trove" / "gamedata" / "classes.json"
            items = []
            if cj.exists():
                for c in json.loads(cj.read_text(encoding="utf-8")):
                    items.append({
                        "tech_name": c.get("qualified_name"), "name": c.get("name"),
                        "shorts": c.get("shorts", []), "damage_type": c.get("damage_type", ""),
                        "weapons": c.get("weapons", []), "attributes": c.get("attributes", []),
                        "stats": c.get("stats", []), "bonuses": c.get("bonuses", []),
                        "subclass": c.get("subclass", {}), "abilities": c.get("abilities", []),
                    })
            return self._send_json({"items": items, "count": len(items)})
        if path == "/site/rotations":
            # "Today in Trove" - resets + buffs + chaos + live merchants/biomes.
            import time as _t
            from datetime import datetime, timezone, timedelta
            now = int(_t.time())
            nowdt = datetime.now(timezone.utc)
            day_reset = nowdt.replace(hour=11, minute=0, second=0, microsecond=0)
            if nowdt >= day_reset:
                day_reset += timedelta(days=1)
            wk = day_reset
            while wk.weekday() != 6:
                wk += timedelta(days=1)
            daily_reset = int(day_reset.timestamp())
            weekly_reset = int(wk.timestamp())
            def _biome(n, icon="permafrost"):
                return {"name": n, "icon": icon}
            _days = ["Sunny Sunday", "Mining Monday", "Trove Tuesday", "Watery Wednesday",
                     "Thorny Thursday", "Fried Friday", "Shadow Saturday"]
            daily_rot = [{"name": _days[i], "emoji": "🌞", "color": "#ffb347",
                          "weekday": i, "normal_buffs": ["+50% Magic Find"],
                          "premium_buffs": ["+100% Magic Find"], "banner": None,
                          "is_current": i == (nowdt.weekday() + 1) % 7,
                          "next_at": daily_reset + (i - 1) * 86400} for i in range(7)]
            weekly_rot = [{"name": f"Week {i+1} Bonus", "emoji": "✨", "color": "#7cc7ff",
                           "buffs": ["+25% Class Gem XP"], "banner": None,
                           "is_current": i == 0, "next_at": weekly_reset + (i - 1) * 7 * 86400}
                          for i in range(4)]
            return self._send_json({
                "server_time": {"now_unix": now, "now_iso": nowdt.isoformat(),
                                "trove_day": nowdt.strftime("%A"),
                                "daily_reset_at": daily_reset, "weekly_reset_at": weekly_reset},
                "daily_buff": {"name": daily_rot[0]["name"], "emoji": "🌞",
                               "normal_buffs": ["+50% Magic Find"],
                               "premium_buffs": ["+100% Magic Find"]},
                "weekly_buff": {"name": "Week 1 Bonus", "emoji": "✨", "buffs": ["+25% Class Gem XP"]},
                "daily_rotation": daily_rot, "weekly_rotation": weekly_rot,
                "chaos": {"starts_at": now - 3 * 86400, "ends_at": now + 4 * 86400,
                          "seconds_remaining": 4 * 86400,
                          "item": {"name": "Diamond Dragon Egg", "identifier": "item/diamond"}},
                "merchants": [
                    {"id": "corruxion", "name": "Corruxion", "active": True,
                     "starts_at": now - 1800, "ends_at": now + 5400,
                     "schedule": [{"starts_at": now + 5400, "ends_at": now + 12600}]},
                    {"id": "fluxion", "name": "Fluxion", "active": False, "state": "away",
                     "starts_at": now + 3600, "ends_at": now + 10800,
                     "schedule": [{"starts_at": now + 3600, "ends_at": now + 10800, "state": "arriving"}]},
                    {"id": "wild_mana", "name": "Wild Mana", "active": True,
                     "starts_at": now - 3600, "ends_at": now + 7200,
                     "biomes": [_biome("Permafrost"), _biome("Cursed Vale", "cursedvale")],
                     "schedule": [{"starts_at": now + 7200, "ends_at": now + 18000,
                                   "biomes": [_biome("Fae Forest", "faeforest")]}]},
                    {"id": "d15", "name": "Long Shade Rotation", "active": True,
                     "starts_at": now - 5400, "ends_at": now + 5400,
                     "biomes": [_biome("Neon City", "neoncity")],
                     "schedule": [{"starts_at": now + 5400, "ends_at": now + 16200,
                                   "biomes": [_biome("Jurassic Jungle", "jurassicjungle")]}]},
                    {"id": "stampy", "name": "Stampy", "active": True,
                     "starts_at": now - 7200, "ends_at": now + 100800,
                     "biomes": [_biome("Candoria", "candoria")],
                     "schedule": []},
                ],
            })
        if path == "/site/calendar/events":
            import time as _t
            now = int(_t.time())
            def _ev(i, name, cat, off_s, off_e, icon):
                return {"event_id": str(i), "name": name, "url": "https://trovesaurus.com/events",
                        "category": cat, "image": None, "icon": icon,
                        "starts_at": now + off_s, "ends_at": now + off_e,
                        "status": "ongoing" if off_s <= 0 < off_e else "upcoming",
                        "seconds_until": (off_e if off_s <= 0 else off_s)}
            return self._send_json({
                "now": now,
                "ongoing": [
                    _ev(1, "Shadow's Eve", "Seasonal", -2 * 86400, 5 * 86400,
                        "https://trovesaurus.com/images/events/shadowseve.png"),
                    _ev(2, "Double XP Weekend", "Bonus", -86400, 2 * 86400, None),
                ],
                "upcoming": [
                    _ev(3, "Sunfest", "Seasonal", 3 * 86400, 12 * 86400, None),
                    _ev(4, "Bacon Bonanza", "Bonus", 7 * 86400, 9 * 86400, None),
                ],
            })
        if path == "/site/calendar/yearly":
            # Synthetic ±365-day rotation timeline matching the real
            # app.trove.calendar.yearly_calendar() shape, so the homepage
            # widget renders end-to-end without the full backend.
            import time as _t
            from datetime import datetime, timezone, timedelta
            UTC = timezone.utc
            now = datetime.now(UTC)
            ws = now - timedelta(days=365)
            we = now + timedelta(days=365)

            def _ev(type_, name, s, e, **extra):
                d = {"type": type_, "name": name,
                     "starts_at": int(s.timestamp()), "ends_at": int(e.timestamp())}
                d.update(extra)
                return d

            def _cycle(base, interval, dur, emit):
                total = interval.total_seconds()
                k = int((ws - base).total_seconds() // total) - 1
                out = []
                while True:
                    s = base + timedelta(seconds=k * total)
                    if s >= we:
                        break
                    e = s + dur
                    if e > ws and s < we:
                        out.append((k, s, e))
                    k += 1
                return out

            evs = []
            wk = timedelta(days=7)
            _buffs = [("Weekly: Class Gem XP", "fbc02d"), ("Weekly: Loot", "7cc7ff"),
                      ("Weekly: Adventure", "a371f7"), ("Weekly: Faction", "3fb950")]
            b_base = datetime(2024, 1, 1, 11, 0, tzinfo=UTC)
            for k, s, e in _cycle(b_base, wk, wk, None):
                nm, col = _buffs[k % 4]
                evs.append(_ev("weekly_buff", nm, s, e, color=col))
            c_base = datetime(2023, 12, 8, 11, 0, tzinfo=UTC)
            for _k, s, e in _cycle(c_base, timedelta(days=14), timedelta(days=3), None):
                evs.append(_ev("corruxion", "Corruxion", s, e))
            f_base = datetime(2023, 12, 5, 11, 0, tzinfo=UTC)
            for _k, s, _e in _cycle(f_base, timedelta(days=14), timedelta(days=3), None):
                evs.append(_ev("fluxion", "Fluxion (Voting)", s, s + timedelta(days=3), state="voting", color="5ca8cc"))
                sell = s + timedelta(days=7)
                evs.append(_ev("fluxion", "Fluxion (Selling)", sell, sell + timedelta(days=3), state="selling", color="02679e"))
            g_base = datetime(2025, 5, 23, 11, 0, tzinfo=UTC)
            for _k, s, _e in _cycle(g_base, timedelta(days=2), timedelta(days=2), None):
                evs.append(_ev("gardening_2", "2-day plants", s + timedelta(days=1), s + timedelta(days=2), color="8bc34a"))
            for _k, s, _e in _cycle(g_base, timedelta(days=3), timedelta(days=3), None):
                evs.append(_ev("gardening_3", "3-day plants", s + timedelta(days=2), s + timedelta(days=3), color="4caf50"))
            _mana_b = [("Permafrost", "permafrost"), ("Cursed Vale", "cursedvale"),
                       ("Neon City", "neoncity"), ("Fae Forest", "faeforest")]
            m_base = datetime(2023, 11, 20, 11, 0, tzinfo=UTC)
            for k, s, e in _cycle(m_base, wk, wk, None):
                bs = [{"name": _mana_b[(k - i) % 4][0], "icon": _mana_b[(k - i) % 4][1]} for i in range(3)]
                evs.append(_ev("mana", "Wild Mana", s, e, biomes=bs))
            _stampy_b = [("Jurassic Jungle", "jurassicjungle"), ("Candoria", "candoria"),
                         ("Neon City", "neoncity"), ("Permafrost", "permafrost")]
            s_base = datetime(2023, 9, 30, 11, 0, tzinfo=UTC)
            for k, s, _e in _cycle(s_base, wk, timedelta(hours=48), None):
                nm, ic = _stampy_b[k % 4]
                evs.append(_ev("stampy", "Stampy", s, s + timedelta(hours=48), biomes=[{"name": nm, "icon": ic}]))

            evs.sort(key=lambda x: (x["starts_at"], x["type"]))
            return self._send_json({
                "starts_at": int(ws.timestamp()), "ends_at": int(we.timestamp()),
                "generated_at": int(now.timestamp()), "count": len(evs), "events": evs,
            })
        if path == "/site/feeds/videos":
            import time as _t
            plat = (parse_qs(url.query).get("platform", ["youtube"])[0])
            if plat == "twitch":
                items = [
                    {"channel": "TroveStreamer", "login": "trovestreamer", "title": "Geode grinding all night!",
                     "viewers": 342, "thumbnail": "/static/assets/favicon.png", "game": "Trove",
                     "url": "https://twitch.tv/example", "started_at": None},
                    {"channel": "PixelPaladin", "login": "pixelpaladin", "title": "Chill delving + chat",
                     "viewers": 88, "thumbnail": "/static/assets/favicon.png", "game": "Trove",
                     "url": "https://twitch.tv/example2", "started_at": None},
                ]
            else:
                items = [
                    {"title": "Trove 2026 Beginner Guide", "channel": "TroveTips",
                     "thumbnail_url": "/static/assets/favicon.png", "video_id": "x",
                     "url": "https://youtube.com/watch?v=x", "published_at": None},
                    {"title": "Top 10 Mounts You Missed", "channel": "MountHunter",
                     "thumbnail_url": "/static/assets/favicon.png", "video_id": "y",
                     "url": "https://youtube.com/watch?v=y", "published_at": None},
                ]
            return self._send_json({"platform": plat, "items": items, "fetched_at": None})
        if path == "/site/feeds/news":
            return self._send_json({"items": [
                {"title": "Sunfest Returns!", "url": "https://trovegame.com/news/sunfest",
                 "author": "Trove Team", "summary": "The sunniest event of the year is back with new rewards.",
                 "category": "Events", "categories": ["Events"],
                 "image": "/static/assets/favicon.png", "published_at": None},
                {"title": "Patch Notes 12.3", "url": "https://trovegame.com/news/patch",
                 "author": "Trove Team", "summary": "Balance changes, bug fixes and a new mount.",
                 "category": "Patch", "categories": ["Patch"], "image": None, "published_at": None},
            ]})
        if path == "/site/btt/latest":
            import time as _t
            from datetime import datetime, timezone
            pub = datetime.now(timezone.utc).isoformat()
            def _asset(name, size, dl):
                return {"name": name, "url": "https://github.com/example/releases/download/v2.4.1/" + name,
                        "size": size, "content_type": "application/octet-stream", "download_count": dl}
            return self._send_json({"channel": "release", "platforms": {
                "windows": {"platform": "windows", "tag_name": "v2.4.1", "published_at": pub,
                            "html_url": "https://github.com/example/releases/tag/v2.4.1",
                            "assets": [_asset("BetterTroveTools_2.4.1_x64.msi", 48210000, 12840),
                                       _asset("BetterTroveTools_2.4.1_x64.exe", 47110000, 3120)]},
                "linux": {"platform": "linux", "tag_name": "v2.4.1", "published_at": pub,
                          "html_url": "https://github.com/example/releases/tag/v2.4.1",
                          "assets": [_asset("BetterTroveTools_2.4.1_amd64.AppImage", 62000000, 1840)]},
                "android": None,
            }})
        if path == "/site/btt/releases":
            import time as _t
            from datetime import datetime, timezone, timedelta
            base = datetime.now(timezone.utc)
            def _rel(i, tag, name, pre, body):
                return {"tag_name": tag, "name": name, "body": body,
                        "html_url": f"https://github.com/example/releases/tag/{tag}",
                        "channel": "beta" if pre else "release", "prerelease": pre,
                        "published_at": (base - timedelta(days=i * 6)).isoformat(),
                        "assets": [{"name": f"BetterTroveTools_{tag[1:]}_x64.msi", "url": "https://github.com/example/x.msi",
                                    "size": 48000000, "content_type": "application/octet-stream", "download_count": 1200 - i * 100}]}
            items = [
                _rel(0, "v2.4.1", "Bug fixes", False, "## What's changed\n\n- Fixed a crash on startup\n- Faster mod loading"),
                _rel(1, "v2.4.0", "Mods Hub integration", False, "### Highlights\n\n- Browse the Mods Hub in-app\n- New settings panel"),
                _rel(2, "v2.4.0-beta.2", "Beta 2", True, "Testing the new updater."),
                _rel(3, "v2.3.5", "Maintenance", False, "Small fixes and dependency bumps."),
            ]
            channel = parse_qs(url.query).get("channel", [None])[0]
            if channel in ("release", "beta"):
                items = [r for r in items if r["channel"] == channel]
            return self._send_json({"channel": channel, "items": items,
                                    "count": len(items), "total": len(items)})
        if path == "/site/btt/changelog":
            def _c(sha, msg, typ):
                return {"sha": sha, "short_sha": sha[:7], "message": msg, "type": typ,
                        "url": f"https://github.com/example/commit/{sha}"}
            return self._send_json({"rate_limited": False, "fetched_at": None, "groups": [
                {"version": "Unreleased", "commits": [
                    _c("aaaaaaa1", "feat: add dark theme toggle", "feat"),
                    _c("aaaaaaa2", "chore: bump deps", "chore")]},
                {"version": "v2.4.1", "commits": [
                    _c("bbbbbbb1", "fix: crash on startup with empty config", "fix"),
                    _c("bbbbbbb2", "perf: lazy-load the mod list", "perf")]},
                {"version": "v2.4.0", "commits": [
                    _c("ccccccc1", "feat: mods hub browser", "feat"),
                    _c("ccccccc2", "docs: update README", "docs"),
                    _c("ccccccc3", "refactor: settings store", "refactor")]},
            ]})
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
        if path == "/site/leaderboards/records":
            # Record-high widgets on the homepage. Mastery points chosen so the
            # level curve resolves to believable in-game values; Geode's points
            # push it past the level-100 soft cap so the "would be N" note shows.
            return self._send_json({
                "trove_mastery": {
                    "points": 167291, "level": 801, "points_into_level": 40,
                    "points_to_next_level": 361, "player_name": "Aallyn",
                    "anchor": STUB_ANCHOR,
                },
                "geode_mastery": {
                    "points": 13450, "level": 100, "points_into_level": 50,
                    "points_to_next_level": 50, "player_name": "Skill",
                    "anchor": STUB_ANCHOR, "level_cap": 100,
                    "uncapped_level": 143, "capped": True,
                },
                "power_rank": {
                    "value": 41230, "board_uuid": 1011, "player_name": "Bae",
                    "anchor": STUB_ANCHOR,
                },
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
                "effort_threshold": 50, "xp_threshold": 2_000_000,
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
                "power_rank_threshold": 25000,
                "effort_threshold": 50, "xp_threshold": 2_000_000, "classes": classes,
                "methodology": "stub class current", "computed_at": STUB_ANCHOR,
            })
        if path == "/site/leaderboards/renames" or path.startswith("/site/leaderboards/renames?"):
            # Synthetic detected renames spanning the confidence range so the
            # Possible-renames tab + its slider have something to show locally.
            def _rename(rid, frm, to, to_anchor, conf, boards):
                return {
                    "id": rid, "from_name": frm, "to_name": to,
                    "from_anchor": to_anchor - 3600, "to_anchor": to_anchor,
                    "confidence": conf, "matched_boards": len(boards),
                    "method_version": 1, "created_at": to_anchor,
                    "evidence": {
                        "gap_seconds": 3600,
                        "boards": boards,
                        "terms": {"matched_boards": len(boards), "board_term": 0.875,
                                  "tightness": 1.0, "exclusivity": 1.0, "rarity": 1.0,
                                  "mean_drift_pct": 0.0, "confidence": conf},
                        "summary": (f"“{frm}” vanished and “{to}” appeared in the "
                                    f"same capture with the same lifetime score fingerprint across "
                                    f"{len(boards)} boards. Mutual, unambiguous best match."),
                    },
                }
            renames = [
                _rename(3, "DragonSlayer", "NightfallX", STUB_ANCHOR, 0.91, [
                    {"uuid": 1, "name": "Trove Mastery", "score_from": 812340, "score_to": 812340, "drift_pct": 0.0},
                    {"uuid": 20, "name": "Geode Mastery", "score_from": 45120, "score_to": 45180, "drift_pct": 0.133},
                    {"uuid": 1000, "name": "Power Rank (Knight)", "score_from": 29800, "score_to": 29800, "drift_pct": 0.0},
                ]),
                _rename(2, "oldmage42", "Arcanist", STUB_ANCHOR - 7200, 0.75, [
                    {"uuid": 1, "name": "Trove Mastery", "score_from": 511200, "score_to": 511200, "drift_pct": 0.0},
                    {"uuid": 20, "name": "Geode Mastery", "score_from": 30100, "score_to": 30240, "drift_pct": 0.465},
                ]),
                _rename(1, "kiwibot", "kiwifruit", STUB_ANCHOR - 90000, 0.62, [
                    {"uuid": 1, "name": "Trove Mastery", "score_from": 220000, "score_to": 221000, "drift_pct": 0.454},
                    {"uuid": 1000, "name": "Power Rank (Gunslinger)", "score_from": 15000, "score_to": 15000, "drift_pct": 0.0},
                ]),
            ]
            return self._send_json({
                "enabled": True, "renames": renames, "total": len(renames),
                "limit": 200, "offset": 0, "method_version": 1,
            })
        if path.startswith("/site/leaderboards/renames/"):
            # Per-name rename chain. Returns a 2-hop history for the demo alias
            # family, empty for everyone else (the common case).
            qname = unquote(path[len("/site/leaderboards/renames/"):])
            chain = ["xXProGamerXx", "1337Hacker", "1337Hackerdu77"]
            if qname.lower() in {n.lower() for n in chain}:
                edges = [
                    {"id": 11, "from_name": chain[0], "to_name": chain[1],
                     "from_anchor": STUB_ANCHOR - 90000, "to_anchor": STUB_ANCHOR - 86400,
                     "confidence": 0.88, "matched_boards": 4, "method_version": 1,
                     "created_at": STUB_ANCHOR - 86400, "evidence": {}},
                    {"id": 12, "from_name": chain[1], "to_name": chain[2],
                     "from_anchor": STUB_ANCHOR - 3600, "to_anchor": STUB_ANCHOR,
                     "confidence": 0.93, "matched_boards": 5, "method_version": 1,
                     "created_at": STUB_ANCHOR, "evidence": {}},
                ]
                return self._send_json({
                    "query": qname, "aliases": sorted(set(chain)),
                    "current_name": chain[-1], "edges": edges,
                    "rename_count": len(edges),
                })
            return self._send_json({
                "query": qname, "aliases": [], "current_name": qname,
                "edges": [], "rename_count": 0,
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
        if path == "/site/leaderboards/days":
            # Archive date-picker: ~30 daily anchors (latest capture ~11:52 UTC per
            # trove-day), newest first - what the signed-in date-jump reads.
            days = [(_today_trove_key - i) * _DAY + _TROVE_OFFSET + 3120 for i in range(30)]
            return self._send_json({"items": days, "count": len(days)})
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
        if path.startswith("/site/leaderboards/players/") and path.endswith("/profile"):
            queried = unquote(path.split("/")[4])
            # Boards spanning every category so the /player page's category
            # grouping + icon mapping (incl. an unmapped Bomber board) render.
            cat_boards = [
                (1, "TROVE MASTERY POINTS", "META"),
                (20, "GEODE MASTERY POINTS", "META"),
                (100, "TOTAL MASTERY POINTS", "META"),
                (1000, "KNIGHT", "POWER RANK"),
                (1012, "REVENANT", "POWER RANK"),
                (1100, "CLUB POWER RANK", "POWER RANK"),
                (4000, "KNIGHT", "EFFORT"),
                (4007, "SHADOW HUNTER", "EFFORT"),
                (5000, "Weekly Highest Paragon Level with Knight", "PARAGON"),
                (50000, "Weekly Highest Paragon Level", "PARAGON"),
                (3, "ENEMIES DEFEATED", "STATS"),
                (33001, "HART-A-PHONES RECEIVED", "STATS"),
                (30002, "WEEKLY BOMBER ROYALE GAMES WON", "STATS"),  # unmapped -> no icon
                (2004, "CHALLENGE: Deepest (WEEKLY)", "DELVES"),
                (2021, "Deepest Diggers of PUBLIC (RECENT)", "DELVES"),
            ]
            boards = []
            for i, (uuid, nm, cat) in enumerate(cat_boards):
                boards.append({
                    "leaderboard": uuid, "board_name": nm, "category": cat,
                    "best_rank": (i % 5) + 1, "latest_rank": (i % 7) + 1,
                    "latest_score": 59736.0 - i * 850, "appearances": 749 - i,
                    "first_seen": STUB_TIMESTAMPS[-1], "last_seen": STUB_ANCHOR,
                })
            # Rename chain + alt-cluster membership so the profile's username
            # history + "Possible alt accounts" sections render locally.
            rn_chain = ["xXProGamerXx", "1337Hacker", queried]
            renames_out = {
                "aliases": sorted(set(rn_chain)), "current_name": queried, "count": 2,
                "edges": [
                    {"id": 11, "from_name": rn_chain[0], "to_name": rn_chain[1],
                     "from_anchor": STUB_ANCHOR - 90000, "to_anchor": STUB_ANCHOR - 86400,
                     "confidence": 0.88, "matched_boards": 4, "evidence": {}},
                    {"id": 12, "from_name": rn_chain[1], "to_name": queried,
                     "from_anchor": STUB_ANCHOR - 3600, "to_anchor": STUB_ANCHOR,
                     "confidence": 0.93, "matched_boards": 5, "evidence": {}},
                ],
            }
            alt_clusters = [{
                "stem": "1337hacker", "label": "1337hacker*", "method": "both",
                "confidence": 0.87, "member_count": 4, "members_truncated": 0,
                "members": [queried, "1337HackerAlt", "1337Hackerbtw", "l337Hacker"],
                "board_count": 3, "corroborated_by": ["name_stem", "co_movement"],
                "summary": ("Four near-identically named accounts moved in lockstep "
                            "across 3 boards with matching hourly gains."),
                "boards": [],
            }]
            return self._send_json({
                "player_name": queried, "verified": True,
                "summary": {
                    "boards_appeared": len(boards), "appearances": 200,
                    "best_rank": 1, "best_rank_board_uuid": 1000,
                    "best_rank_board_name": "KNIGHT", "top10_count": 6,
                    "top100_count": 7, "latest_anchor": STUB_ANCHOR,
                    # Last played earlier than last seen: on lifetime boards they
                    # appear every capture, but their score last rose a day ago.
                    "last_played": STUB_ANCHOR - 86400,
                },
                "boards": boards, "recent": [],
                "renames": renames_out, "alt_clusters": alt_clusters,
            })

        if path.startswith("/site/leaderboards/players/") and path.endswith("/history"):
            queried = unquote(path.split("/")[4])
            # Mirror the prod case-insensitive match (service.py uses
            # an anchored ``$regex`` with ``i``). Return rows ONLY for
            # the small set of stub players, regardless of input casing.
            known = {e["player_name"].lower(): e["player_name"] for e in STUB_ENTRIES}
            # The rename-demo family also resolves to a prolific player so the
            # leaderboards panel shows tiles (crowns) alongside the alias banner.
            for _alias in ("xXProGamerXx", "1337Hacker", "1337Hackerdu77"):
                known[_alias.lower()] = "1337Hackerdu77"
            canonical = known.get(queried.lower())
            if canonical is None:
                return self._send_json({
                    "player_name": queried, "items": [], "count": 0,
                })
            # A prolific player lands on every board at each capture. Emit the
            # full board set at the latest anchor AND at the prior one (newest
            # first) so the client's latest-capture collapse + tile grid can be
            # exercised locally.
            prev_anchor = STUB_TIMESTAMPS[1] if len(STUB_TIMESTAMPS) > 1 else STUB_ANCHOR - _DAY
            items = []
            for anchor in (STUB_ANCHOR, prev_anchor):
                for i, b in enumerate(STUB_BOARDS):
                    items.append({
                        "player_name": canonical,
                        "rank": i + 1,
                        "score": 59731.0 - i * 1200 - (0 if anchor == STUB_ANCHOR else 300),
                        "leaderboard": b["uuid"],
                        "created_at": anchor,
                    })
            return self._send_json({
                "player_name": canonical,
                "items": items,
                "count": len(items),
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

    def do_POST(self):
        url = urlparse(self.path)
        path = url.path
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b"{}"
        try:
            body = json.loads(raw.decode("utf-8") or "{}")
        except (ValueError, UnicodeDecodeError):
            return self._send_json({"detail": "invalid JSON body"}, 400)

        if path == "/site/gems/evaluate":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            try:
                out = _gem_evaluator.evaluate_gem(
                    body.get("tier", 4), body.get("type", 1), body.get("level", 1),
                    body.get("stats", []), bool(body.get("auto_guess_procs", False)),
                )
            except _gem_evaluator.GemEvaluatorError as e:
                return self._send_json({"detail": str(e)}, 400)
            except (ValueError, KeyError, ZeroDivisionError) as e:
                return self._send_json({"detail": f"Could not evaluate gem: {e}"}, 400)
            payload = {**out["result"],
                       "available_extra_containers": out["available_extra_containers"],
                       "guessed_distribution": out["guessed_distribution"]}
            return self._send_json(payload)

        if path == "/site/gems/evaluate-simple":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            try:
                out = _gem_evaluator.evaluate_gem_simple(
                    body.get("tier", 4), body.get("type", 1),
                    body.get("power_rank", 0), body.get("level", 1),
                )
            except _gem_evaluator.GemEvaluatorError as e:
                return self._send_json({"detail": str(e)}, 400)
            except (ValueError, KeyError, ZeroDivisionError) as e:
                return self._send_json({"detail": f"Could not evaluate gem: {e}"}, 400)
            return self._send_json(out)

        if path == "/site/gems/builds/calculate":
            if not _GEMS_OK:
                return self._send_json({"detail": "gem tools unavailable"}, 503)
            try:
                results = _gem_builds.calculate_builds(body)
            except _gem_builds.BuildError as e:
                return self._send_json({"detail": str(e)}, 400)
            return self._send_json({"results": results, "count": len(results)})

        return self.send_error(404)

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
        # Minimal Jinja emulation for page templates: inline `{% include
        # "partials/x.html" %}`, drop `{# … #}` comments, and strip the remaining
        # statement/expression tags so partials (navbar, support widget, …) and
        # feature-gated sections render in the local preview instead of leaking
        # raw `{% if %}` / `{{ }}` text into the page. One level deep is enough
        # for our partials.
        if content_type == "text/html" and p.parent == TEMPLATES:
            import re as _re
            _strip_comments = lambda s: _re.sub(r"{#.*?#}", "", s, flags=_re.S)
            text = _strip_comments(data.decode("utf-8"))
            # Comments are stripped BEFORE inlining (and again on each partial as
            # it's read) so that documentation comments which happen to quote a
            # literal `{% include … %}` as an example don't get re-expanded into a
            # duplicate partial on the next pass.
            def _inline(m):
                f = TEMPLATES / m.group(1)
                return _strip_comments(f.read_text(encoding="utf-8")) if f.exists() else ""
            for _ in range(3):
                if "{% include" not in text:
                    break
                text = _re.sub(r'{%\s*include\s*"([^"]+)"\s*%}', _inline, text)
            # Drop every remaining `{% … %}` control tag (if/else/endif/set/for/…).
            # This keeps the body of every conditional, so all feature-gated
            # sections are visible in preview — exactly what we want when eyeballing
            # layout/responsive behaviour. `{{ … }}` expressions collapse to empty.
            text = _re.sub(r"{%.*?%}", "", text, flags=_re.S)
            text = _re.sub(r"{{.*?}}", "", text, flags=_re.S)
            data = text.encode("utf-8")
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(data)

    def _send_json(self, obj, status=200):
        data = json.dumps(obj).encode("utf-8")
        self.send_response(status)
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
    # Port comes from $PORT (set by the preview harness' autoPort) or argv[1],
    # falling back to 8913 for a plain manual run.
    import os as _os
    _port = int(_os.environ.get("PORT") or (_sys.argv[1] if len(_sys.argv) > 1 else 8913))
    print(f"[site-dev] http://localhost:{_port}/leaderboards")
    # ThreadingHTTPServer (not the single-threaded HTTPServer): the browser
    # preview opens several keep-alive connections at once, which would wedge a
    # single-threaded server mid-request. Each request is independent stub data,
    # so threading is safe here.
    ThreadingHTTPServer(("127.0.0.1", _port), Handler).serve_forever()
