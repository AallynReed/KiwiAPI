"""The searchable map of the site itself: every page, and the tabs inside them.

Codex entries, players and mods are searched by querying their own stores. Pages
are not stored anywhere - they're routes - so they need a registry, and this is it.

Each destination carries the feature flag that governs it, so a page whose feature
is switched off disappears from search the same way it disappears from the navbar
instead of returning a result that 404s.

**Tabs matter as much as pages.** "Analytics" lives inside Market and "Evaluator"
inside Gems; someone searching for either has no reason to know which page owns it.
A tab entry carries its parent's path plus the query/hash that selects it, so the
result navigates straight to the right view.

Keep in sync with ``partials/navbar.html`` when a page is added - the navbar renders
its own markup, so a new page has to be listed in both. The flag key is the join
between them, and ``tests/unit/site/test_search.py`` asserts every entry here names
a real flag.

Pure + stdlib-only, so the matching is unit-testable without a request.
"""

from __future__ import annotations

import re
import unicodedata
from dataclasses import dataclass

# Subject keys. The order here is the order the sidebar shows them in: the things a
# person is most likely to have meant first.
SUBJECT_PAGES = "pages"
SUBJECT_COLLECTIONS = "collections"
SUBJECT_ITEMS = "items"
SUBJECT_RECIPES = "recipes"
SUBJECT_STYLES = "styles"
SUBJECT_PLAYERS = "players"
SUBJECT_MODS = "mods"
SUBJECT_MODPACKS = "modpacks"

SUBJECT_ORDER: tuple[str, ...] = (
    SUBJECT_PAGES, SUBJECT_COLLECTIONS, SUBJECT_ITEMS, SUBJECT_RECIPES,
    SUBJECT_STYLES, SUBJECT_PLAYERS, SUBJECT_MODS, SUBJECT_MODPACKS,
)

# Display labels. English IS the i18n key on this site, so these strings are the keys.
SUBJECT_LABELS: dict[str, str] = {
    SUBJECT_PAGES: "Pages",
    SUBJECT_COLLECTIONS: "Collections",
    SUBJECT_ITEMS: "Items",
    SUBJECT_RECIPES: "Recipes",
    SUBJECT_STYLES: "Styles",
    SUBJECT_PLAYERS: "Players",
    SUBJECT_MODS: "Mods",
    SUBJECT_MODPACKS: "Modpacks",
}

# Codex type -> the subject it is presented under. Several codex types collapse into
# one subject: nobody searching "ganda" is choosing between `mount` and `dragon`.
CODEX_TYPE_SUBJECT: dict[str, str] = {
    "ally": SUBJECT_COLLECTIONS, "mount": SUBJECT_COLLECTIONS,
    "dragon": SUBJECT_COLLECTIONS, "badge": SUBJECT_COLLECTIONS,
    "wings": SUBJECT_COLLECTIONS, "aura": SUBJECT_COLLECTIONS,
    "boat": SUBJECT_COLLECTIONS, "sail": SUBJECT_COLLECTIONS,
    "flask": SUBJECT_COLLECTIONS, "tome": SUBJECT_COLLECTIONS,
    "magrider": SUBJECT_COLLECTIONS, "fishingpole": SUBJECT_COLLECTIONS,
    "skin": SUBJECT_COLLECTIONS,
    "item": SUBJECT_ITEMS, "fish": SUBJECT_ITEMS, "memento": SUBJECT_ITEMS,
    "recipe": SUBJECT_RECIPES,
    "style": SUBJECT_STYLES,
}

CODEX_SUBJECTS: frozenset[str] = frozenset(CODEX_TYPE_SUBJECT.values())


@dataclass(frozen=True)
class Destination:
    """One searchable place on the site."""

    name: str                       # display name (also its i18n key)
    path: str                       # where to navigate
    flag: str | None = None         # feature_map flag key; None = always on
    icon: str = "fa-solid fa-file"
    group: str = ""                 # the navbar column it lives under
    parent: str = ""                # the page a tab belongs to ("" for a page)
    keywords: tuple[str, ...] = ()  # extra terms that should find it
    badge: str = ""                 # pill shown beside the nav label (e.g. "Beta")
    in_nav: bool = True             # listed in the Pages mega-menu

    @property
    def is_tab(self) -> bool:
        return bool(self.parent)


def _p(name, path, flag, icon, group, *keywords, badge="", in_nav=True) -> Destination:
    return Destination(name=name, path=path, flag=flag, icon=icon, group=group,
                       keywords=tuple(keywords), badge=badge, in_nav=in_nav)


def _tab(name, path, flag, parent, *keywords) -> Destination:
    # Tabs are searchable but never listed in the menu - the menu is pages.
    return Destination(name=name, path=path, flag=flag, icon="fa-solid fa-table-columns",
                       group=parent, parent=parent, keywords=tuple(keywords), in_nav=False)


# --- pages -------------------------------------------------------------------

PAGES: tuple[Destination, ...] = (
    _p("Leaderboards", "/leaderboards", "leaderboards_enabled", "fa-solid fa-ranking-star", "Live", "ranking", "top", "standings"),
    _p("Clubs", "/clubs", "clubs_enabled", "fa-solid fa-shield-halved", "Live", "guild"),
    _p("Player Activity", "/activity", "player_activity_enabled", "fa-solid fa-chart-line", "Live", "online", "population"),
    _p("Class Activity", "/class-activity", "class_activity_enabled", "fa-solid fa-users", "Live", "classes played"),
    _p("Server Status", "/status", "server_status_enabled", "fa-solid fa-signal", "Live", "uptime", "down", "maintenance"),
    _p("Server Time", "/server-time", "server_time_enabled", "fa-solid fa-clock", "Live", "clock", "timezone", "utc"),
    _p("Calendar", "/calendar", "calendar_enabled", "fa-solid fa-calendar-days", "Live", "events", "schedule"),

    _p("Market", "/market", "market_enabled", "fa-solid fa-shop", "Economy", "marketplace", "prices", "trading", "flux"),
    _p("Tome Values", "/tomes", "tomes_enabled", "fa-solid fa-book", "Economy", "tomes"),
    _p("Store History", "/store", "store_enabled", "fa-solid fa-clock-rotate-left", "Economy", "shop", "credits"),
    _p("Calculators", "/calculators", "calculators_enabled", "fa-solid fa-calculator", "Economy", "math"),

    _p("Star Chart", "/star-chart", "star_chart_enabled", "fa-solid fa-star", "Plan", "constellation", "build planner"),
    _p("Dressing Room", "/dressing-room", "dressing_room_page_enabled", "fa-solid fa-shirt", "Plan", "costume", "outfit", "preview", badge="Beta"),
    _p("Gem Simulator", "/gem-simulator", "gem_simulator_enabled", "fa-solid fa-gem", "Plan", "gems", "rolling"),
    _p("Gem Evaluator", "/gem-evaluator", "gem_evaluator_enabled", "fa-solid fa-magnifying-glass-chart", "Plan", "gems", "rate my gem"),
    _p("Gem Builds", "/gem-builds", "gem_builds_enabled", "fa-solid fa-wand-magic-sparkles", "Plan", "gems", "optimizer"),
    _p("How Gems Work", "/gems-guide", "gems_guide_enabled", "fa-solid fa-circle-question", "Plan", "gems", "guide", "explainer"),

    _p("Mods Hub", "/mods", "mods_hub_enabled", "fa-solid fa-cubes", "Create", "modding", "addons"),
    _p("Modpacks", "/modpacks", "mods_hub_enabled", "fa-solid fa-box-open", "Create", "bundles"),
    _p("Mod Workshop", "/mod-workshop", "mod_workshop_enabled", "fa-solid fa-screwdriver-wrench", "Create", "tmod", "compiler", "build a mod"),
    _p("Blueprint Editor", "/blueprint-editor", "blueprint_editor_enabled", "fa-solid fa-cube", "Create", "voxel", "blueprint", "recolour", "recolor", "materials", "glass", "model", "mount", "creature", "parts"),
    _p("Sound Studio", "/sound-studio", "sound_studio_enabled", "fa-solid fa-sliders", "Create", "audio", "music", "bnk"),
    _p("Unlock Debug", "/unlock-debug", "unlock_debug_enabled", "fa-solid fa-terminal", "Create", "debug console", "trove.exe", "patch", "byte patch", "console"),

    _p("Classes", "/classes", "classes_enabled", "fa-solid fa-hat-wizard", "Learn", "class list"),
    _p("Trove Commands", "/commands", "commands_enabled", "fa-solid fa-keyboard", "Learn", "slash commands", "chat"),
    _p("Codexes", "/codexes", "codexes_enabled", "fa-solid fa-book-atlas", "Learn", "database", "catalog", "game data", badge="Beta"),
    _p("Recipe Cost Calculator", "/codexes/crafting", "codexes_enabled", "fa-solid fa-flask", "Learn", "crafting", "cost", in_nav=False),
    _p("Updates", "/updates", "updates_enabled", "fa-solid fa-code-branch", "Learn", "patch notes", "changes", "datamining"),
    _p("Streams", "/streams", "streams_enabled", "fa-solid fa-video", "Learn", "twitch", "live"),
    _p("Giveaways", "/giveaways", "giveaways_enabled", "fa-solid fa-gift", "Learn", "free"),
    _p("App Releases", "/releases", "btt_releases_enabled", "fa-solid fa-cloud-arrow-down", "Learn", "app", "download", "versions"),

    _p("Get the App", "/app", None, "fa-solid fa-download", "About", "desktop", "install", in_nav=False),
    _p("Dashboard", "/dashboard", None, "fa-solid fa-gauge", "About", "account", "settings", "profile", in_nav=False),
    _p("Changelog", "/changelog", None, "fa-solid fa-list-check", "About", "site updates", in_nav=False),
    _p("Accessibility", "/accessibility", None, "fa-solid fa-universal-access", "About", "a11y", in_nav=False),
    _p("Support", "/support", None, "fa-solid fa-heart", "About", "donate", "tip", in_nav=False),
)

# --- tabs inside pages -------------------------------------------------------
#
# Only tabs that are a real destination - a distinct view someone would look for by
# name. Filter chips and sort toggles are not destinations and are deliberately absent;
# listing them would bury the pages under noise.

TABS: tuple[Destination, ...] = (
    _tab("Market Analytics", "/market?tab=analytics", "market_enabled", "Market", "movers", "liquidity", "volume", "deals"),
    _tab("Market Listings", "/market?tab=listings", "market_enabled", "Market", "prices", "sell"),

    _tab("Possible Renames", "/leaderboards?tab=renames", "leaderboard_renames_enabled", "Leaderboards", "name change"),
    _tab("Cheaters", "/leaderboards?tab=cheaters", "cheater_detection_enabled", "Leaderboards", "anti-cheat", "suspicious"),
    _tab("Alt Clusters", "/leaderboards?tab=clusters", "alt_clusters_enabled", "Leaderboards", "alts", "multi-account"),

    _tab("Updates: Files", "/updates?tab=files", "updates_enabled", "Updates", "archive", "browse", "tree"),
    _tab("Updates: Audio", "/updates?tab=audio", "updates_enabled", "Updates", "bnk", "sound", "music"),
    _tab("Updates: Interface", "/updates?tab=interface", "updates_enabled", "Updates", "swf", "ui", "flash"),

    _tab("Store Gallery", "/store?tab=gallery", "store_enabled", "Store History", "art", "textures"),
    _tab("Store Availability", "/store?tab=availability", "store_enabled", "Store History", "timeline", "history"),

    _tab("My Mods", "/dashboard#mods", "mods_hub_enabled", "Dashboard", "my projects", "uploads"),
    _tab("Stray Mods", "/mods/stray", "mods_hub_enabled", "Mods Hub", "unclaimed", "imported"),
)

ALL_DESTINATIONS: tuple[Destination, ...] = PAGES + TABS


# --- matching ----------------------------------------------------------------

_WORD_RE = re.compile(r"[a-z0-9]+")


def normalize(text: str) -> str:
    """Casefold + strip accents, so "Ganda" finds "gánda" and vice versa."""
    decomposed = unicodedata.normalize("NFKD", str(text or ""))
    return "".join(c for c in decomposed if not unicodedata.combining(c)).casefold().strip()


def _tokens(text: str) -> list[str]:
    return _WORD_RE.findall(normalize(text))


def score(destination: Destination, query: str) -> int:
    """How well a destination matches, 0 = not at all. Higher is better.

    Ranked so an exact name beats a prefix beats a word-start beats a keyword. Without
    the tiers, "market" surfaces "Market Analytics" above "Market" purely on string
    length, which puts the thing you asked for below the thing inside it.
    """
    q = normalize(query)
    if not q:
        return 0
    name = normalize(destination.name)
    if name == q:
        return 100
    if name.startswith(q):
        return 80
    name_tokens = _tokens(destination.name)
    if any(tok.startswith(q) for tok in name_tokens):
        return 60
    if q in name:
        return 40
    for keyword in destination.keywords:
        kw = normalize(keyword)
        if kw.startswith(q) or q in kw:
            return 25
    # Multi-word queries: every word has to land somewhere in the name or keywords.
    words = _tokens(query)
    if len(words) > 1:
        haystack = " ".join([name, *(normalize(k) for k in destination.keywords)])
        if all(w in haystack for w in words):
            return 20
    return 0


def search_destinations(query: str, enabled: dict[str, bool], *,
                        limit: int = 25) -> list[Destination]:
    """Matching pages/tabs, best first, with disabled features removed.

    ``enabled`` is the feature-flag map (`feature_map.flags()`); a destination whose
    flag is off is dropped, so search can never offer a page the site isn't serving.
    An unknown flag key is treated as OFF - a typo hides the row rather than shipping a
    link into a 404, and the unit test catches it before it ships.
    """
    scored: list[tuple[int, int, Destination]] = []
    for destination in ALL_DESTINATIONS:
        if destination.flag is not None and not enabled.get(destination.flag, False):
            continue
        value = score(destination, query)
        if value:
            # Pages outrank their own tabs at equal score.
            scored.append((value, 0 if not destination.is_tab else -1, destination))
    scored.sort(key=lambda row: (-row[0], -row[1], row[2].name))
    return [row[2] for row in scored[:limit]]


def to_row(destination: Destination) -> dict:
    """A destination as a search result row."""
    return {
        "name": destination.name,
        "path": destination.path,
        "icon": destination.icon,
        "group": destination.group,
        "parent": destination.parent,
        "kind": "tab" if destination.is_tab else "page",
    }


# --- the navbar's Pages menu -------------------------------------------------
#
# The mega-menu renders FROM this registry, so a page is declared once and appears in
# both the menu and search. Previously the menu was hand-written markup and this list
# was its shadow: adding a page meant editing both, and forgetting one was invisible
# until someone went looking for a link that was never there.

# Column order in the menu. A group absent here isn't rendered as a column - "About"
# holds destinations reached from the CTA and the utility cluster instead.
NAV_GROUPS: tuple[str, ...] = ("Live", "Economy", "Plan", "Create", "Learn")


def nav_menu(enabled: dict[str, bool]) -> list[dict]:
    """`[{group, items: [Destination, …]}, …]` for the Pages menu, in column order.

    Only enabled, nav-listed PAGES (never tabs). A group whose every page is switched
    off is omitted entirely, so the menu can't render an empty column - which is what
    the old markup's `{% if a or b or c %}` guards were doing by hand, one per group.
    """
    out: list[dict] = []
    for group in NAV_GROUPS:
        items = [
            d for d in PAGES
            if d.group == group and d.in_nav
            and (d.flag is None or enabled.get(d.flag, False))
        ]
        if items:
            out.append({"group": group, "items": items})
    return out


def nav_paths(enabled: dict[str, bool]) -> list[str]:
    """Every path the Pages menu can reach - what the trigger's active state keys on."""
    return [d.path for section in nav_menu(enabled) for d in section["items"]]


def nav_is_active(path: str, enabled: dict[str, bool]) -> bool:
    """Does `path` belong to something the Pages menu can reach?

    Sub-paths count: `/mods/<author>/<mod>` is still Mods Hub. `/` is excluded from
    prefix matching or every page on the site would match it.
    """
    current = (path or "/").rstrip("/") or "/"
    for known in nav_paths(enabled):
        base = known.split("?", 1)[0].rstrip("/") or "/"
        if base == "/":
            continue
        if current == base or current.startswith(base + "/"):
            return True
    return False
