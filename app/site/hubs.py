"""Server-side render model for the hub pages (/guides, /gem-tools).

A hub is a page whose only job is to list other pages, so the navbar carries one
entry instead of one per thing. Both hubs are the same shape - a title and some
rows - so they share a model and a template rather than being written twice.

Each row names the feature flag that governs its own page, so a switched-off page
drops off the hub the same way it drops out of search, rather than being linked
into a 404. Adding a row here plus its own page route is the whole job.
"""
from typing import Any

HUBS: dict[str, dict[str, Any]] = {
    "guides": {
        "path": "/guides",
        "title": "Trove <span class=\"highlight-blue\">Guides</span>",
        "sub": "Everything we have written up, in one place. More as we go.",
        "empty": "No guides are available right now.",
        "entries": (
            {
                "name": "How Gems Work",
                "path": "/gems-guide",
                "flag": "gems_guide_enabled",
                "icon": "fa-solid fa-gem",
                "blurb": "Tiers, elements, lesser versus empowered, stat rolls, levelling "
                         "and focusing - the whole gem system, one step at a time.",
                "kind": "Interactive",
            },
            {
                "name": "Fishing Guide",
                "path": "/fishing-guide",
                "flag": "fishing_guide_enabled",
                "icon": "fa-solid fa-fish",
                "blurb": "Every lure, pool and quest, plus all 155 fish with the "
                         "conditions each one needs and what it is worth.",
                "kind": "Reference",
            },
        ),
    },
    "gem-tools": {
        "path": "/gem-tools",
        "title": "Trove <span class=\"highlight-blue\">Gem Tools</span>",
        "sub": "Roll one, rate the one you have, or work out the build around it.",
        "empty": "No gem tools are available right now.",
        "entries": (
            {
                "name": "Gem Simulator",
                "path": "/gem-simulator",
                "flag": "gem_simulator_enabled",
                "icon": "fa-solid fa-gem",
                "blurb": "Roll, level and augment gems the way the game does, and "
                         "equip a loadout to see what it comes to.",
                "kind": "Simulator",
            },
            {
                "name": "Gem Evaluator",
                "path": "/gem-evaluator",
                "flag": "gem_evaluator_enabled",
                "icon": "fa-solid fa-magnifying-glass-chart",
                "blurb": "Score a gem you already own against what it could have "
                         "rolled, so you know whether to keep pouring into it.",
                "kind": "Rating",
            },
            {
                "name": "Gem Builds",
                "path": "/gem-builds",
                "flag": "gem_builds_enabled",
                "icon": "fa-solid fa-wand-magic-sparkles",
                "blurb": "The optimizer: class, subclass, ally, food and star chart "
                         "in, the best gem split out.",
                "kind": "Optimizer",
            },
        ),
    },
}


def hub_view(key: str, enabled: dict[str, bool] | None = None) -> dict[str, Any]:
    """`{title, sub, empty, entries, count}` - only the rows whose feature is on.

    `enabled` is the flag map resolved onto `request.state`; anything missing is
    treated as on, matching how the navbar renders an unresolved flag.
    """
    hub = HUBS[key]
    flags = enabled or {}
    entries = [dict(e) for e in hub["entries"] if flags.get(e["flag"], True)]
    return {"title": hub["title"], "sub": hub["sub"], "empty": hub["empty"],
            "entries": entries, "count": len(entries)}


def hub_flags(key: str) -> tuple[str, ...]:
    """The flags a hub's rows depend on, for the route to resolve off request.state."""
    return tuple(e["flag"] for e in HUBS[key]["entries"])
