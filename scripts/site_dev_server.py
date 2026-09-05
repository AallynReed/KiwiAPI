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
import os
import re
import urllib.error as _urlerr
import urllib.request as _urlreq
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

_RENDER_CACHE: dict[str, bytes] = {}


def _codex_render(path_qs: str) -> bytes:
    """Fetch a real blueprint render from the API, falling back to the placeholder.

    Cached per URL for the life of the process: a fish table asks for ~155 of
    these and the upstream render is not free.
    """
    if path_qs in _RENDER_CACHE:
        return _RENDER_CACHE[path_qs]
    png = _PLACEHOLDER_PNG
    try:
        with _urlreq.urlopen(_API_ORIGIN + path_qs, timeout=10) as resp:
            if resp.status == 200:
                png = resp.read()
    except Exception:  # noqa: BLE001 - dev preview: a miss is a placeholder, never a crash
        pass
    _RENDER_CACHE[path_qs] = png
    return png


ROOT = Path(__file__).resolve().parent.parent
SITE_DIR = ROOT / "site"
TEMPLATES = SITE_DIR / "templates"
STATIC = SITE_DIR / "static"


# ── page-template rendering ────────────────────────────────────────────────
# Real Jinja when it's importable (it is, inside the project venv), so the
# preview matches what FastAPI serves: `{% if %}` branches resolve properly and
# the server-rendered-first-paint blocks take their no-data fallback - which is
# exactly the state a browser sees before the page JS runs. Undefined values
# render empty and chain safely, so no per-page context is needed.
#
# Falling back to the old regex emulation keeps the script runnable with a bare
# stdlib interpreter: inline the partials, drop comments, then strip the
# remaining tags. That path keeps BOTH branches of an `{% if %}`, so it shows a
# little more than the real render does - fine for eyeballing layout.
try:
    import jinja2 as _jinja2
except ImportError:
    _jinja2 = None

_JINJA_ENV = _jinja2 and _jinja2.Environment(
    loader=_jinja2.FileSystemLoader(str(TEMPLATES)),
    autoescape=True,
    undefined=_jinja2.ChainableUndefined,
)

# Every feature switched on, so no section is hidden in preview.
_PREVIEW_FLAGS = {
    "mods_hub_enabled", "market_enabled", "store_enabled", "leaderboards_enabled",
    "player_activity_enabled", "class_activity_enabled", "clubs_enabled",
    "updates_enabled", "codexes_enabled", "server_status_enabled",
    "giveaways_enabled", "commands_enabled", "server_time_enabled",
    "webhooks_enabled", "dm_subscriptions_enabled", "image_studio_enabled",
    "calendar_enabled", "streams_enabled", "btt_releases_enabled",
    "classes_enabled", "star_chart_enabled", "gem_simulator_enabled",
    "gem_evaluator_enabled", "gem_builds_enabled", "calculators_enabled",
    "gems_guide_enabled", "fishing_guide_enabled", "cheater_detection_enabled", "alt_clusters_enabled",
    "renames_enabled", "duplicates_enabled", "discord_oauth_enabled",
    "dressing_room_enabled", "dressing_room_page_enabled",
    "sound_studio_enabled", "mod_workshop_enabled",
}


class _PreviewRequest:
    """Just enough of a Starlette request for the navbar, which reads
    ``request.url.path`` to mark the active nav item."""

    def __init__(self, path: str):
        self.url = type("_U", (), {"path": path})()


def _render_page_template(p: Path, request_path: str = "/", extra: dict | None = None) -> bytes:
    """Render a page template to HTML bytes for the preview.

    ``extra`` supplies the per-page context a route passes in production (the embed
    page's source params, for instance) - without it those `{{ }}` render empty and
    the page can't tell what it was asked to show."""
    if _JINJA_ENV is not None:
        ctx = dict.fromkeys(_PREVIEW_FLAGS, True)
        ctx["request"] = _PreviewRequest(request_path)
        ctx.update(extra or {})
        return _JINJA_ENV.get_template(p.name).render(**ctx).encode("utf-8")

    import re as _re
    def _strip_comments(s: str) -> str:
        return _re.sub(r"{#.*?#}", "", s, flags=_re.S)
    # Comments are stripped BEFORE inlining (and again on each partial as it's
    # read) so a documentation comment quoting a literal `{% include … %}` as an
    # example isn't re-expanded into a duplicate partial on the next pass.
    text = _strip_comments(p.read_text(encoding="utf-8"))

    def _inline(m):
        f = TEMPLATES / m.group(1)
        return _strip_comments(f.read_text(encoding="utf-8")) if f.exists() else ""

    for _ in range(3):
        if "{% include" not in text:
            break
        text = _re.sub(r'{%\s*include\s*"([^"]+)"\s*%}', _inline, text)
    text = _re.sub(r"{%.*?%}", "", text, flags=_re.S)
    text = _re.sub(r"{{.*?}}", "", text, flags=_re.S)
    return text.encode("utf-8")


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

# Serve the same CSP the edge does. Without it an inline <script> or `onclick=`
# works perfectly here and is silently refused in production - the one class of
# bug this server used to be blind to.
try:
    from app.core.csp import SITE_CSP as _SITE_CSP
except Exception as _e:  # noqa: BLE001 - dev-only, degrade gracefully
    print(f"[site-dev] CSP unavailable ({_e}); pages served without one")
    _SITE_CSP = ""

# Session endpoints are proxied to the real API rather than stubbed - see
# Handler._proxy_auth for why cookies make direct calls impossible from dev.
_AUTH_PREFIX = "/v1/site-auth"
_API_ORIGIN = os.environ.get("SITE_DEV_API", "https://api.aallyn.net").rstrip("/")
_SITE_ORIGIN = os.environ.get("SITE_DEV_ORIGIN", "https://trove.aallyn.net").rstrip("/")

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
    # Three class Effort boards (4000+i, named for the class) flagged as this
    # week's contests - what /gems-guide reads to name the current rotation.
    # Copied from a real capture; production rotates these every week, so treat
    # the trio here as a shape to render against, not as the live answer.
    {"uuid": 4005, "name_id": "Leaderboard_Effort_CandyBarbarian", "name": "CANDY BARBARIAN",
     "category_id": "Leaderboard_Category_Effort", "category": "EFFORT",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 4006, "name_id": "Leaderboard_Effort_IceSage", "name": "ICE SAGE",
     "category_id": "Leaderboard_Category_Effort", "category": "EFFORT",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
    {"uuid": 4010, "name_id": "Leaderboard_Effort_TombRaiser", "name": "TOMB RAISER",
     "category_id": "Leaderboard_Category_Effort", "category": "EFFORT",
     "contest_type": "weekly", "reset_kind": "weekly", "player_board": True},
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

# Boards 2004 / 2014 / 2024 pack a depth AND a run time into one float:
#   score = depth + 1 - sqrt(minutes / 180)
# The shared entry stub is mastery-sized, so those boards get their own scores -
# otherwise the delve rendering can't be looked at locally. Real depths from a
# capture, with run times spread across the curve (fast runs at the top).
STUB_DELVE_ENTRIES = [
    {"player_name": "Skill", "rank": 1, "score": 235.46743369687553},      # 235 in 51:03
    {"player_name": "noa00__00", "rank": 1, "score": 235.46743369687553},
    {"player_name": "MaxOG", "rank": 3, "score": 235.2},                   # 235, slower
    {"player_name": "Bae", "rank": 4, "score": 234.85757003175073},
    {"player_name": "VatsanT", "rank": 5, "score": 220.7643},              # 220 in 10:00
    {"player_name": "Aallyn", "rank": 6, "score": 186.83141108746116},
    {"player_name": "TestPlayer", "rank": 7, "score": 150.0},              # 150 at the 180:00 cap
]
# Packed by uuid, exactly as the site hardcodes it - nothing in a board's name
# separates these three from the delve boards that carry a plain depth.
STUB_DELVE_UUIDS = {2004, 2014, 2024}


def _with_movement(rows, delve=False):
    """Attach the day-over-day fields the real /entries response carries, so the
    movement chips render locally. The prior scores are made up here, but their
    SHAPE is what matters: on a packed delve board the pair has to cover both a
    depth change and a same-depth run-time change, since the page reports those
    two as different readings (never as the subtraction)."""
    delve_prev = [
        None,                       # no prior row -> NEW
        lambda v: v - 2.0,          # two depths deeper
        lambda v: v - 0.06,         # same depth, faster run
        lambda v: v + 1.4,          # lost a depth
        lambda v: v,                # unchanged
        lambda v: v + 0.02,         # same depth, slower run
        None,
    ]
    plain_prev = [
        None,
        lambda v: v - 240.0,
        lambda v: v - 12.0,
        lambda v: v + 95.0,
        lambda v: v,
        lambda v: v + 3.0,
        None,
    ]
    prev = delve_prev if delve else plain_prev
    out = []
    for i, row in enumerate(rows):
        row = dict(row)
        fn = prev[i % len(prev)]
        if fn is None:
            row.update(is_new=True, prev_rank=None, prev_score=None,
                       rank_delta=None, score_delta=None)
        else:
            p = fn(row["score"])
            row.update(is_new=False, prev_rank=row["rank"] + 1, prev_score=p,
                       rank_delta=1, score_delta=row["score"] - p)
        out.append(row)
    return out

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
_DRESS_CLASSES = [
    {"key": "knight", "name": "Knight", "skeleton": "biped_medium", "weapons": ["Melee"],
     "sockets": [{"ap": "r_prop", "slot": 28, "family": "Melee"},
                 {"ap": "hat", "slot": 24, "family": "Hat"},
                 {"ap": "face", "slot": 26, "family": "Face"}], "costumes": 2},
    {"key": "candybarbarian", "name": "Candy Barbarian", "skeleton": "candybarbarian",
     "weapons": ["Melee"],
     "sockets": [{"ap": "prop_r_jnt", "slot": 28, "family": "Melee"},
                 {"ap": "prop_l_jnt", "slot": 28, "family": "Melee"},
                 {"ap": "hat", "slot": 24, "family": "Hat"},
                 {"ap": "face", "slot": 26, "family": "Face"}], "costumes": 1},
]

_DRESS_OPTIONS = {
    "costume": [
        {"key": "knight_lvl3", "name": "Knight", "slot": "costume", "family": "",
         "blueprint": "", "prefab": "prefabs/skins/knight_lvl3.binfab"},
        {"key": "knight_dragon", "name": "Dragon Knight", "slot": "costume", "family": "",
         "blueprint": "", "prefab": "prefabs/skins/knight_dragon.binfab"},
    ],
    "hat": [{"key": "hat_party", "name": "Party Hat", "slot": "hat", "family": "Hat",
             "blueprint": "equipment_hat_party", "prefab": "prefabs/equipment/hat_party.binfab"}],
    "face": [{"key": "face_wolf", "name": "Wolf Mask", "slot": "face", "family": "Face",
              "blueprint": "equipment_face_mask_wolf", "prefab": "prefabs/equipment/face_wolf.binfab"}],
    "weapon": [{"key": "axe_purity", "name": "Axe of Purity", "slot": "weapon", "family": "Melee",
                "blueprint": "equipment_weapon_1h_axe_038", "prefab": "prefabs/equipment/axe_purity.binfab"}],
}

_STUB_MODS = [
    {"slug": "neon-hud", "title": "Neon HUD Overhaul",
     "summary": "A clean, high-contrast HUD retexture with neon accents.",
     "tags": ["GUI", "Reskin", "hud"], "owner_username": "Aallyn",
     "visibility": "public", "banner_sha": None, "download_count": 1280,
     "updated_at": None, "created_at": None},
    {"slug": "tiny-mounts", "title": "Tiny Mounts",
     "summary": "Shrinks every mount to adorable proportions.",
     # Card text the creator also wrote in French (cards follow the site language).
     "title_i18n": {"fr": "Montures Minuscules"},
     "summary_i18n": {"fr": "Réduit toutes les montures à des proportions adorables."},
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
    _m.setdefault("is_beta", False)
    _m.setdefault("issues_enabled", True)
    _m.setdefault("open_issue_count", 0)
    _m.setdefault("owner_avatar_url", "/site/mods/image/avatarsha")
    _m.setdefault("handle", _m["owner_username"].lower())   # /mods/<handle>/<slug>
_STUB_MODS[0]["preview_sha"] = "prevsha1"   # neon-hud: no banner -> card uses first preview
_STUB_MODS[1]["mode"] = "releases"   # tiny-mounts is a releases-only mod
_STUB_MODS[0]["star_count"] = 87
_STUB_MODS[1]["star_count"] = 34
_STUB_MODS[2]["star_count"] = 12
_STUB_MODS[0]["fork_count"] = 1   # neon-hud has one fork (quiet-ui)
_STUB_MODS[1]["is_beta"] = True   # tiny-mounts is still in development

# --- Issues & requests stub (/site/mods/**/issues, /site/mods/notifications) --
_STUB_ISSUES = [
    {"number": 2, "kind": "request", "title": "Could the bars be thinner?",
     "body": "Love it, but the health bar eats a third of the HUD on 1080p.",
     "status": "open", "author": "player_two", "author_id": "u2",
     "is_author": False, "can_moderate": True, "comment_count": 1,
     "created_at": None, "updated_at": None, "last_activity_at": None,
     "closed_at": None,
     "events": [
         {"id": "e1", "kind": "comment", "by_owner": True, "author": "Aallyn",
          "author_id": "u1", "created_at": None,
          "body": "Good call - **v1.3** ships a slim layout."},
     ]},
    {"number": 1, "kind": "issue", "title": "Minimap frame is offset by 2px",
     "body": "Only at UI scale 90%.", "status": "closed", "author": "player_one",
     "author_id": "u3", "is_author": False, "can_moderate": True,
     "comment_count": 0, "created_at": None, "updated_at": None,
     "last_activity_at": None, "closed_at": None,
     "events": [
         {"id": "e2", "kind": "closed", "by_owner": True, "author": "Aallyn",
          "author_id": "u1", "created_at": None, "body": ""},
     ]},
]


def _stub_issue_list(status):
    rows = [i for i in _STUB_ISSUES if status == "all" or i["status"] == status]
    return {"items": rows, "count": len(rows), "total": len(rows),
            "open_count": sum(1 for i in _STUB_ISSUES if i["status"] == "open"),
            "closed_count": None, "can_moderate": True}


# --- Flash code stub (/site/mods/releases/*/swf/scripts) --------------------
# What FFDec hands back for a real interface mod, in miniature: a couple of
# packages, one class per file, and the token shapes the viewer's highlighter has
# to get right (block + line comments, escaped quotes, hex/exponent literals).
_SWF_SCRIPTS_STUB = {
    "path": "ui/samplemod.swf", "size": 262144, "decompiler": "ffdec 26.2.1",
    "truncated": False, "count": 3,
    "scripts": [
        {"path": "SampleModUI.as", "source": """package
{
   import _kiwi.Core.UIComponent;
   import flash.display.MovieClip;
   import flash.external.ExternalInterface;
   import flash.text.TextField;

   public class SampleModUI extends UIComponent
   {

      public var label_txt:TextField;

      private var lastValue:Number = 0;

      public const MODE_ADVENTURE:String = "adventure";

      public function SampleModUI()
      {
         super();
         addFrameScript(0,this.frame1);
      }

      /* Called by the game every time the HUD refreshes. */
      public function setValue(param1:Number) : void
      {
         if(param1 == this.lastValue)
         {
            return;
         }
         this.lastValue = param1;
         this.label_txt.text = "Value: " + param1.toFixed(0x02);
         ExternalInterface.call("Mod.log","it\\'s updated // not a comment");
      }

      internal function frame1() : *
      {
         // Nothing on the timeline; everything is driven from setValue.
         this.stop();
      }
   }
}
"""},
        {"path": "_kiwi/Core/UIComponent.as", "source": """package _kiwi.Core
{
   import flash.display.MovieClip;
   import flash.events.Event;

   public class UIComponent extends MovieClip
   {

      protected var invalidated:Boolean = false;

      public function UIComponent()
      {
         super();
         this.addEventListener(Event.ADDED_TO_STAGE,this.onAdded);
      }

      protected function onAdded(param1:Event) : void
      {
         this.removeEventListener(Event.ADDED_TO_STAGE,this.onAdded);
         this.draw();
      }

      protected function draw() : void
      {
      }
   }
}
"""},
        {"path": "_kiwi/Constants/Colors.as", "source": """package _kiwi.Constants
{
   public class Colors
   {

      public static const WHITE:uint = 0xFFFFFF;

      public static const ACCENT:uint = 0x58A6FF;

      public static const FADE_SECONDS:Number = 1.5e-1;

      public function Colors()
      {
         super();
      }
   }
}
"""},
    ],
}
for _s in _SWF_SCRIPTS_STUB["scripts"]:
    _s["size"] = len(_s["source"])


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
                available=True, reason=None, author=None, beta=False):
    return {"handle": handle, "slug": slug, "title": title, "custom": False,
            "author": author or handle.title(), "branch": branch,
            "version": version, "version_locked": locked,
            "locked_tag": version if locked else None, "is_beta": beta,
            "available": available, "reason": reason}


def _stub_custom_entry(title, author):
    return {"custom": True, "custom_sha": "deadbeef", "custom_filename": f"{title}.tmod",
            "handle": "", "slug": "", "title": title, "author": author, "branch": "",
            "version": None, "version_locked": False, "locked_tag": None,
            "is_beta": False, "available": True, "reason": None}


def _stub_pack_detail(handle, slug):
    base = next((p for p in _STUB_PACKS if p["slug"] == slug), _STUB_PACKS[0])
    variants = [
        {"name": "default", "label": "Default", "mod_count": 4, "available_count": 3,
         "entries": [
             _stub_entry("aallyn", "neon-hud", "Neon HUD Overhaul", "main", "v1.2.0", author="Aallyn"),
             _stub_entry("skill", "tiny-mounts", "Tiny Mounts", "main", "v0.9", locked=True, author="Skill", beta=True),
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
        "Pick mods, group them into editions and lock versions. Downloads as a "
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


# A stand-in sound bank for /embed/viewer's audio player. Shaped exactly like the
# real index (app/trove/audio), including one media object that cannot be decoded -
# the list has to keep showing those rather than silently dropping them.
_DEV_SOUNDS = [
    {"id": 1099092, "name": "ui_gems_upgrade_sm_01", "group": "ui_gems", "path": "",
     "source": "", "notes": "", "bytes": 18240, "codec": "vorbis", "channels": 2,
     "sample_rate": 44100, "duration": 1.4, "object_id": 1, "error": None},
    {"id": 1099093, "name": "ui_gems_upgrade_sm_02", "group": "ui_gems", "path": "",
     "source": "", "notes": "", "bytes": 17980, "codec": "vorbis", "channels": 2,
     "sample_rate": 44100, "duration": 2.1, "object_id": 2, "error": None},
    {"id": 2200417, "name": "wpn_sword_swing_heavy", "group": "combat", "path": "",
     "source": "", "notes": "", "bytes": 9120, "codec": "pcm", "channels": 1,
     "sample_rate": 22050, "duration": 0.8, "object_id": 3, "error": None},
    {"id": 3300512, "name": "mus_hub_loop", "group": "music", "path": "",
     "source": "", "notes": "", "bytes": 812000, "codec": "vorbis", "channels": 2,
     "sample_rate": 44100, "duration": 96.0, "object_id": 4, "error": None},
    {"id": 4400001, "name": None, "group": "", "path": "", "source": "", "notes": "",
     "bytes": 640, "codec": None, "channels": 0, "sample_rate": 0, "duration": 0.0,
     "object_id": None, "error": "unsupported codec 0x0401"},
]


def _dev_tone_wav(seconds: float, hz: int) -> bytes:
    """A plain 16-bit mono WAV. Real audio, so the transport, the seek bar and the
    browser-side waveform decode all exercise for real without a game archive."""
    import math
    import struct

    rate = 22050
    frames = max(1, int(rate * min(float(seconds) or 1.0, 8.0)))
    body = bytearray()
    for i in range(frames):
        fade = min(1.0, (frames - i) / (rate * 0.15))          # avoid an end-click
        body += struct.pack("<h", int(12000 * fade * math.sin(2 * math.pi * hz * i / rate)))
    header = (b"RIFF" + struct.pack("<I", 36 + len(body)) + b"WAVEfmt "
              + struct.pack("<IHHIIHH", 16, 1, 1, rate, rate * 2, 2, 16)
              + b"data" + struct.pack("<I", len(body)))
    return header + bytes(body)


def _stub_blueprint():
    """A small solid voxel cube in the web viewer's payload shape - enough for the
    3D stages (/updates, mod pages, /embed/viewer) to actually render in dev.
    Banded by specular value along Y so the specular map is visible locally (it
    needs the BRDF atlas below; without it every band shades as rough)."""
    n = 6
    xs, ys, zs, rgb, kind, level, spec = [], [], [], [], [], [], []
    for x in range(n):
        for y in range(n):
            for z in range(n):
                if not (x in (0, n - 1) or y in (0, n - 1) or z in (0, n - 1)):
                    continue                       # hollow shell - fewer voxels, same look
                xs.append(x); ys.append(y); zs.append(z)
                rgb.append((60 + x * 30) << 16 | (90 + y * 25) << 8 | (140 + z * 18))
                kind.append(0); level.append(255); spec.append(y % 5)
    return {"count": len(xs), "size": [n, n, n], "x": xs, "y": ys, "z": zs,
            "rgb": rgb, "kind": kind, "level": level, "spec": spec}


# The specular BRDF atlas the 3D viewers sample. Production pulls it out of the
# updates CAS; locally it can only come from a game install, so point
# TROVE_LIVE_DIR at one (or accept the default Glyph path). Missing -> 404, which
# is exactly what the viewers already degrade around.
_BRDF_SRC = Path(os.environ.get(
    "TROVE_LIVE_DIR", r"C:/Program Files (x86)/Glyph/Games/Trove/Live")) / "extracted/textures/brdfmap.dds"
_brdf_png_cache: bytes | None = None


# The .swf asset gallery runs the REAL extractor (app.trove.swf) so the dev
# preview exercises the same decode prod does. Point TROVE_DEV_SWF at a game
# interface file for real artwork; otherwise a synthetic movie is built below,
# which is enough to drive the gallery, the filter and the download paths.
_SWF_SRC = os.environ.get("TROVE_DEV_SWF") or str(
    Path(os.environ.get("TROVE_LIVE_DIR",
                        r"C:/Program Files (x86)/Glyph/Games/Trove/Live"))
    / "extracted/ui/settings.swf"
)
_swf_manifest_cache: dict | None = None


def _synthetic_swf() -> bytes:
    """A minimal CWS movie holding a handful of DefineBitsLossless2 bitmaps."""
    import struct
    import zlib

    def tag(code: int, body: bytes) -> bytes:
        if len(body) < 0x3F:
            return struct.pack("<H", (code << 6) | len(body)) + body
        return struct.pack("<HI", (code << 6) | 0x3F, len(body)) + body

    swatches = [
        ("HealthBarFill", 64, 16, (214, 69, 80)),
        ("ManaBarFill", 64, 16, (76, 130, 240)),
        ("ScrollThumb", 24, 96, (120, 132, 148)),
        ("PanelBackdrop", 256, 160, (28, 36, 48)),
        ("IconCoin", 32, 32, (240, 196, 84)),
        ("IconGem", 32, 32, (156, 96, 232)),
    ]
    body = b""
    symbols = []
    for i, (name, w, h, (r, g, b)) in enumerate(swatches):
        char_id = i + 1
        # ARGB, premultiplied, with a soft alpha ramp so transparency is visible.
        px = bytearray()
        for y in range(h):
            for x in range(w):
                a = max(0, min(255, int(255 * min(x, y, w - 1 - x, h - 1 - y) / 6 + 40)))
                px += bytes((a, r * a // 255, g * a // 255, b * a // 255))
        payload = struct.pack("<HBHH", char_id, 5, w, h) + zlib.compress(bytes(px), 6)
        body += tag(36, payload)
        symbols.append((char_id, name))
    sym = struct.pack("<H", len(symbols))
    for char_id, name in symbols:
        sym += struct.pack("<H", char_id) + name.encode() + b"\0"
    body += tag(76, sym) + tag(1, b"") + tag(0, b"")

    # RECT(nbits=15) covering 800x600 px, then 24fps / 1 frame.
    rect = b"\x78\x00\x05\x00\x00\x0f\xa0\x00"
    payload = rect + struct.pack("<BBH", 0, 24, 1) + body
    head = b"CWS\x0a" + struct.pack("<I", 8 + len(payload))
    return head + zlib.compress(payload, 6)


def _swf_manifest() -> dict:
    """Extract once, cache. Mirrors what app.trove.swf.service stores in prod."""
    global _swf_manifest_cache
    if _swf_manifest_cache is None:
        import sys
        sys.path.insert(0, str(ROOT))
        from app.trove.swf.extract import extract_images

        try:
            raw = Path(_SWF_SRC).read_bytes()
        except OSError:
            raw = _synthetic_swf()
        header, images, inventory = extract_images(raw)
        _swf_manifest_cache = {
            "swf": {"version": header.version, "compression": header.compression,
                    "width": header.width, "height": header.height,
                    "frame_rate": round(header.frame_rate, 2),
                    "frame_count": header.frame_count},
            "inventory": inventory,
            "images": images,
        }
    return _swf_manifest_cache


# The .bnk sound browser runs the REAL decoder (app.trove.audio) so the dev
# preview exercises the same Vorbis/ADPCM/PCM path prod does. Point TROVE_DEV_BNK
# at a game bank; ui.bnk is the good one to develop against (167 named sounds,
# every codec, and small enough to index instantly). Missing -> the Audio tab
# reports an empty bank, which is a state it has to handle anyway.
_BNK_SRC = os.environ.get("TROVE_DEV_BNK") or str(
    Path(os.environ.get("TROVE_LIVE_DIR",
                        r"C:/Program Files (x86)/Glyph/Games/Trove/Live"))
    / "extracted/audio/ui.bnk"
)
_bnk_cache: dict | None = None


def _bank_manifest() -> dict:
    """Index once, cache. Mirrors what app.trove.audio.service stores in prod,
    except the ``.wem`` bytes stay in memory instead of going to a content store."""
    global _bnk_cache
    if _bnk_cache is None:
        import sys
        sys.path.insert(0, str(ROOT))
        from app.trove.audio import bank as bank_reader
        from app.trove.audio import names as name_reader
        from app.trove.audio import wem as wem_reader

        sounds: list[dict] = []
        blobs: dict[int, bytes] = {}
        raw_bank = b""
        info = {"version": 0, "bank_id": 0, "sections": [], "objects": 0, "events": 0}
        try:
            raw = raw_bank = Path(_BNK_SRC).read_bytes()
            parsed = bank_reader.parse(raw)
            named: dict[int, object] = {}
            events: dict[int, str] = {}
            try:
                side = Path(name_reader.sidecar_path(_BNK_SRC))
                named, events = name_reader.parse(side.read_text("utf-8", errors="replace"))
            except OSError:
                pass
            info = {"version": parsed.version, "bank_id": parsed.bank_id,
                    "sections": [s.tag for s in parsed.sections],
                    "objects": len(parsed.objects), "events": len(events)}
            for entry in parsed.media:
                data = parsed.media_bytes(entry)
                blobs[entry.media_id] = data
                tag = named.get(entry.media_id)
                record = {"id": entry.media_id, "bytes": entry.size,
                          "name": getattr(tag, "name", None),
                          "group": getattr(tag, "group", ""),
                          "path": getattr(tag, "path", ""),
                          "source": getattr(tag, "source", ""),
                          "notes": getattr(tag, "notes", "")}
                try:
                    meta = wem_reader.parse(data)
                except wem_reader.WemError as exc:
                    record |= {"codec": None, "channels": 0, "sample_rate": 0,
                               "duration": 0.0, "error": str(exc)}
                else:
                    record |= {"codec": meta.codec, "channels": meta.channels,
                               "sample_rate": meta.sample_rate,
                               "duration": round(meta.duration, 3), "error": None}
                sounds.append(record)
            sounds.sort(key=lambda s: (s["name"] or "").lower() or f"~{s['id']}")
        except (OSError, bank_reader.BankError):
            pass
        _bnk_cache = {"bank": info, "sounds": sounds, "blobs": blobs, "raw": raw_bank}
    return _bnk_cache


def _brdf_map_png() -> bytes | None:
    global _brdf_png_cache
    if _brdf_png_cache is None:
        try:
            from io import BytesIO

            from PIL import Image
            buf = BytesIO()
            Image.open(_BRDF_SRC).convert("RGB").save(buf, format="PNG")
            _brdf_png_cache = buf.getvalue()
        except Exception:      # noqa: BLE001 - no game install / no Pillow: just 404
            return None
    return _brdf_png_cache


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
    "ui/settings.swf",
    # A real effect path, so picking a .pkfx in the explorer exercises the inline
    # VFX preview (it resolves against the local pack, like the API's does).
    "particles/VFX/Particles/character_ally_dancepad_lights_01.pkfx",
    "audio/ui.bnk",
    "audio/ui.txt",
    "audio/foley.bnk",
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


# ── VFX previewer: the one surface a synthetic tree can't fake ────────────
# A particle effect only means anything when it plays, and it only plays with the
# game's own textures and meshes - so this tab reads an extracted PopcornFX pack
# off disk (the folder holding popcornproject.xml) instead of stubbing anything.
# Point SITE_DEV_VFX_DIR at one; without it the tab renders its empty state.
def _vfx_dir() -> str:
    """Where the extracted pack lives: $SITE_DEV_VFX_DIR, else the project's own
    ``PKFX_DEV_VFX_DIR`` (the same .env knob the Mods Hub's dev resolver reads)."""
    if os.environ.get("SITE_DEV_VFX_DIR"):
        return os.environ["SITE_DEV_VFX_DIR"]
    if os.environ.get("PKFX_DEV_VFX_DIR"):
        return os.environ["PKFX_DEV_VFX_DIR"]
    env = ROOT / ".env"
    if env.is_file():
        for line in env.read_text("utf-8", "replace").splitlines():
            key, _, val = line.partition("=")
            if key.strip().upper() == "PKFX_DEV_VFX_DIR":
                return val.strip().strip('"').strip("'")
    return ""


_VFX_DIR = _vfx_dir()
_VFX_PACK_ROOT = "particles/vfx/"
_vfx_index_cache: dict | None = None


def _vfx_index():
    """``{lowercased archive path: real file}`` for the local pack, plus the list of
    effects - the same two things app/trove/updates/vfx.py builds from the archive."""
    global _vfx_index_cache
    if _vfx_index_cache is not None:
        return _vfx_index_cache
    paths: dict[str, Path] = {}
    effects: list[dict] = []
    root = Path(_VFX_DIR) if _VFX_DIR else None
    if root and root.is_dir():
        for f in root.rglob("*"):
            if not f.is_file():
                continue
            rel = f.relative_to(root).as_posix()
            paths[_VFX_PACK_ROOT + rel.lower()] = f
            if rel.lower().endswith(".pkfx"):
                effects.append({"path": "particles/VFX/" + rel,
                                "size": f.stat().st_size, "thumb": None})
        for e in effects:
            name = e["path"].rsplit("/", 1)[-1].lower()
            thumb = _VFX_PACK_ROOT + "editor/thumbnails/particles/" + name + ".png"
            if thumb in paths:
                e["thumb"] = thumb
        effects.sort(key=lambda e: (e["path"].rsplit("/", 1)[-1].lower(), e["path"].lower()))
    _vfx_index_cache = {"paths": paths, "effects": effects}
    return _vfx_index_cache


def _vfx_resolve(ref: str):
    idx = _vfx_index()
    r = (ref or "").replace("\\", "/").lstrip("/").lower()
    if not r:
        return None
    return idx["paths"].get(r) or idx["paths"].get(_VFX_PACK_ROOT + r)


def _vfx_helpers():
    """The production reference parser, so dev classifies deps exactly as the API
    does (it imports nothing but ``re``). None if run outside the project venv."""
    try:
        from app.trove.mods_hub.vfx import extract_refs, media_type_for
    except Exception:      # noqa: BLE001 - bare interpreter: fall back to no deps
        return None, None
    return extract_refs, media_type_for


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
    # The lead riser is deliberately a >100% move. Live data is full of them
    # (+850% on a thin item is routine), and that asymmetry - rises unbounded,
    # falls floored at -100% - is what used to starve the Fallers column when
    # both sides shared one abs()-ranked query. Keep a runaway riser here so the
    # stub renders the same shape production does.
    import time
    return {"days": 14, "now": int(time.time()),
            "risers": [
                {"name": "Onionito", "recent_med": 18999.0, "prior_med": 2000.0, "recent_n": 35, "change": 8.4995},
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
        # Two buckets nobody listed in, so local preview exercises the dashed
        # "no data" bridge instead of only the happy gapless path.
        if days >= 8 and i in (days - 6, days - 5):
            continue
        bucket = ((now - (days - 1 - i) * day) // day) * day
        wob = 1 + 0.15 * math.sin(i / 2.0) + ((i * 7 % 5) - 2) * 0.02
        p50 = round(base * wob, 2)
        pts.append({"bucket": bucket, "listings": 8 + (i * 13 % 20), "stack": 100 + (i * 29 % 400),
                    "p50": p50, "p25": round(p50 * 0.85, 2), "p75": round(p50 * 1.2, 2)})
    events = [{"name": "Fluxion", "kind": "merchant", "starts_at": now - 11 * day, "ends_at": now - 8 * day},
              {"name": "Corruxion", "kind": "merchant", "starts_at": now - 6 * day, "ends_at": now - 3 * day}]
    return {"name": name, "days": days, "bucket_hours": 24, "points": pts, "events": events, "now": now}


# ── /market Browse tab synthetic listings ─────────────────────────────────
# Prices deliberately span the whole fee curve (a few hundred flux up to past
# the 40M saturation point) so the client-side Fee / Seller nets columns are
# actually exercised, cap included, instead of all landing in the flat
# low-end of the curve.
_MARKET_STACKS = [1, 1, 5, 10, 25, 100, 1, 3, 50, 200, 1, 100]
_MARKET_UNIT_MULT = [0.62, 0.78, 0.91, 1.0, 1.04, 1.12, 1.3, 1.55, 2.1, 3.4,
                     8.0, 40.0]


def _market_listings(name, limit, offset):
    import time
    now = int(time.time())
    base = 900 + (len(name) * 613 % 40000)
    rows = []
    for i in range(len(_MARKET_STACKS)):
        stack = _MARKET_STACKS[i]
        each = round(base * _MARKET_UNIT_MULT[i], 2)
        rows.append({
            "id": f"stub-{i}",
            "name": name,
            "type": "Crafting",
            "stack": stack,
            "price": int(each * stack),
            "price_each": each,
            "created_at": now - (i + 1) * 5400,
            "last_seen": now - (i % 4) * 3600,
            "expired": False,
        })
    rows.sort(key=lambda r: r["price_each"])
    return {"items": rows[offset:offset + limit], "total": len(rows),
            "limit": limit, "offset": offset}


def _market_item_summary(name):
    rows = _market_listings(name, 999, 0)["items"]
    each = sorted(r["price_each"] for r in rows)
    mid = len(each) // 2
    return {"name": name, "count": len(rows), "min_each": each[0],
            "max_each": each[-1], "median_each": each[mid],
            "avg_each": round(sum(each) / len(each), 2),
            "total_stack": sum(r["stack"] for r in rows)}


def _market_item_history(name, days):
    """Scatter payload for the Browse price-evolution chart."""
    import time
    now = int(time.time())
    rows = _market_listings(name, 999, 0)["items"]
    pts = [{"created_at": r["created_at"], "price_each": r["price_each"]} for r in rows]
    pts.sort(key=lambda p: p["created_at"])
    return {"name": name, "points": pts, "count": len(pts),
            "window_start": now - days * 86400, "window_end": now,
            "truncated": False, "outliers_excluded": 0}


class Handler(SimpleHTTPRequestHandler):
    def do_PATCH(self):
        if self.path.startswith(_AUTH_PREFIX):
            return self._proxy_auth()
        return self.send_error(405)

    def do_DELETE(self):
        if self.path.startswith(_AUTH_PREFIX):
            return self._proxy_auth()
        return self.send_error(405)

    def _proxy_auth(self):
        """Reverse-proxy /v1/site-auth/* to the real API.

        The site session is HttpOnly cookies now, and a cookie set for
        `.aallyn.net` over https is one a browser on http://localhost will not
        store - so talking to api.aallyn.net directly from dev cannot work. We
        stand in as the site origin instead: forward the browser's cookies up,
        rewrite Set-Cookie on the way back down (drop Domain + Secure so
        localhost accepts them), and present `Origin: <app_url>` because the
        API's cookie-origin allowlist rightly refuses localhost in production.

        The upshot is that dev exercises the SAME cookie code path prod does,
        rather than quietly falling back to something more permissive.
        """
        length = int(self.headers.get("Content-Length", 0) or 0)
        body = self.rfile.read(length) if length else None

        req = _urlreq.Request(_API_ORIGIN + self.path, data=body, method=self.command)
        for h in ("Content-Type", "Cookie", "Authorization", "Accept"):
            if self.headers.get(h):
                req.add_header(h, self.headers[h])
        req.add_header("Origin", _SITE_ORIGIN)

        try:
            resp = _urlreq.urlopen(req, timeout=20)
            status, headers, payload = resp.status, resp.headers, resp.read()
        except _urlerr.HTTPError as e:
            status, headers, payload = e.code, e.headers, e.read()
        except Exception as e:  # noqa: BLE001 - dev proxy: surface, never crash
            return self._send_json({"detail": f"auth proxy failed: {e}"}, 502)

        self.send_response(status)
        for cookie in headers.get_all("Set-Cookie") or []:
            parts = [
                p for p in cookie.split("; ")
                if not p.lower().startswith("domain=") and p.lower() != "secure"
            ]
            self.send_header("Set-Cookie", "; ".join(parts))
        self.send_header("Content-Type", headers.get("Content-Type", "application/json"))
        self.send_header("Content-Length", str(len(payload)))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        if payload:
            self.wfile.write(payload)

    def do_GET(self):
        url = urlparse(self.path)
        path = url.path

        if path.startswith(_AUTH_PREFIX):
            return self._proxy_auth()

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
        if path == "/sound-studio":
            return self._send_file(TEMPLATES / "sound-studio.html", "text/html")
        if path == "/mod-workshop":
            return self._send_file(TEMPLATES / "mod-workshop.html", "text/html")
        if path == "/blueprint-editor":
            return self._send_file(TEMPLATES / "blueprint-editor.html", "text/html")
        if path == "/unlock-debug":
            return self._send_file(TEMPLATES / "unlock-debug.html", "text/html")
        if path == "/codexes":
            return self._send_file(TEMPLATES / "codexes.html", "text/html")
        if path == "/codexes/crafting":
            return self._send_file(TEMPLATES / "codexes-crafting.html", "text/html")
        if path == "/status":
            return self._send_file(TEMPLATES / "status.html", "text/html")
        if path == "/server-time":
            return self._send_file(TEMPLATES / "server-time.html", "text/html")
        if path == "/tomes":
            return self._send_file(TEMPLATES / "tomes.html", "text/html")
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
        if path == "/fishing-guide":
            return self._send_file(TEMPLATES / "fishing-guide.html", "text/html")
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
        if path == "/dressing-room":
            return self._send_file(TEMPLATES / "dressing-room.html", "text/html")
        if path == "/star-chart":
            return self._send_file(TEMPLATES / "star-chart.html", "text/html")
        if path == "/login":
            return self._send_file(TEMPLATES / "login.html", "text/html")
        if path.startswith("/drop/"):
            # One-off upload link. The slug is the URL's last segment in
            # production too, so the shell renders the same here; whether the
            # link is alive is the API's answer, not this server's.
            return self._send_file(TEMPLATES / "drop.html", "text/html",
                                   {"slug": path.rsplit("/", 1)[-1]})
        if path.startswith("/player/"):
            # Public player profile shell. data-player is stripped to empty by the
            # {{ }} emulation, but player.js falls back to the URL path segment.
            return self._send_file(TEMPLATES / "player.html", "text/html")
        if path == "/embed/viewer":
            # Embeddable viewer shell (app/embed). Production passes the source params
            # into the template; do the same from the query string here, defaulting to
            # a stub upload token so a bare /embed/viewer still renders something.
            q = parse_qs(url.query)
            def _q(name, default=""):
                return q.get(name, [default])[0]
            other = _q("release") or _q("game") or _q("prefab") or _q("dress")
            return self._send_file(TEMPLATES / "embed_viewer.html", "text/html", {
                "release": _q("release"),
                "tmod": _q("tmod") or ("" if other else "devtoken"),
                "game": _q("game"),
                "prefab": _q("prefab"),
                "dress": _q("dress"),
                "path": _q("path"),
                "sound": _q("sound"),
                "mode": _q("mode", "auto"),
                "theme": _q("theme", "dark"),
                "app_url": "http://localhost:8913",
            })

        # Embeddable viewer stubs (/site/embed/*) - one blueprint + one effect, so
        # the page exercises its tabs, picker and viewer mount without a game tree.
        if path == "/site/embed/manifest":
            # A `game=` source the stub doesn't know answers like production does -
            # the error envelope - so the in-frame error state is exercisable too.
            want = parse_qs(url.query).get("game", [""])[0]
            if want and want not in _UPDATE_PATHS:
                return self._send_json(
                    {"error": {"message": f"No '{want}' in the current game files."}}, 404)
            # Two blueprints so the file picker is exercised; no rig (the assembled
            # tab needs baked rigs + the codex, neither of which the stub has).
            return self._send_json({
                "source": "tmod", "title": "Dev preview",
                "blueprints": {"items": [
                    {"path": "blueprints/dev_cube.blueprint", "size": 512, "assembled": False},
                    {"path": "blueprints/dev_cube_alt.blueprint", "size": 512, "assembled": False},
                ], "rig": None, "animations": []},
                "vfx": {"items": []},
                # One bank, so the Sounds tab and the audio player mount in dev.
                "audio": {"items": [{"path": "audio/dev_bank.bnk", "size": 4096}]},
            })
        if path == "/site/render/brdf-map.png":
            png = _brdf_map_png()
            if png is None:
                return self._send_json({"error": {"message": "No local game install."}}, 404)
            self.send_response(200)
            self.send_header("Content-Type", "image/png")
            self.send_header("Content-Length", str(len(png)))
            self.end_headers()
            return self.wfile.write(png)
        # Dressing room. There is no game archive in dev, so the catalogue is stubbed
        # and every outfit draws the pre-baked spider - enough to exercise the picker,
        # the URL state and the viewer mount.
        if path == "/site/dressing/classes":
            return self._send_json({"items": _DRESS_CLASSES})
        if path == "/site/dressing/options":
            slot = (parse_qs(url.query).get("slot") or ["costume"])[0]
            return self._send_json({"items": _DRESS_OPTIONS.get(slot, []),
                                    "total": len(_DRESS_OPTIONS.get(slot, [])),
                                    "offset": 0, "limit": 100})
        if path == "/site/dressing/model":
            mp = STATIC / "models" / "companion_spidermonkey.model.json"
            if mp.exists():
                return self._send_file(mp, "application/json")
            return self._send_json({"error": {"message": "no model"}}, 404)
        if path == "/site/dressing/render":
            return self._send_json({"error": {"message": "no thumbnails in dev"}}, 404)
        if path == "/site/embed/blueprint":
            return self._send_json(_stub_blueprint())
        if path == "/site/embed/assembled":
            return self._send_json({"error": {"message": "No assemblable creature here."}}, 404)
        if path.startswith("/site/embed/vfx/"):
            return self._send_json({"error": {"message": "No VFX in this dev stub."}}, 404)
        if path == "/site/embed/audio/bank":
            return self._send_json({
                "path": "audio/dev_bank.bnk",
                "bank": {"version": 128, "bank_id": 1, "sections": ["BKHD", "DIDX", "DATA"],
                         "objects": len(_DEV_SOUNDS), "events": 0},
                "sounds": _DEV_SOUNDS,
                "count": len(_DEV_SOUNDS),
                "playable": sum(1 for s in _DEV_SOUNDS if not s["error"]),
                "total_duration": round(sum(s["duration"] for s in _DEV_SOUNDS), 1),
            })
        if path == "/site/embed/audio/sound":
            want = (parse_qs(url.query).get("id") or ["0"])[0]
            sound = next((s for s in _DEV_SOUNDS if str(s["id"]) == want), None)
            if sound is None or sound["error"]:
                return self._send_json({"error": {"message": "No such sound."}}, 404)
            wav = _dev_tone_wav(sound["duration"], 180 + (sound["id"] % 7) * 90)
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(wav)))
            self.send_header("Cache-Control", "public, max-age=60")
            self.end_headers()
            return self.wfile.write(wav)

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
        if path.startswith("/site/drops/"):
            # A one-off upload link, always alive in the preview. Production
            # answers 404 for a slug that has expired or been used up; there is
            # no way to reach that state here, so the shell renders the form.
            import time as _t
            return self._send_json({
                "label": "Send me your Trove log",
                "max_file_bytes": 256 * 1024 * 1024,
                "uploads_left": 1,
                "expires_at": _t.strftime("%Y-%m-%dT%H:%M:%SZ",
                                          _t.gmtime(_t.time() + 86400)),
            })
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
            # Padded past the grid's fold so "Show more" is testable locally.
            mods += [dict(m, slug=f"{m['slug']}-{i}", title=f"{m['title']} {i}")
                     for i in (2, 3) for m in _STUB_MODS]
            return self._send_json({
                "handle": handle or "aallyn",
                "display_name": "Aallyn",
                "tagline": "Trove modder · HUD & retexture artist",
                "readme": "## Hey!\n\nI make **clean HUD** mods. Check out my work below.\n\n"
                "[![ko-fi](https://img.shields.io/badge/support-ko--fi-ff5e5b)](https://ko-fi.com/example)",
                "tagline_i18n": {"fr": "Moddeur Trove et artiste de textures"},
                "readme_i18n": {"fr": "## Bonjour\n\nMa page, en français."},
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
        if path == "/site/mods/notifications":
            return self._send_json({"unread": 1, "seen_at": None, "items": [
                {"number": 2, "kind": "request", "title": "Could the bars be thinner?",
                 "status": "open", "mod_title": "Neon HUD", "handle": "aallyn",
                 "slug": "neon-hud", "url": "/mods/aallyn/neon-hud#issue-2",
                 "comment_count": 1, "last_activity_at": None, "unread": True},
            ]})
        if path == "/site/mods/tags":
            return self._send_json({
                "categories": [{"tag": "GUI", "count": 7}, {"tag": "Dragons", "count": 3},
                               {"tag": "Reskin", "count": 5}, {"tag": "Mounts", "count": 2}],
                "custom": [{"tag": "retexture", "count": 4}, {"tag": "hud", "count": 2},
                           {"tag": "fun", "count": 1}, {"tag": "minimal", "count": 1}],
            })
        if path == "/site/mods/projects":
            # Honour q/tag like the real endpoint, so the no-results empty state
            # is reachable locally instead of only in production.
            rows = _STUB_MODS
            mq = parse_qs(url.query)
            q = (mq.get("q", [""])[0] or "").strip().lower()
            tag = (mq.get("tag", [""])[0] or "").strip().lower()
            if q:
                rows = [m for m in rows if q in m["title"].lower()
                        or q in (m.get("summary") or "").lower()
                        or q in m["owner_username"].lower()]
            if tag:
                rows = [m for m in rows if tag in [str(x).lower() for x in (m.get("tags") or [])]]
            return self._send_json({"items": rows, "count": len(rows),
                                    "total": len(rows)})
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
        # Stub the packed-config list + download so the dedicated "Config" button renders.
        # The decoded artifact behind the release "Contents" modal.
        if path.startswith("/site/mods/releases/") and path.endswith("/inspect"):
            files = [
                {"path": "ui/samplemod.swf", "size": 262144},
                {"path": "ui/samplemod.cfg", "size": 128},
                {"path": "ui/extra.cfg", "size": 64},
                {"path": "ui/sample-mod.png", "size": 40960},
                {"path": "blueprints/equipment/c_p_knight_lvl3_torso.blueprint", "size": 1820},
                {"path": "blueprints/equipment/c_p_knight_lvl3_l_hand.blueprint", "size": 540},
                {"path": "prefabs/item/style/hat.binfab", "size": 2048},
            ]
            return self._send_json({
                "format": "tmod", "tag": "v1.2.0", "branch": "main",
                "filename": "SampleMod.tmod", "sha256": "0" * 64, "prior_sha256s": [],
                "size": 20480, "version": 1, "readable": True,
                "properties": {
                    "title": "SampleMod", "author": "Aallyn", "modVersion": "v1.2.0",
                    "notes": "Local preview build.\nSecond line of notes.",
                    "modLoader": "KiwiAPI", "previewPath": "ui/sample-mod.png",
                    "configPath": "ui/samplemod.cfg", "tags": "Interface,Quality of Life",
                },
                "categories": ["Interface", "Quality of Life"], "flags": 6,
                "preview_path": "ui/sample-mod.png", "config_path": "ui/samplemod.cfg",
                "files": files, "file_count": len(files),
                "total_size": sum(f["size"] for f in files),
            })
        # Two configs, one declared by the build's configPath - so the page shows a
        # single Config button (the declared one) rather than one per .cfg.
        if path.startswith("/site/mods/releases/") and path.endswith("/cfgs"):
            return self._send_json({"has_flash_ui": True, "items": [
                {"path": "ui/samplemod.cfg", "size": 128, "filename": "SampleMod.cfg",
                 "declared": True},
                {"path": "ui/extra.cfg", "size": 64, "filename": "extra.cfg",
                 "declared": False}]})
        if path.startswith("/site/mods/releases/") and path.endswith("/cfg"):
            return self._send_bytes(b"[Settings]\nstub = 1\n", "text/plain; charset=utf-8")
        # The build's .swf, and whether code can be read out of it - the real thing
        # needs FFDec and an actual movie, so dev answers yes and serves a small
        # hand-written class tree below.
        if path.startswith("/site/mods/releases/") and path.endswith("/swfs"):
            return self._send_json({"decompiler": True, "items": [
                {"path": "ui/samplemod.swf", "size": 262144}]})
        if path.startswith("/site/mods/releases/") and path.endswith("/swf/scripts"):
            return self._send_json(_SWF_SCRIPTS_STUB)
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
        # Sound banks bundled in a release - the same stub bank the embed viewer
        # gets, so the release page's player is exercisable locally too.
        if path.startswith("/site/mods/releases/") and path.endswith("/audio"):
            return self._send_json({
                "items": [{"path": "audio/dev_bank.bnk", "size": 4096},
                          {"path": "audio/dev_music.bnk", "size": 812000}],
            })
        if path.startswith("/site/mods/releases/") and path.endswith("/audio/bank"):
            return self._send_json({
                "path": (parse_qs(url.query).get("path") or ["audio/dev_bank.bnk"])[0],
                "bank": {"version": 128, "bank_id": 1, "sections": ["BKHD", "DIDX", "DATA"],
                         "objects": len(_DEV_SOUNDS), "events": 0},
                "sounds": _DEV_SOUNDS,
                "count": len(_DEV_SOUNDS),
                "playable": sum(1 for s in _DEV_SOUNDS if not s["error"]),
                "total_duration": round(sum(s["duration"] for s in _DEV_SOUNDS), 1),
            })
        if path.startswith("/site/mods/releases/") and path.endswith("/audio/sound"):
            want = (parse_qs(url.query).get("id") or ["0"])[0]
            sound = next((s for s in _DEV_SOUNDS if str(s["id"]) == want), None)
            if sound is None or sound["error"]:
                return self._send_json({"error": {"message": "No such sound."}}, 404)
            wav = _dev_tone_wav(sound["duration"], 180 + (sound["id"] % 7) * 90)
            self.send_response(200)
            self.send_header("Content-Type", "audio/wav")
            self.send_header("Content-Length", str(len(wav)))
            self.send_header("Cache-Control", "public, max-age=60")
            self.end_headers()
            return self.wfile.write(wav)
        # Serve the pre-baked assembled spider so the model viewer can be previewed.
        if path.startswith("/site/mods/releases/") and path.endswith("/assembled"):
            mp = STATIC / "models" / "companion_spidermonkey.model.json"
            if mp.exists():
                return self._send_file(mp, "application/json")
            return self._send_json({"error": {"message": "no model"}})
        # The rig's animation state machine: /site/rigs/<skeleton>/graph
        if path.startswith("/site/rigs/") and path.endswith("/graph"):
            import re as _re
            m = _re.match(r"^/site/rigs/([a-z0-9_]+)/graph$", path)
            if m:
                gp = _under(ROOT / "app" / "trove" / "mods_hub" / "rigs" / "graph",
                            m.group(1) + ".graph.json")
                if gp is not None and gp.exists():
                    return self._send_file(gp, "application/json")
            return self._send_json({"error": {"message": "no graph"}}, 404)
        # Lazily-loaded rig animation clip (TANIM1 binary): /site/rigs/<skeleton>/anim/<name>
        if path.startswith("/site/rigs/") and "/anim/" in path:
            import re as _re
            m = _re.match(r"^/site/rigs/([a-z0-9_]+)/anim/([a-z0-9_]+)$", path)
            if m:
                ap = _under(ROOT / "app" / "trove" / "mods_hub" / "rigs" / "anim",
                            m.group(1), m.group(2) + ".anim")
                if ap is not None and ap.exists():
                    return self._send_file(ap, "application/octet-stream")
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
                    "title_i18n": {"fr": "Montures Minuscules"},
                    "warnings_i18n": {
                        "fr": "Nécessite la dernière version du jeu.<br>Sauvegardez votre dossier mods.",
                    },
                    "description_i18n": {
                        "fr": "Un mod **d'exemple** pour l'aperçu local.\n\n"
                              "La description traduite par le moddeur.",
                    },
                    "readme_text": "## Sample README\n\n"
                    "[![badge](https://img.shields.io/badge/build-passing-brightgreen)](https://example.com)\n\n"
                    "<div align=\"center\">Centered HTML works.</div>\n\n"
                    "| A | B |\n|---|---|\n| 1 | 2 |\n\n"
                    "- bullet one\n- bullet two\n\n"
                    "Saved on the project (releases-only mode). Normal **bold** + `code`.",
                    # Creator-written translations; the page shows the reader's
                    # language and offers a switch for the others.
                    "readme_i18n": {
                        "fr": "## README d'exemple\n\nLa version française, écrite par le moddeur.\n\n"
                              "- premier point\n- deuxième point",
                        "ja": "## サンプル README\n\nモッダーが書いた日本語版です。\n\n- 一つ目\n- 二つ目",
                    },
                    "warnings": "Requires the latest game build.<br>Back up your mods folder first.",
                    "default_branch": "main", "preview_shas": ["prevsha1", "prevsha2"], "taken_down": False,
                    "takedown_reason": None, "is_owner": False, "starred": False,
                    "owner_avatar_url": "/site/mods/image/avatarsha",
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
                         "title_i18n": {"fr": "Stable"},
                         "changelog_i18n": {"fr": "Dernière version stable."},
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
            if sub == "issues":
                want = (parse_qs(url.query).get("status") or ["open"])[0]
                return self._send_json(_stub_issue_list(want))
            if sub.startswith("issues/"):
                want = sub.split("/", 1)[1]
                row = next((i for i in _STUB_ISSUES if str(i["number"]) == want), None)
                if row is None:
                    return self._send_json(
                        {"error": {"message": "No such issue on this mod."}}, 404)
                return self._send_json(row)
            if sub == "archive":
                # The Files "Download ZIP" button - a real (empty) zip, so the
                # browser save path is exercised rather than mocked.
                import io as _io
                import zipfile as _zipfile
                buf = _io.BytesIO()
                with _zipfile.ZipFile(buf, "w") as zf:
                    zf.writestr("readme.md", "# Sample Mod\n")
                blob = buf.getvalue()
                self.send_response(200)
                self.send_header("Content-Type", "application/zip")
                self.send_header("Content-Length", str(len(blob)))
                self.send_header(
                    "Content-Disposition",
                    f'attachment; filename="{slug}-main-stub123.zip"')
                self.end_headers()
                return self.wfile.write(blob)
            if sub == "tree":
                return self._send_json({"commit": {"id": "stub", "seq": 2}, "entries": [
                    {"path": "readme.md", "blob_sha": "stub", "size": 180},
                    # A translated README next to the English one (files mode's
                    # equivalent of readme_i18n).
                    {"path": "README.fr.md", "blob_sha": "stub", "size": 150},
                    {"path": "config/default.cfg", "blob_sha": "stub", "size": 128},
                    # A .swf so the release modal's "Settings file" field is previewable
                    # (it only appears for mods with a Flash UI).
                    {"path": "ui/sample.swf", "blob_sha": "stub", "size": 65536},
                    {"path": "ui/icon.png", "blob_sha": "stub", "size": 4096}]})
            if sub.startswith("raw/") and sub.lower().endswith("readme.fr.md"):
                return self._send_text(
                    "# Mod d'exemple\n\nCe README est **rendu** depuis le fichier "
                    "`README.fr.md` du dépôt.\n\n## Fonctionnalités\n\n- Un truc sympa\n"
                    "- Un autre truc\n")
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
                    {"id": "c3", "seq": 3, "branch": "main", "author_username": "tester",
                     # Multi-line + overlong subjects, so the history rail's
                     # collapsed message state is previewable.
                     "message": "Retexture every HUD panel\n\nThe old sheet was a 512 atlas "
                                "shared with the map, so widening the health bar smeared the "
                                "minimap frame.\n- split the atlas\n- redrew the frames at 2x",
                     "file_count": 9, "created_at": None},
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
            # Anything inside the local PopcornFX pack is real (the VFX picker's
            # thumbnails come through here), so serve the actual bytes.
            local = _vfx_resolve(p)
            if local is not None:
                return self._send_bytes(
                    local.read_bytes(),
                    "image/png" if ext == "png" else "application/octet-stream")
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
            if sub == "vfx":
                idx = _vfx_index()
                needle = (qs.get("q", [""])[0] or "").strip().lower()
                items = [e for e in idx["effects"] if needle in e["path"].lower()]
                off = int(qs.get("offset", ["0"])[0] or 0)
                lim = int(qs.get("limit", ["300"])[0] or 300)
                page = items[off:off + lim]
                return self._send_json({"branch": branch, "items": page,
                                        "count": len(page), "total": len(items)})
            if sub == "vfx/manifest":
                target = qs.get("path", [""])[0]
                f = _vfx_resolve(target)
                if f is None:
                    return self._send_json({"detail": f"No effect '{target}'"}, 404)
                text = f.read_text("utf-8", "replace")
                extract_refs, _mime = _vfx_helpers()
                refs = extract_refs(text) if extract_refs else []
                deps = [{"ref": r, "basename": r.rsplit("/", 1)[-1],
                         "source": "game" if _vfx_resolve(r) else "missing"} for r in refs]
                return self._send_json({
                    "branch": branch, "path": target, "pkfx": text, "deps": deps,
                    "missing": [d["basename"] for d in deps if d["source"] == "missing"],
                    "game_available": True})
            if sub == "vfx/asset":
                ref = qs.get("path", [""])[0]
                f = _vfx_resolve(ref)
                if f is None:
                    return self.send_error(404)
                _refs, media_type_for = _vfx_helpers()
                media = media_type_for(ref) if media_type_for else "application/octet-stream"
                return self._send_bytes(f.read_bytes(), media)
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
                counts = {"added": sum(1 for _, t in ch if t == "added"),
                          "modified": sum(1 for _, t in ch if t == "modified"),
                          "removed": sum(1 for _, t in ch if t == "removed")}
                # Mirror prod: the type filter and paging happen HERE, so the
                # page's chips exercise the same round-trip they do live.
                ty_filter = qs.get("type", [""])[0] or ""
                rows = sorted((r for r in ch if not ty_filter or r[1] == ty_filter),
                              key=lambda r: (r[1], r[0]))
                off = int(qs.get("offset", ["0"])[0] or 0)
                lim = int(qs.get("limit", ["200"])[0] or 200)
                page = rows[off:off + lim]
                entries = [{"path": p, "type": ty, "content_sha256": "stub", "size": 512}
                           for p, ty in page]
                return self._send_json({"branch": branch, "ordinal": 2,
                    "version_tag": "1.0.stub", "entries": entries,
                    "count": len(entries), "total": len(rows),
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
                if ext == "swf":
                    return self._send_json({**base, "size": 262144, "viewable": False,
                                            "kind": "swf", "reason": "swf"})
                if ext == "bnk":
                    return self._send_json({**base, "size": 4567829, "viewable": False,
                                            "kind": "bnk", "reason": "bnk"})
                if ext == "pkfx":
                    return self._send_json({**base, "size": 50969, "viewable": False,
                                            "kind": "pkfx", "reason": "pkfx"})
                if ext in ("binfab", "tfa", "tex"):
                    return self._send_json({**base, "size": 512, "viewable": False,
                                            "kind": "binary", "reason": "binary"})
                return self._send_json({**base, "size": 42, "viewable": True,
                    "kind": "text", "reason": None,
                    "text": f"-- stub contents of {p}\nprint('hello')\n"})
            if sub == "file/swf":
                m = _swf_manifest()
                assets = [{"id": i.char_id, "name": i.name, "source": i.source,
                           "codec": i.codec, "width": i.width, "height": i.height,
                           "mime": i.mime, "bytes": len(i.data),
                           "thumb": i.thumb is not None} for i in m["images"]]
                return self._send_json({"branch": branch, "path": qs.get("path", [""])[0],
                    "content_sha256": "stub", "swf": m["swf"],
                    "inventory": m["inventory"], "assets": assets, "count": len(assets)})
            if sub == "file/swf/asset":
                m = _swf_manifest()
                want = int(qs.get("id", ["-1"])[0])
                img = next((i for i in m["images"] if i.char_id == want), None)
                if img is None:
                    return self._send_json({"detail": "no such asset"}, status=404)
                use_thumb = qs.get("thumb", ["0"])[0] in ("1", "true") and img.thumb
                data = img.thumb if use_thumb else img.data
                return self._send_bytes(data, "image/png" if use_thumb else img.mime)
            if sub == "file/swf/zip":
                import io as _io
                import zipfile as _zip
                m = _swf_manifest()
                buf = _io.BytesIO()
                with _zip.ZipFile(buf, "w", _zip.ZIP_STORED) as zf:
                    for i in m["images"]:
                        ext = {"image/jpeg": "jpg", "image/gif": "gif"}.get(i.mime, "png")
                        zf.writestr(f"{i.char_id:04d}_{i.name or 'asset'}.{ext}", i.data)
                return self._send_bytes(buf.getvalue(), "application/zip")
            if sub == "file/bnk":
                m = _bank_manifest()
                sounds = m["sounds"]
                return self._send_json({
                    "branch": branch, "path": qs.get("path", [""])[0],
                    "content_sha256": "stub", "bank": m["bank"], "sounds": sounds,
                    "count": len(sounds),
                    "playable": sum(1 for s in sounds if s["error"] is None),
                    "total_duration": round(sum(s["duration"] for s in sounds), 1)})
            if sub == "file/bnk/audio":
                import sys
                sys.path.insert(0, str(ROOT))
                from app.trove.audio import wem as wem_reader
                m = _bank_manifest()
                want = int(qs.get("id", ["-1"])[0])
                data = m["blobs"].get(want)
                if data is None:
                    return self._send_json({"detail": "no such sound"}, status=404)
                if qs.get("raw", ["0"])[0] in ("1", "true"):
                    return self._send_bytes(data, "audio/vnd.wave")
                try:
                    out, mime, _ext = wem_reader.convert(data)
                except wem_reader.WemError as exc:
                    return self._send_json({"detail": str(exc)}, status=422)
                return self._send_bytes(out, mime)
            if sub == "file/bnk/zip":
                import io as _io
                import sys
                import zipfile as _zip
                sys.path.insert(0, str(ROOT))
                from app.trove.audio import wem as wem_reader
                m = _bank_manifest()
                buf = _io.BytesIO()
                with _zip.ZipFile(buf, "w", _zip.ZIP_STORED) as zf:
                    for s in m["sounds"]:
                        if s["error"]:
                            continue
                        try:
                            out, _mime, ext = wem_reader.convert(m["blobs"][s["id"]])
                        except wem_reader.WemError:
                            continue
                        zf.writestr(f"{s['name'] or s['id']}.{ext}", out)
                return self._send_bytes(buf.getvalue(), "application/zip")
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
        if path == "/site/market/listings":
            qs = parse_qs(url.query)
            name = qs.get("name", ["Glim"])[0]
            try:
                limit = int(qs.get("limit", ["100"])[0])
                offset = int(qs.get("offset", ["0"])[0])
            except ValueError:
                limit, offset = 100, 0
            return self._send_json(_market_listings(name, limit, offset))
        if path.startswith("/site/market/items/") and path.endswith("/summary"):
            name = unquote(path[len("/site/market/items/"):-len("/summary")])
            return self._send_json(_market_item_summary(name))
        if path.startswith("/site/market/items/") and path.endswith("/history"):
            name = unquote(path[len("/site/market/items/"):-len("/history")])
            qs = parse_qs(url.query)
            try:
                days = int(qs.get("days", ["7"])[0])
            except ValueError:
                days = 7
            return self._send_json(_market_item_history(name, days))
        if path == "/site/market/analytics/signals":
            # Synthetic anomalies so the Unusual-activity UI can be worked on
            # without Postgres. Covers each severity and the market-wide alarm.
            return self._send_json({
                "days": 21, "generated_for": 0, "scanned_items": 312,
                "market": {
                    "items": 118, "median_move": 0.41, "share_up": 0.71,
                    "share_down": 0.02, "verdict": "flux_weaker",
                    "reading": (
                        "Most of the market got more expensive at once. When "
                        "unrelated items all rise together it is usually flux "
                        "losing value rather than the items gaining it - which is "
                        "what a flux duplication looks like from the outside."),
                },
                "signals": [
                    {"name": "Bleached Bone", "pattern": "supply_flood",
                     "reading": ("Price collapsed while supply and stack sizes both "
                                 "spiked - the shape a duplicated item makes."),
                     "severity": "extreme", "day": 0, "price": 310.0,
                     "baseline": 2700.0, "change": -0.885, "listings": 194,
                     "stack_med": 99.0, "stack_max": 999,
                     "price_z": -11.9, "supply_z": 42.3, "stack_z": 61.0},
                    {"name": "Credit Pouch", "pattern": "spike",
                     "reading": "Priced well above its own recent range.",
                     "severity": "extreme", "day": 0, "price": 50000000.0,
                     "baseline": 9500000.0, "change": 4.26, "listings": 12,
                     "stack_med": 1.0, "stack_max": 3,
                     "price_z": 18.4, "supply_z": 0.4, "stack_z": 0.0},
                    {"name": "Golden Seashell", "pattern": "squeeze",
                     "reading": "Supply dried up and the price ran up behind it.",
                     "severity": "elevated", "day": 0, "price": 8800.0,
                     "baseline": 4100.0, "change": 1.146, "listings": 6,
                     "stack_med": 20.0, "stack_max": 40,
                     "price_z": 5.2, "supply_z": -4.1, "stack_z": 0.2},
                ],
            })
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
            # Blueprint→PNG render. The rasterizer needs the game archive, which
            # dev doesn't have, so borrow the real one and keep the placeholder
            # as the offline fallback - thumbnails are how you tell a wrong
            # blueprint name from a right one.
            return self._send_bytes(_codex_render(self.path), "image/png")
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
        if path == "/site/tomes":
            # Real service, not a stub - it needs no DB to answer. Without
            # Postgres every median comes back empty, so dev shows the
            # "nothing listed" path, which is worth seeing.
            import asyncio

            from app.trove.tomes import service as _tomes
            return self._send_json(asyncio.run(_tomes.valued_tomes()))
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
            # Prefer the real loader so the dev server sees exactly what production
            # serves - abilities live in class_abilities.json and are merged in there,
            # so reading classes.json alone would render a page with none.
            try:
                from app.trove import stats as _stats
                return self._send_json(_stats.all_classes())
            except Exception:
                pass
            cj = ROOT / "app" / "trove" / "gamedata" / "classes.json"
            aj = ROOT / "app" / "trove" / "gamedata" / "class_abilities.json"
            abil = {}
            if aj.exists():
                abil = {c["name"]: c.get("abilities", [])
                        for c in json.loads(aj.read_text(encoding="utf-8"))}
            items = []
            if cj.exists():
                for c in json.loads(cj.read_text(encoding="utf-8")):
                    items.append({
                        "tech_name": c.get("qualified_name"), "name": c.get("name"),
                        "shorts": c.get("shorts", []), "damage_type": c.get("damage_type", ""),
                        "weapons": c.get("weapons", []), "attributes": c.get("attributes", []),
                        "stats": c.get("stats", []), "bonuses": c.get("bonuses", []),
                        "subclass": c.get("subclass", {}),
                        "abilities": c.get("abilities") or abil.get(c.get("name"), []),
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
                          # blueprint → the /site/codexes/render stub below, so the
                          # featured-item icon is previewable locally.
                          "item": {"name": "Diamond Dragon Egg", "identifier": "item/diamond",
                                   "blueprint": "stub/diamond_dragon_egg"}},
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
                # A stretch of missed captures, plus a pair of holes that strand
                # a single bucket between them - so local preview exercises both
                # the dashed no-data bridge and the lone-point dot.
                if count // 3 <= i < count // 3 + max(2, count // 10) or i in (count - 5, count - 3):
                    continue
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
            # A stretch of buckets missing ENTIRELY (no captures at all) on top of
            # the per-class nulls below - the two hole kinds the chart has to tell
            # apart, both drawn as dashed no-data bridges.
            idxs = [i for i in range(count)
                    if not (count // 4 <= i < count // 4 + max(2, count // 12))]
            buckets = [start + i * bucket for i in idxs]
            classes = []
            for ci, name in enumerate(_STUB_CLASSES):
                vals, vals_clean = [], []
                for i in idxs:
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
        if (path == "/site/leaderboards/duplicates"
                or path.startswith("/site/leaderboards/duplicates?")):
            # Synthetic duplicate-name groups covering every verdict + cause so
            # the Possible-duplicates tab and its filter chips have something to
            # render locally. Mirrors the real payload shape exactly.
            def _series(slot, score, rank, moved):
                return {
                    "slot": slot, "captures": 24,
                    "first_anchor": STUB_ANCHOR - 86400,
                    "last_anchor": STUB_ANCHOR,
                    "first_score": score if moved else score,
                    "last_score": score, "last_rank": rank,
                    "moved": moved, "frozen": not moved,
                }

            def _group(name, kind, verdict, boards, occ, spellings=()):
                return {
                    "name": name, "kind": kind, "verdict": verdict,
                    "boards": len(boards), "max_occurrences": occ,
                    "spellings": list(spellings),
                    "first_anchor": STUB_ANCHOR - 7 * 86400,
                    "last_anchor": STUB_ANCHOR,
                    "method_version": 1, "updated_at": STUB_ANCHOR,
                    "evidence": {
                        "lookback_days": 7, "boards": boards,
                        "summary": (
                            f"Trove's own capture lists “{name}” {occ} times on the "
                            f"same board across {len(boards)} boards."),
                    },
                }
            duplicates = [
                _group("LateCom", "same_name", "one_live", [
                    {"uuid": 4, "name": "Quests Completed", "occurrences": 2,
                     "series": [_series(0, 1647674, 39, True),
                                _series(1, 192044, 2209, False)]},
                    {"uuid": 11, "name": "Glim Collected", "occurrences": 2,
                     "series": [_series(0, 8141459, 670, True),
                                _series(1, 2675220, 2019, False)]},
                ], 2),
                _group("FaeGoMyEggo", "same_name", "multi_live", [
                    {"uuid": 1, "name": "Trove Mastery", "occurrences": 2,
                     "series": [_series(0, 162986, 810, True),
                                _series(1, 98939, 4715, True)]},
                ], 2),
                _group("Robot", "case", "case_only", [], 2, ("Robot", "robot")),
            ]
            # Bulk filler so the tab has more rows than one page and the
            # "Load more" path is actually exercisable locally. Production has
            # thousands; the handful above stay first so every verdict + cause
            # is visible without paging.
            for i in range(120):
                kind = "case" if i % 5 == 0 else "same_name"
                verdict = "case_only" if kind == "case" else (
                    "not_analysed" if i % 3 else "all_idle")
                g = _group(f"Filler{i:03d}", kind, verdict, [], 2)
                if verdict == "not_analysed":
                    # Dated by the archive walk but never measured - no evidence.
                    g["evidence"] = {}
                    g["last_anchor"] = STUB_ANCHOR - (i + 2) * 86400   # historical
                duplicates.append(g)

            # Honour limit/offset/kind exactly as the real endpoint does, so the
            # pager and the server-side cause filter can be verified here. 'both'
            # satisfies either filter, matching pg_store.list_duplicates.
            # `path` has the query stripped (see `url = urlparse(self.path)`
            # above), so read it off `url` - parsing `path` yields nothing and
            # every page would silently return the same first slice.
            q = parse_qs(url.query)
            kind = (q.get("kind") or [""])[0]
            limit = max(1, min(int((q.get("limit") or ["50"])[0]), 200))
            offset = max(0, int((q.get("offset") or ["0"])[0]))
            if kind:
                duplicates = [d for d in duplicates
                              if d["kind"] == kind or d["kind"] == "both"]
            total = len(duplicates)
            current = sum(1 for d in duplicates if d["last_anchor"] == STUB_ANCHOR)
            return self._send_json({
                "enabled": True, "duplicates": duplicates[offset:offset + limit],
                "total": total, "current": current,
                "latest_anchor": STUB_ANCHOR,
                "limit": limit, "offset": offset, "method_version": 1,
            })
        if path.startswith("/site/leaderboards/duplicates/"):
            # Per-name lookup driving the player panel's shared-name banner.
            # Only the demo name is flagged; everyone else comes back clean,
            # which is the common case in production too.
            qname = unquote(path[len("/site/leaderboards/duplicates/"):])
            if qname.lower() != "latecom":
                return self._send_json(
                    {"query": qname, "found": False, "enabled": True})
            return self._send_json({
                "query": qname, "found": True, "enabled": True,
                "name": "LateCom", "kind": "same_name", "verdict": "one_live",
                "boards": 25, "max_occurrences": 2, "spellings": [],
                "first_anchor": STUB_ANCHOR - 7 * 86400,
                "last_anchor": STUB_ANCHOR, "method_version": 1,
                "updated_at": STUB_ANCHOR,
                "evidence": {
                    "lookback_days": 7, "boards": [],
                    "summary": (
                        "Trove's own capture lists “LateCom” 2 times on the same "
                        "board across 25 boards, and only one of those score "
                        "lines is still moving."),
                },
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
                                # Packed: depth 999 in 03:58 (see STUB_DELVE_ENTRIES).
                                "rank": 1, "score": 999.8517, "confidence": 0.94,
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
            delve = uuid in STUB_DELVE_UUIDS
            rows = _with_movement(
                STUB_DELVE_ENTRIES if delve else STUB_ENTRIES, delve=delve)
            page = rows[offset:offset + limit]
            return self._send_json({
                "uuid": uuid, "created_at": created_at,
                "items": page, "count": len(page), "total": len(rows),
                "comparison": {"comparable": True,
                               "prev_anchor": STUB_TIMESTAMPS[1], "reason": "ok"},
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
            _name_ids = {b["uuid"]: b["name_id"] for b in STUB_BOARDS}
            for i, (uuid, nm, cat) in enumerate(cat_boards):
                boards.append({
                    "leaderboard": uuid, "board_name": nm,
                    # The board's translation key - half of what the client
                    # matches on to tell a run-time board from a plain one.
                    "board_name_id": _name_ids.get(uuid),
                    "category": cat,
                    "best_rank": (i % 5) + 1, "latest_rank": (i % 7) + 1,
                    "latest_score": (235.46743369687553 if uuid in STUB_DELVE_UUIDS
                                     else 59736.0 - i * 850),
                    "appearances": 749 - i,
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

        if (path.startswith("/site/leaderboards/")
                and path.endswith("/history")
                and not path.startswith("/site/leaderboards/players/")):
            # Top-N trajectories for the per-board chart. Delve boards climb in
            # depth AND drift in run time, so their lines exercise the packed
            # reading in the tooltip and the depth-only y-axis.
            uuid = int(path.split("/")[3])
            _HR = 3600
            hanchors = [STUB_ANCHOR - (11 - i) * _HR for i in range(12)]
            delve = uuid in STUB_DELVE_UUIDS
            rows = STUB_DELVE_ENTRIES if delve else STUB_ENTRIES
            series = []
            for e in rows[:5]:
                pts, cur = [], e["score"] - (0.9 if delve else 900.0)
                for a in hanchors:
                    pts.append({"created_at": a, "rank": e["rank"],
                                "score": round(cur, 6), "synthetic": False})
                    cur += (0.09 if delve else 90.0)
                series.append({"player_name": e["player_name"],
                               "current_rank": e["rank"], "points": pts})
            return self._send_json({
                "uuid": uuid, "days": 7, "window_start": hanchors[0],
                "window_end": STUB_ANCHOR, "anchors": hanchors, "series": series,
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
                        "score": (235.46743369687553 - (0 if anchor == STUB_ANCHOR else 0.02)
                                  if b["uuid"] in STUB_DELVE_UUIDS
                                  else 59731.0 - i * 1200
                                  - (0 if anchor == STUB_ANCHOR else 300)),
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

            def _pts(base, rate, holes=()):
                # ``holes`` drops anchors from this series only - the board was
                # captured, this player just wasn't in it - so the chart's
                # dashed "no data" bridge shows up in local preview.
                out, cur = [], float(base)
                for i, a in enumerate(sanchors):
                    if i not in holes:
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
                    {"uuid": 2004, "name": "CHALLENGE: Deepest (WEEKLY)",
                     "points": _pts(230.4 + (seed % 10) * 0.01, 0.4,
                                    holes=(4, 5, 6))},
                ],
            })

        self.send_error(404)

    def _sound_studio_build(self):
        """Run the REAL editor (app.trove.audio.studio) against the local bank so
        the dev preview exercises the same rebuild production does."""
        import email
        import sys
        sys.path.insert(0, str(ROOT))
        from app.core.errors import APIError
        from app.trove.audio import studio

        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        header = "\n".join([
            "Content-Type: " + (self.headers.get("Content-Type") or ""),
            "MIME-Version: 1.0", "", "",
        ]).encode()
        message = email.message_from_bytes(header + raw)
        spec_text, clips = "{}", {}
        for part in message.walk():
            if part.get_content_maintype() == "multipart":
                continue
            name = part.get_param("name", header="content-disposition")
            payload = part.get_payload(decode=True) or b""
            if name == "spec":
                spec_text = payload.decode("utf-8", "replace")
            elif name == "clips":
                clips[part.get_filename() or ""] = payload
        try:
            spec = json.loads(spec_text)
        except ValueError:
            return self._send_json({"detail": "invalid spec"}, 400)

        bank = _bank_manifest()
        if not bank.get("raw"):
            return self._send_json({"detail": "no local bank - set TROVE_DEV_BNK"}, 503)
        try:
            result = studio.apply_edits(bank["raw"], spec, clips)
            blob, filename, media_type = studio.package(
                result, str(spec.get("path") or "audio/ui.bnk"), spec)
        except APIError as e:
            return self._send_json({"error": {"code": "bad_request", "message": e.message}},
                                   getattr(e, "status_code", 400))
        except Exception as e:      # noqa: BLE001 - dev server: surface, never crash
            return self._send_json({"error": {"code": "bad_request", "message": str(e)}}, 400)
        self.send_response(200)
        self.send_header("Content-Type", media_type)
        self.send_header("Content-Length", str(len(blob)))
        self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
        self.end_headers()
        self.wfile.write(blob)

    def _unlock_debug(self):
        """The REAL patcher (the constants + rules from app.site.router), so the dev
        preview rejects the same files production does instead of always succeeding."""
        import email
        import sys
        sys.path.insert(0, str(ROOT))
        from app.site.router import _DEBUG_FIND, _DEBUG_REPL

        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        header = "\n".join([
            "Content-Type: " + (self.headers.get("Content-Type") or ""),
            "MIME-Version: 1.0", "", "",
        ]).encode()
        message = email.message_from_bytes(header + raw)
        data = b""
        for part in message.walk():
            if part.get_content_maintype() == "multipart":
                continue
            if part.get_param("name", header="content-disposition") == "trove_exe":
                data = part.get_payload(decode=True) or b""

        def bad(text):
            return self._send_json({"error": {"code": "bad_request", "message": text}}, 400)

        if not data:
            return bad("No file was uploaded.")
        if data[:2] != b"MZ":
            return bad("That doesn't look like a Windows executable.")
        if _DEBUG_FIND not in data:
            return bad("This build of Trove.exe doesn't contain the sequence this patch "
                       "edits - it may already be patched, or the game may have changed.")
        return self._send_binary(data.replace(_DEBUG_FIND, _DEBUG_REPL),
                                 "Trove.exe", "application/octet-stream")

    def _send_binary(self, blob, filename, media_type, extra=None):
        self.send_response(200)
        self.send_header("Content-Type", media_type)
        self.send_header("Content-Length", str(len(blob)))
        self.send_header("Content-Disposition", f'attachment; filename="{filename}"')
        for key, value in (extra or {}).items():
            self.send_header(key, str(value))
        self.end_headers()
        self.wfile.write(blob)

    def _read_multipart(self):
        """``(fields, files)`` from a multipart body - files as ``[(name, filename,
        bytes)]`` in the order they arrived, which is what the workshop's paths list
        is aligned to."""
        import email
        length = int(self.headers.get("Content-Length", 0) or 0)
        raw = self.rfile.read(length) if length else b""
        head = "\n".join(["Content-Type: " + (self.headers.get("Content-Type") or ""),
                          "MIME-Version: 1.0", "", ""]).encode()
        message = email.message_from_bytes(head + raw)
        fields, files = {}, []
        for part in message.walk():
            if part.get_content_maintype() == "multipart":
                continue
            name = part.get_param("name", header="content-disposition") or ""
            payload = part.get_payload(decode=True) or b""
            filename = part.get_filename()
            if filename:
                files.append((name, filename, payload))
            else:
                fields[name] = payload.decode("utf-8", "replace")
        return fields, files

    def _blueprint_editor(self, path):
        """Run the REAL blueprint editor engine (app.trove.blueprint.editor), which is
        pure Python with no database behind it - so the dev preview decodes and writes
        exactly what production does."""
        import sys
        sys.path.insert(0, str(ROOT))
        from app.trove.blueprint import editor as bp_editor

        fields, files = self._read_multipart()
        blobs = {name: (filename, data) for name, filename, data in files}

        if path == "/site/blueprint-editor/import-qb":
            parts = {fn: d for nm, fn, d in files if nm == "files" and fn}
            if not parts:
                return self._send_json({"detail": "No .qb files were sent."}, 400)
            try:
                out, summary = bp_editor.import_qb(parts)
            except bp_editor.EditorError as e:
                return self._send_json({"detail": str(e)}, 400)
            self.send_response(200)
            self.send_header("Content-Type", "application/octet-stream")
            self.send_header("Content-Length", str(len(out)))
            self.send_header("X-Kiwi-Summary", json.dumps(summary))
            self.send_header("Cache-Control", "no-store")
            self.end_headers()
            self.wfile.write(out)
            return None

        if path in ("/site/blueprint-editor/model", "/site/blueprint-editor/model-save"):
            return self._blueprint_model(path, fields, files, blobs)

        if "file" not in blobs:
            return self._send_json({"detail": "No file was sent."}, 400)
        name, data = blobs["file"]
        try:
            if path == "/site/blueprint-editor/inspect":
                return self._send_json(bp_editor.inspect(data, name=name or "blueprint"))
            if path == "/site/blueprint-editor/check":
                return self._send_json(bp_editor.check(
                    data, json.loads(fields.get("edits") or "[]"),
                    fields.get("kind") or "other",
                    [d for nm, fn, d in files if nm == "layers"],
                    json.loads(fields.get("stack") or "[]"),
                    int(fields.get("anchor_at") or 0)))
            if path == "/site/blueprint-editor/flatten":
                out, summary = bp_editor.composite(
                    data, json.loads(fields.get("edits") or "[]"),
                    [d for nm, fn, d in files if nm == "layers"],
                    json.loads(fields.get("stack") or "[]"),
                    int(fields.get("anchor_at") or 0))
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(out)))
                self.send_header("X-Kiwi-Summary", json.dumps(summary))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(out)
                return None
            if path == "/site/blueprint-editor/transform":
                out, summary = bp_editor.transform(
                    data, json.loads(fields.get("edits") or "[]"),
                    json.loads(fields.get("ops") or "[]"))
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(out)))
                self.send_header("X-Kiwi-Summary", json.dumps(summary))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(out)
                return None
            if path == "/site/blueprint-editor/export-qb":
                stem = name[:-len(".blueprint")] if name.lower().endswith(".blueprint") else name
                archive, summary = bp_editor.export_qb(
                    data, json.loads(fields.get("edits") or "[]"),
                    [d for nm, fn, d in files if nm == "layers"],
                    json.loads(fields.get("stack") or "[]"),
                    int(fields.get("anchor_at") or 0), stem=stem or "model")
                self.send_response(200)
                self.send_header("Content-Type", "application/zip")
                self.send_header("Content-Length", str(len(archive)))
                self.send_header("X-Kiwi-Notes", json.dumps(summary["notes"]))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(archive)
                return None
            if path == "/site/blueprint-editor/save":
                out, summary = bp_editor.composite(
                    data, json.loads(fields.get("edits") or "[]"),
                    [d for nm, fn, d in files if nm == "layers"],
                    json.loads(fields.get("stack") or "[]"),
                    int(fields.get("anchor_at") or 0))
                self.send_response(200)
                self.send_header("Content-Type", "application/octet-stream")
                self.send_header("Content-Length", str(len(out)))
                self.send_header("Content-Disposition", f'attachment; filename="{name}"')
                self.send_header("X-Kiwi-Recoloured", str(summary["recoloured"]))
                self.send_header("X-Kiwi-Rematerialised", str(summary["rematerialised"]))
                self.send_header("X-Kiwi-Ignored", str(summary["ignored"]))
                self.send_header("Cache-Control", "no-store")
                self.end_headers()
                self.wfile.write(out)
                return None
        except bp_editor.EditorError as e:
            return self._send_json({"detail": str(e)}, 400)
        except (ValueError, KeyError) as e:
            return self._send_json({"detail": f"{type(e).__name__}: {e}"}, 400)
        return self._send_json({"detail": "unknown endpoint"}, 404)

    def _blueprint_model(self, path, fields, files, blobs):
        """Model projects - a whole creature open at once - on the real engine
        (app.trove.blueprint.model).

        One thing differs from production and it matters: WHERE THE PARTS GO. The API
        resolves the rig from the game's own prefab bindings, which live in Postgres;
        there is none here, so the preview falls back to matching part names against the
        baked rigs by suffix. That is a GUESS, and the real resolver refuses to make it.
        It exists so the assembled view can be looked at locally - nothing under app/
        ever calls it."""
        import sys
        sys.path.insert(0, str(ROOT))
        from app.trove.blueprint import editor as bp_editor
        from app.trove.blueprint import model as bp_model

        def dev_match(basenames):
            from app.trove.mods_hub import assembly
            best, best_hits = None, {}
            for rig_name, rig in assembly._rigs().items():
                aps = sorted(rig["rest"], key=len, reverse=True)   # longest suffix wins
                hits = {}
                for b in basenames:
                    for ap in aps:
                        if b == ap or b.endswith("_" + ap):
                            hits[b] = ap
                            break
                if len(hits) > len(best_hits):
                    best, best_hits = rig_name, hits
            return (best, best_hits) if best_hits else (None, {})

        try:
            if "file" in blobs:
                name, data = blobs["file"]
                kind, props, unpacked = bp_model.unpack(data, name)
            else:
                loose = [(fn.replace("\\", "/").rsplit("/", 1)[-1], d)
                         for nm, fn, d in files if nm == "files" and fn]
                if not loose:
                    return self._send_json({"detail": "Send a .tmod, .zip or blueprints."}, 400)
                kind, props, unpacked, name = "files", {}, loose, "model"

            if path == "/site/blueprint-editor/model":
                blueprints = bp_model.parts_of(unpacked)
                skeleton, attach = dev_match(
                    [bp_model.basename_of(p) for p, _ in blueprints])
                payload = bp_model.open_project(unpacked, rig_name=skeleton,
                                                attach=attach, name=name)
                payload["source"] = kind
                return self._send_json(payload)

            extra = []
            if "file" in blobs:
                paths = json.loads(fields.get("paths") or "[]")
                posted = [(fn, d) for nm, fn, d in files if nm == "files" and fn]
                for (fn, d), want in zip(posted, paths, strict=False):
                    extra.append((bp_model.pack_path(str(want) or fn), d))
            edited, summary = bp_model.apply_project(
                unpacked, json.loads(fields.get("edits") or "{}"), extra,
                json.loads(fields.get("moves") or "{}"))
            out, ext = bp_model.repack(kind, props, edited)
        except bp_editor.EditorError as e:
            return self._send_json({"detail": str(e)}, 400)
        except (ValueError, KeyError) as e:
            return self._send_json({"detail": f"{type(e).__name__}: {e}"}, 400)

        stem = re.sub(r"\.(tmod|zip)$", "", name, flags=re.I) or "model"
        self.send_response(200)
        self.send_header("Content-Type", "application/octet-stream")
        self.send_header("Content-Length", str(len(out)))
        self.send_header("Content-Disposition", f'attachment; filename="{stem}.{ext}"')
        self.send_header("X-Kiwi-Summary", json.dumps(summary))
        self.send_header("Cache-Control", "no-store")
        self.end_headers()
        self.wfile.write(out)
        return None

    def _mod_workshop(self, path):
        """Run the REAL Mod Workshop engine (app.trove.mods_hub.workshop) so the dev
        preview compiles and unpacks exactly what production does. The one thing it
        can't do here is the game-file lookup that says where a misplaced file
        belongs - that needs the updates archive - so placement degrades to Trove's
        folder rules, the same as it does on a server whose archive is empty."""
        import asyncio
        import sys
        sys.path.insert(0, str(ROOT))
        from app.trove.mods_hub import workshop

        fields, files = self._read_multipart()
        blobs = {name: (filename, data) for name, filename, data in files}
        try:
            if path == "/site/mod-workshop/inspect":
                if "archive" in blobs:
                    name, data = blobs["archive"]
                    kind, header, entries = workshop.read_archive(data, name)
                    names = [p for p, _ in entries]
                    sizes = {i: len(b) for i, (_, b) in enumerate(entries)}
                else:
                    kind, header, entries, sizes = "files", {}, None, {}
                    names = json.loads(fields.get("paths") or "[]")
                plan = asyncio.run(workshop.preview(names))
                plan.pop("mapping", None)
                plan["config_candidates"] = workshop.config_candidates(names)
                plan["preview_candidates"] = workshop.preview_candidates(names)
                if entries is not None:
                    for entry in plan["entries"]:
                        entry["size"] = sizes.get(entry["index"], 0)
                return self._send_json({**plan, "source": kind, "properties": header})

            if path == "/site/mod-workshop/build":
                spec = json.loads(fields.get("spec") or "{}")
                if "archive" in blobs:
                    name, data = blobs["archive"]
                    _, header, source = workshop.read_archive(data, name)
                    page = spec.get("properties") or {}
                    props = {**header,
                             **{k: v for k, v in page.items() if v or k == "tags"}}
                else:
                    names = json.loads(fields.get("paths") or "[]")
                    source = [(str(n), d) for n, (_, _, d)
                              in zip(names, [f for f in files if f[0] == "files"],
                                     strict=True)]
                    props = spec.get("properties") or {}
                config_path = spec.get("config_path") or ""
                preview_path = spec.get("preview_path") or ""
                attached = []
                for part, is_config in (("config", True), ("preview", False)):
                    if part not in blobs:
                        continue
                    name = workshop.norm_path(blobs[part][0])
                    attached.append((name, blobs[part][1]))
                    if is_config:
                        config_path = name
                    else:
                        preview_path = name
                artifact, plan = asyncio.run(workshop.build_mod(
                    source, props, fix=bool(spec.get("fix", True)),
                    keep=spec.get("keep") or [], config_path=config_path,
                    preview_path=preview_path, attached=attached))
                return self._send_binary(
                    artifact, f"{workshop.safe_title(props.get('title'))}.tmod",
                    "application/octet-stream",
                    {"X-Kiwi-Packed": plan["packed"],
                     "X-Kiwi-Moved": plan["counts"]["moved"],
                     "X-Kiwi-Skipped": plan["counts"]["skipped"]})

            if path == "/site/mod-workshop/extract":
                info = workshop.describe(blobs["file"][1])
                plan = asyncio.run(workshop.plan([f["path"] for f in info["files"]], fix=False))
                sizes = {i: f["size"] for i, f in enumerate(info["files"])}
                for entry in plan["entries"]:
                    entry["size"] = sizes.get(entry["index"], 0)
                plan.pop("mapping", None)
                return self._send_json({**plan, **info})

            if path == "/site/mod-workshop/extract/download":
                _, entries = workshop.read_mod(blobs["file"][1])
                wanted = workshop.norm_path(fields.get("path") or "")
                if wanted:
                    match = next((b for p, b in entries if p == wanted), None)
                    if match is None:
                        return self._send_json({"detail": "no such file"}, 404)
                    return self._send_binary(match, wanted.rsplit("/", 1)[-1],
                                             "application/octet-stream")
                stem = blobs["file"][0].rsplit(".", 1)[0]
                return self._send_binary(workshop.to_zip(entries), f"{stem}.zip",
                                         "application/zip")
        except workshop.WorkshopError as e:
            return self._send_json({"error": {"code": "bad_request", "message": str(e)}}, 400)
        except Exception as e:      # noqa: BLE001 - dev server: surface, never crash
            return self._send_json({"error": {"code": "bad_request", "message": str(e)}}, 400)
        return self._send_json({"detail": "not found"}, 404)

    def do_POST(self):
        url = urlparse(self.path)
        path = url.path

        if path.startswith(_AUTH_PREFIX):
            return self._proxy_auth()

        if path == "/site/sound-studio/build":
            return self._sound_studio_build()

        if path == "/site/unlock-debug":
            return self._unlock_debug()

        if path.startswith("/site/drops/"):
            # Accept whatever the form sends so the PIN step + the upload can be
            # walked through locally. Nothing is stored and no PIN is checked -
            # that is the API's job, and this server has no drops to check against.
            length = int(self.headers.get("Content-Length", 0) or 0)
            if length:
                self.rfile.read(length)
            if path.endswith("/upload"):
                return self._send_json({
                    "id": "preview", "filename": "preview.bin", "size": length,
                    "content_type": None, "sha256": "0" * 64, "note": None,
                    "uploaded_at": "1970-01-01T00:00:00Z",
                })
            return self._send_json({
                "label": "Send me your Trove log",
                "max_file_bytes": 256 * 1024 * 1024,
                "uploads_left": 1,
                "expires_at": "2099-01-01T00:00:00Z",
            })

        if path.startswith("/site/mod-workshop/"):
            return self._mod_workshop(path)

        if path.startswith("/site/blueprint-editor/"):
            return self._blueprint_editor(path)

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

    def _send_file(self, p: Path, content_type: str | None, extra_ctx: dict | None = None):
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
        if content_type == "text/html" and p.parent == TEMPLATES:
            data = _render_page_template(p, self.path.split("?", 1)[0], extra_ctx)
        self.send_response(200)
        self.send_header("Content-Type", content_type)
        self.send_header("Content-Length", str(len(data)))
        self.send_header("Cache-Control", "no-store")
        if content_type == "text/html" and _SITE_CSP:
            self.send_header("Content-Security-Policy", _SITE_CSP)
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
