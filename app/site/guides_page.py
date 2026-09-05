"""Server-side render model for /guides.

A hub, not a guide: it lists the explainers we have written so they live in one
place instead of one navbar entry each. Each entry names the feature flag that
governs its page, so a guide whose feature is switched off drops off the hub the
same way it drops out of search - rather than being advertised into a 404.

Adding a guide is a row here plus its own page route; nothing else moves.
"""
from typing import Any

# Ordered as the hub shows them: the broadest first.
GUIDES: tuple[dict[str, str], ...] = (
    {
        "name": "How Gems Work",
        "path": "/gems-guide",
        "flag": "gems_guide_enabled",
        "icon": "fa-solid fa-gem",
        "blurb": "Tiers, elements, lesser versus empowered, stat rolls, levelling and "
                 "focusing - the whole gem system, one step at a time.",
        "length": "Interactive",
    },
    {
        "name": "Fishing Guide",
        "path": "/fishing-guide",
        "flag": "fishing_guide_enabled",
        "icon": "fa-solid fa-fish",
        "blurb": "Every lure, pool and quest, plus all 155 fish with the conditions "
                 "each one needs and what it is worth.",
        "length": "Reference",
    },
)


def guides_view(enabled: dict[str, bool] | None = None) -> dict[str, Any]:
    """`{guides, count}` - only the ones whose own feature is on.

    `enabled` is the flag map resolved onto `request.state`; anything missing is
    treated as on, matching how the navbar renders an unresolved flag.
    """
    flags = enabled or {}
    guides = [dict(g) for g in GUIDES if flags.get(g["flag"], True)]
    return {"guides": guides, "count": len(guides)}
