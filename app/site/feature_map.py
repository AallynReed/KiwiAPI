"""DB-free showcase-site metadata shared by the API and the website containers.

The API container (``app.main``) and the website container (``app.web.main``)
both need the site feature-flag map, the path→feature gate, the sitemap page
list, and the host-aware robots body. None of these touch a database — they're
pure constants + functions over config — so they live here, importable by the
DB-less website app without dragging in ``app.trove.*`` service modules.

The *resolution* of the flags (``feature_flags.is_enabled`` → runtime_config, a
DB read) stays out of this module: the API resolves them in-process
(``app/site/router.py``); the website fetches the resolved map over HTTP
(``app/web/feature_flags.py``). This module only defines the SHAPE.
"""

from app.core import features as feature_flags
from app.core.config import settings
from app.core.middleware import API_SIDE_HOSTS

# Site features surfaced to templates: template context key (also the
# ``request.state`` attr) → runtime-config flag. Each is a master switch the
# admin Configuration tab flips. The API resolves these once per request and
# injects them into every template so the navbar can hide a disabled feature's
# link; the website fetches the same map from ``GET /site/feature-flags``.
SITE_FEATURE_FLAGS = {
    "mods_hub_enabled": feature_flags.MODS_HUB_FLAG,
    "market_enabled": feature_flags.MARKET_FLAG,
    "store_enabled": feature_flags.STORE_FLAG,
    "leaderboards_enabled": feature_flags.LEADERBOARDS_FLAG,
    "player_activity_enabled": feature_flags.PLAYER_ACTIVITY_FLAG,
    "class_activity_enabled": feature_flags.CLASS_ACTIVITY_FLAG,
    "clubs_enabled": feature_flags.CLUBS_FLAG,
    "updates_enabled": feature_flags.UPDATES_FLAG,
    "codexes_enabled": feature_flags.CODEXES_FLAG,
    "server_status_enabled": feature_flags.SERVER_STATUS_FLAG,
    "giveaways_enabled": feature_flags.GIVEAWAYS_FLAG,
    "commands_enabled": feature_flags.COMMANDS_FLAG,
    "server_time_enabled": feature_flags.SERVER_TIME_FLAG,
    "webhooks_enabled": feature_flags.WEBHOOKS_FLAG,
    "dm_subscriptions_enabled": feature_flags.DM_SUBS_FLAG,
    "image_studio_enabled": feature_flags.IMAGE_STUDIO_FLAG,
    "calendar_enabled": feature_flags.CALENDAR_FLAG,
    "streams_enabled": feature_flags.STREAMS_FLAG,
    "btt_releases_enabled": feature_flags.BTT_RELEASES_FLAG,
    "classes_enabled": feature_flags.CLASSES_FLAG,
    "star_chart_enabled": feature_flags.STAR_CHART_FLAG,
    "gem_simulator_enabled": feature_flags.GEM_SIMULATOR_FLAG,
    "gem_evaluator_enabled": feature_flags.GEM_EVALUATOR_FLAG,
    "gem_builds_enabled": feature_flags.GEM_BUILDS_FLAG,
    "calculators_enabled": feature_flags.CALCULATORS_FLAG,
    "gems_guide_enabled": feature_flags.GEMS_GUIDE_FLAG,
    "abilities_enabled": feature_flags.ABILITIES_FLAG,
    "guides_enabled": feature_flags.GUIDES_FLAG,
    "fishing_guide_enabled": feature_flags.FISHING_GUIDE_FLAG,
    "dressing_room_enabled": feature_flags.DRESSING_ROOM_FLAG,
    "dressing_room_page_enabled": feature_flags.DRESSING_ROOM_PAGE_FLAG,
    "sound_studio_enabled": feature_flags.SOUND_STUDIO_FLAG,
    "mod_workshop_enabled": feature_flags.MOD_WORKSHOP_FLAG,
    "blueprint_editor_enabled": feature_flags.BLUEPRINT_EDITOR_FLAG,
    "tomes_enabled": feature_flags.TOMES_FLAG,
    "unlock_debug_enabled": feature_flags.UNLOCK_DEBUG_FLAG,
    "file_drops_enabled": feature_flags.FILE_DROPS_FLAG,
}


def apply_derived(flags: dict) -> dict:
    """Fold the flags that depend on another flag, in place.

    Only the Dressing Room needs this: its page has a switch of its own, but a page
    whose system is off is off too. Applied where the map is RESOLVED, so the 404
    gate, the navbar, search and the sitemap all read one honest value instead of
    each re-deriving it."""
    flags["dressing_room_page_enabled"] = bool(
        flags.get("dressing_room_page_enabled", True)
        and flags.get("dressing_room_enabled", True))
    return flags


def feature_blocks(p: str, f: dict) -> bool:
    """True if the request path ``p`` belongs to a feature that is OFF (``f`` is
    the resolved flag map). Covers both the page route and that feature's
    ``/site/<feature>/*`` JSON proxies + OG images, so a disabled feature is
    hidden, not just unlinked."""
    # Mods Hub + Modpacks ride the Mods Hub toggle (modpacks are a layer over it).
    if not f["mods_hub_enabled"] and (
        p == "/mods" or p.startswith("/mods/") or p.startswith("/site/mods/")
        or p == "/modpacks" or p.startswith("/modpacks/")
        or p.startswith("/site/modpacks/")
    ):
        return True
    if not f["market_enabled"] and (p == "/market" or p.startswith("/site/market/")):
        return True
    if not f["store_enabled"] and (p == "/store" or p.startswith("/site/store/")):
        return True
    # Leaderboards: board browser + per-player profile pages. The activity /
    # class-activity proxies share the /site/leaderboards/ root but have their
    # own toggles, so they're explicitly excluded here.
    if not f["leaderboards_enabled"] and (
        p == "/leaderboards"
        or p.startswith("/player/")
        or (p.startswith("/site/leaderboards/")
            and not p.startswith("/site/leaderboards/activity")
            and not p.startswith("/site/leaderboards/class-activity"))
    ):
        return True
    if not f["player_activity_enabled"] and (
        p == "/activity" or p.startswith("/activity/")           # page + /activity/og.png
        or p.startswith("/site/leaderboards/activity")
    ):
        return True
    if not f["class_activity_enabled"] and (
        p == "/class-activity"
        or p.startswith("/site/leaderboards/class-activity")
    ):
        return True
    if not f["clubs_enabled"] and (p == "/clubs" or p == "/site/clubs"):
        return True
    if not f["updates_enabled"] and (p == "/updates" or p.startswith("/site/updates/")):
        return True
    if not f["codexes_enabled"] and (
        p == "/codexes" or p == "/codexes/crafting" or p.startswith("/site/codexes/")
    ):
        return True
    if not f["server_status_enabled"] and (
        p == "/status" or p.startswith("/status/")               # page + /status/og.png
        or p.startswith("/site/trove-status")
    ):
        return True
    if not f["giveaways_enabled"] and (p == "/giveaways" or p == "/site/giveaways"):
        return True
    if not f["commands_enabled"] and p == "/commands":
        return True
    if not f["server_time_enabled"] and (
        p == "/server-time" or p == "/site/server-time"
    ):
        return True
    if not f["calendar_enabled"] and (
        p == "/calendar" or p.startswith("/site/calendar")
    ):
        return True
    if not f["streams_enabled"] and p == "/streams":
        return True
    if not f["btt_releases_enabled"] and (
        p == "/releases" or p.startswith("/site/btt")
    ):
        return True
    if not f["classes_enabled"] and (
        p == "/classes" or p.startswith("/site/stats/classes")
    ):
        return True
    # Star Chart is fully client-rendered from the static /static/star_chart.json
    # asset (no /site proxy, no /v1 API), so only the page route needs blocking.
    if not f["star_chart_enabled"] and p == "/star-chart":
        return True
    # Sound Studio: the page plus its build endpoint. It reads banks through the
    # updates archive, so turning /updates off takes its source data with it.
    if not f["sound_studio_enabled"] and (
        p == "/sound-studio" or p.startswith("/site/sound-studio")
    ):
        return True
    # Mod Workshop: the page plus its stateless compile/unpack endpoints. Its
    # placement check reads the game's file tree from the updates archive, but it
    # degrades to the pure path rules without it, so it doesn't ride /updates.
    if not f["mod_workshop_enabled"] and (
        p == "/mod-workshop" or p.startswith("/site/mod-workshop")
    ):
        return True
    # Blueprint Editor: the page plus its stateless inspect/save endpoints. Wholly
    # self-contained (it only needs the file the visitor opens), so it rides no other
    # feature's toggle.
    if not f["blueprint_editor_enabled"] and (
        p == "/blueprint-editor" or p.startswith("/site/blueprint-editor")
    ):
        return True
    # Gem Simulator is likewise fully client-rendered (static /static/gem-engine.js,
    # no /site proxy, no /v1 API), so only the page route needs blocking.
    if not f["gem_simulator_enabled"] and p == "/gem-simulator":
        return True
    # Gem Evaluator: page + its evaluate / stat-range / lookups proxies.
    if not f["gem_evaluator_enabled"] and (
        p == "/gem-evaluator"
        or p in ("/site/gems/evaluate", "/site/gems/evaluate-simple",
                 "/site/gems/stat-range", "/site/gems/lookups")
    ):
        return True
    # Gem Builds: page + its builds/* proxies.
    if not f["gem_builds_enabled"] and (
        p == "/gem-builds" or p.startswith("/site/gems/builds")
    ):
        return True
    if not f["calculators_enabled"] and p == "/calculators":
        return True
    # Dressing Room, the one feature split in two: the master flag governs the
    # SYSTEM (its /site + /v1 data, which partner sites call for their own dressing
    # rooms), the page flag only our own page. Page OFF + master ON = we stop
    # publishing /dressing-room and every partner keeps working. The page attr is
    # already false whenever the master is (see ``apply_derived``).
    if not f["dressing_room_page_enabled"] and p == "/dressing-room":
        return True
    if not f["dressing_room_enabled"] and p.startswith("/site/dressing/"):
        return True
    # Gems guide is a fully client-rendered explainer (static JS, no proxy or
    # /v1 API), so only the page route needs blocking.
    if not f["abilities_enabled"] and p in ("/abilities", "/gem-abilities"):
        return False
    if not f["guides_enabled"] and p == "/guides":
        return False
    if not f["gems_guide_enabled"] and p == "/gems-guide":
        return True
    # Same shape as the gems guide: a client-rendered explainer whose only read
    # is the shared codex render endpoint, so only the page route is blocked.
    if not f["fishing_guide_enabled"] and p == "/fishing-guide":
        return True
    # Tomes: the page plus its valuation proxy. It prices payouts from market
    # medians, but degrades to "not evaluated" without them, so it does not ride
    # the /market toggle.
    if not f["tomes_enabled"] and (p == "/tomes" or p == "/site/tomes"):
        return True
    # Unlock Debug: the page, its legacy underscore URL, and the patch endpoint.
    # Ships OFF by default (see the runtime-config description), so this gate is
    # what most deployments actually hit.
    if not f["unlock_debug_enabled"] and (
        p in ("/unlock-debug", "/unlock_debug", "/site/unlock-debug")
    ):
        return True
    # File drops: the uploader's page and its endpoints. Unlisted by nature (no
    # navbar entry, no sitemap, no search) - the switch is how a live link is
    # closed off without deleting it.
    if not f["file_drops_enabled"] and (
        p.startswith("/drop/") or p.startswith("/site/drops/")
    ):
        return True
    # The star-chart preview proxy feeds both Builds and Calculators; only hide it
    # when both of those features are OFF.
    if (not f["calculators_enabled"] and not f["gem_builds_enabled"]
            and p == "/site/gems/parse-star-chart"):
        return True
    return False


# Public, indexable STATIC site pages for the sitemap: (path, feature attr that
# must be truthy; ``None`` = always on). The attr is a key of
# ``SITE_FEATURE_FLAGS``, so a page whose master toggle is OFF drops out of the
# sitemap instead of being advertised to Google as a 404. Private/utility/noindex
# pages (/login, /dashboard, /swf-docs) are deliberately omitted. The dynamic
# mod/modpack pages are appended separately by the sitemap renderer.
SITEMAP_PAGES: tuple[tuple[str, str | None], ...] = (
    ("/", None),
    ("/app", None),
    ("/browse", None),
    ("/documentation", None),
    ("/swf-docs", None),
    ("/support", None),
    ("/terms", None),
    ("/privacy", None),
    ("/accessibility", None),
    ("/changelog", None),
    ("/commands", "commands_enabled"),
    ("/classes", "classes_enabled"),
    ("/star-chart", "star_chart_enabled"),
    ("/gem-simulator", "gem_simulator_enabled"),
    ("/gems-guide", "gems_guide_enabled"),
    ("/abilities", "abilities_enabled"),
    ("/guides", "guides_enabled"),
    ("/fishing-guide", "fishing_guide_enabled"),
    ("/dressing-room", "dressing_room_page_enabled"),
    ("/sound-studio", "sound_studio_enabled"),
    ("/mod-workshop", "mod_workshop_enabled"),
    ("/blueprint-editor", "blueprint_editor_enabled"),
    ("/leaderboards", "leaderboards_enabled"),
    ("/activity", "player_activity_enabled"),
    ("/class-activity", "class_activity_enabled"),
    ("/updates", "updates_enabled"),
    ("/market", "market_enabled"),
    ("/tomes", "tomes_enabled"),
    ("/store", "store_enabled"),
    ("/codexes", "codexes_enabled"),
    ("/codexes/crafting", "codexes_enabled"),
    ("/status", "server_status_enabled"),
    ("/giveaways", "giveaways_enabled"),
    ("/clubs", "clubs_enabled"),
    ("/server-time", "server_time_enabled"),
    ("/calendar", "calendar_enabled"),
    ("/streams", "streams_enabled"),
    ("/releases", "btt_releases_enabled"),
    ("/mods", "mods_hub_enabled"),
    ("/mods/why", "mods_hub_enabled"),
    ("/modpacks", "mods_hub_enabled"),
)


# Machine-surface hosts this app also answers on - raw JSON / portal, never
# search material, so robots.txt blanket-disallows them. Everything else
# (notably the public site ``trove.aallyn.net``) defaults to crawlable: a
# default-allow is the safe failure mode if a proxy ever mangles the Host
# header, since the worst case is an extra host indexed, not the main site
# silently de-indexed.
_ROBOTS_BLOCKED_HOSTS = frozenset(
    url.split("://", 1)[-1].split("/", 1)[0].lower()
    for url in (settings.api_url, settings.dev_url, settings.docs_url)
)

def robots_body(host: str) -> str:
    """Host-aware robots.txt body. The public site is fully crawlable and
    advertises the sitemap; the raw-JSON API hosts get a disallow so Google never
    indexes endpoint payloads. ``/static/`` is intentionally NOT blocked - Google
    needs the CSS/JS to render the pages.

    The api-side hosts are a special case: the one app answers on the api host, the
    apex and its www, so showcase pages leak onto all three (api.aallyn.net/login,
    aallyn.net/login) and some already got indexed. Those now 301 to app_url (see
    ``add_api_host_redirect_middleware``) - but a blanket ``Disallow: /`` would FREEZE
    the stale entries, because Google must be allowed to crawl a URL to see its
    redirect and drop it. So they allow page crawling (the pages just 301 away) while
    still blocking the JSON API subtrees (/v1, /site, /git) so payloads are never
    indexed. dev./docs. keep the blanket disallow - they have no such redirect."""
    host = (host or "").lower()
    if host in API_SIDE_HOSTS:
        return (
            "User-agent: *\n"
            "Allow: /\n"
            "Disallow: /v1/\n"
            "Disallow: /site/\n"
            "Disallow: /git/\n"
        )
    if host in _ROBOTS_BLOCKED_HOSTS:
        return "User-agent: *\nDisallow: /\n"
    return (
        "User-agent: *\n"
        "Allow: /\n"
        "Disallow: /dashboard\n"
        "Disallow: /login\n"
        "Disallow: /site/\n"
        f"\nSitemap: {settings.app_url.rstrip('/')}/sitemap.xml\n"
    )
