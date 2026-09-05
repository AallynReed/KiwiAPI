"""Master-flippable feature toggles.

A whole site feature (the Mods Hub, the Market, the Leaderboards, …) can be
hidden completely without a code change or restart - it's a ``bool`` in the
runtime-config registry (``app/admin/runtime_config.py``), flippable from the
dev-portal Configuration tab's "features" category. When OFF:
  - the navbar entry is hidden (a Jinja context flag the site router injects),
  - the page routes + ``/site/<feature>/*`` proxies 404 (gated in app/site/router.py),
  - the ``/v1/<feature>/*`` (and git) routers 404 via the dependencies here.
Stored data is untouched and the feature reappears intact when toggled back ON.

Two of the flags here (``CHEATER_DETECTION_FLAG`` / ``ALT_CLUSTERS_FLAG``) don't
hide a page - they gate the *calculation* of the Possible-cheaters / Alt-cluster
analysis in the leaderboards warmer (see app/trove/leaderboards/detection.py).
They're INDEPENDENT: cheater detection gates the per-player checks, alt-clusters
gates the cluster pass, and either can run without the other (only when both are
OFF does the warmer skip the compute entirely). They still live under the same
"features" category so all the master switches sit together.
"""

from collections.abc import Callable, Coroutine
from typing import Any

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode

# ── Page / API features (hide a whole feature, 404 its endpoints) ──────────
MODS_HUB_FLAG = "feature_mods_hub_enabled"
MARKET_FLAG = "feature_market_enabled"
LEADERBOARDS_FLAG = "feature_leaderboards_enabled"
PLAYER_ACTIVITY_FLAG = "feature_player_activity_enabled"
CLASS_ACTIVITY_FLAG = "feature_class_activity_enabled"
CLUBS_FLAG = "feature_clubs_enabled"
UPDATES_FLAG = "feature_updates_enabled"
CODEXES_FLAG = "feature_codexes_enabled"
SERVER_STATUS_FLAG = "feature_server_status_enabled"
GIVEAWAYS_FLAG = "feature_giveaways_enabled"
COMMANDS_FLAG = "feature_commands_enabled"
SERVER_TIME_FLAG = "feature_server_time_enabled"
WEBHOOKS_FLAG = "feature_webhooks_enabled"
IMAGE_STUDIO_FLAG = "feature_image_studio_enabled"
CALENDAR_FLAG = "feature_calendar_enabled"
STREAMS_FLAG = "feature_streams_enabled"
BTT_RELEASES_FLAG = "feature_btt_releases_enabled"
CLASSES_FLAG = "feature_classes_enabled"
STAR_CHART_FLAG = "feature_star_chart_enabled"
GEM_SIMULATOR_FLAG = "feature_gem_simulator_enabled"
GEM_EVALUATOR_FLAG = "feature_gem_evaluator_enabled"
GEM_BUILDS_FLAG = "feature_gem_builds_enabled"
CALCULATORS_FLAG = "feature_calculators_enabled"
GEMS_GUIDE_FLAG = "feature_gems_guide_enabled"
ABILITIES_FLAG = "feature_abilities_enabled"
GUIDES_FLAG = "feature_guides_enabled"
ALLIES_FLAG = "feature_allies_enabled"
GEM_TOOLS_FLAG = "feature_gem_tools_enabled"
FISHING_GUIDE_FLAG = "feature_fishing_guide_enabled"
DM_SUBS_FLAG = "feature_dm_subscriptions_enabled"
DELVES_FLAG = "feature_delves_enabled"
STORE_FLAG = "feature_store_enabled"
EMBED_FLAG = "feature_embed_viewer_enabled"
DRESSING_ROOM_FLAG = "feature_dressing_room_enabled"
# The Dressing Room is the one feature whose data plane is also a PARTNER API
# (other sites call /v1/dressing/* and /site/dressing/render for their own
# dressing rooms), so its website page has its own switch: page OFF + master ON
# unpublishes /dressing-room while every partner keeps working. Gated in
# app/site/feature_map.py (the page route + navbar + search + sitemap), not here -
# there is no endpoint that belongs to the page alone.
DRESSING_ROOM_PAGE_FLAG = "feature_dressing_room_page_enabled"
SOUND_STUDIO_FLAG = "feature_sound_studio_enabled"
MOD_WORKSHOP_FLAG = "feature_mod_workshop_enabled"
BLUEPRINT_EDITOR_FLAG = "feature_blueprint_editor_enabled"
TOMES_FLAG = "feature_tomes_enabled"
UNLOCK_DEBUG_FLAG = "feature_unlock_debug_enabled"
# Mod issues + requests. Site-wide kill switch; per-mod consent is a field on the
# project (ModProject.issues_enabled) and stays the creator's call.
MOD_ISSUES_FLAG = "feature_mod_issues_enabled"
# File drops: master-minted one-off upload links (/drop/<slug>). Nothing here is
# linked from the site - the switch exists so the upload surface can be closed
# outright without deleting the links that are already out there.
FILE_DROPS_FLAG = "feature_file_drops_enabled"

# ── Calculation switches (gate compute, not a page) ───────────────────────
CHEATER_DETECTION_FLAG = "feature_cheater_detection_enabled"
ALT_CLUSTERS_FLAG = "feature_alt_clusters_enabled"
# Player-rename detection: gates the warmer's live rename pass + the dev-portal
# backfill + the Possible-renames tab (a tab on /leaderboards, not its own page).
RENAMES_FLAG = "feature_leaderboard_renames_enabled"
# Duplicate-name detection: gates the warmer's live pass + the dev-portal backfill
# + the Possible-duplicates tab. The per-series SPLIT in the read paths is NOT
# gated - it is a correctness fix for the chart/deltas, not a feature.
DUPLICATES_FLAG = "feature_leaderboard_duplicates_enabled"


async def is_enabled(flag: str) -> bool:
    """Resolved value of a feature flag (runtime override, else default)."""
    return bool(await runtime_config.get_setting(flag))


def _disabled(name: str) -> APIError:
    # 404 (not 403/503) so a disabled feature is indistinguishable from one that
    # never existed - it's "hidden", not "forbidden".
    return APIError(404, ErrorCode.not_found, f"The {name} is currently disabled.")


def _gate(flag: str, name: str) -> Callable[[], Coroutine[Any, Any, None]]:
    """Build a FastAPI dependency that 404s the route when ``flag`` is OFF.

    Used as ``dependencies=[Depends(require_<x>_enabled)]`` on the ``/v1``
    routers (and individual endpoints) so a disabled feature's API surface
    vanishes alongside its website page."""
    async def dependency() -> None:
        if not await is_enabled(flag):
            raise _disabled(name)
    dependency.__name__ = f"require_{flag}"
    # Marker read by main.custom_openapi() to prune this operation from the
    # OpenAPI reference while the flag is OFF (a disabled feature shouldn't
    # merely 404 - it should vanish from the docs too).
    dependency._feature_flag = flag  # type: ignore[attr-defined]
    return dependency


require_mods_hub_enabled = _gate(MODS_HUB_FLAG, "Mods Hub")
require_market_enabled = _gate(MARKET_FLAG, "Market")
require_leaderboards_enabled = _gate(LEADERBOARDS_FLAG, "Leaderboards")
require_player_activity_enabled = _gate(PLAYER_ACTIVITY_FLAG, "Player Activity")
require_class_activity_enabled = _gate(CLASS_ACTIVITY_FLAG, "Class Activity")
require_updates_enabled = _gate(UPDATES_FLAG, "Updates archive")
require_codexes_enabled = _gate(CODEXES_FLAG, "Codexes")
require_server_status_enabled = _gate(SERVER_STATUS_FLAG, "Server Status")
require_giveaways_enabled = _gate(GIVEAWAYS_FLAG, "Giveaways")
require_webhooks_enabled = _gate(WEBHOOKS_FLAG, "Webhooks")
require_image_studio_enabled = _gate(IMAGE_STUDIO_FLAG, "Image Studio")
require_dm_subs_enabled = _gate(DM_SUBS_FLAG, "DM subscriptions")
require_delves_enabled = _gate(DELVES_FLAG, "Delve rotation data")
require_store_enabled = _gate(STORE_FLAG, "Store catalog")
require_leaderboard_renames_enabled = _gate(RENAMES_FLAG, "Rename detection")
require_leaderboard_duplicates_enabled = _gate(
    DUPLICATES_FLAG, "Duplicate-name detection")
require_embed_enabled = _gate(EMBED_FLAG, "embeddable viewer")
require_dressing_room_enabled = _gate(DRESSING_ROOM_FLAG, "Dressing Room")
require_tomes_enabled = _gate(TOMES_FLAG, "Tomes")
require_unlock_debug_enabled = _gate(UNLOCK_DEBUG_FLAG, "Unlock Debug patcher")
require_mod_issues_enabled = _gate(MOD_ISSUES_FLAG, "mod issues")
require_file_drops_enabled = _gate(FILE_DROPS_FLAG, "file drops")
