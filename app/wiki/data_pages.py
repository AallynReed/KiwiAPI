"""Wiki pages the site renders itself (from game data, or a built-in guide), each
with an editable write-up.

A write-up is stored at ``data/<name>`` with a fixed title and read at ``/<name>``,
so ``<name>`` is reserved for plain articles.
"""

DATA_PAGES: dict[str, str] = {
    "delve-modifiers": "Delve Modifiers",
    "gems": "How Gems Work",
    "stat-modifiers": "How Stat Modifiers Work",
    "pvp-stats": "How PvP Stats Work",
    "titles": "Titles",
    "worlds": "Worlds & Biomes",
    "delve-gateways": "Delve Gateways",
    "shadow-tower": "Shadow Tower",
    "daily-bonuses": "Daily Bonuses",
    "leaderboards": "Leaderboards",
    "star-chart": "Star Chart",
    "depths-of-the-angler": "Depths of the Angler",
    "rune-anvil": "Rune Anvil",
    "gearcrafting-progression": "Gearcrafting Progression",
    "geode-tools": "Geode Tools",
    "mastery": "Mastery",
    "lootbox-odds": "Lootbox Odds",
    "pvp-powerups": "PvP Power-ups",
    "chat-commands": "Chat Commands",
}


def title_for(slug: str) -> str | None:
    """The fixed title of a ``data/<name>`` slug, else None."""
    prefix, _, name = slug.partition("/")
    return DATA_PAGES.get(name) if prefix == "data" else None
