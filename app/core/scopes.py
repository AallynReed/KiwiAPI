"""Bitmask scope registry (Discord-style permissions).

CONVENTION
----------
Scope keys are ``<resource>:<action>`` where action is ``read`` or ``write``
(``write`` covers create / update / delete on that resource). This keeps tokens
least-privilege and the portal UI groupable by resource.

RULES (these make scopes safe long-term — do not break them)
- Each scope owns a PERMANENT power-of-two bit, assigned once in ``_REGISTRY``.
  A bit is NEVER renumbered or reused. To retire a scope, delete its entry but
  never hand its bit to a different scope (leave a gap).
- The mask ``0`` means ALL scopes — present and future — so adding a new scope
  never invalidates existing "all" tokens.
- New scopes take the next free bit. Append; don't reorder.

This 1.0 base ships with NO scopes — they're defined alongside the real data
endpoints. When you add an endpoint, append its scope here, e.g.:

    Scope("widgets:read",  1 << 0, "widgets", "Read your widgets"),
    Scope("widgets:write", 1 << 1, "widgets", "Create / update / delete your widgets"),

Until a scope exists, tokens can only be created with the "all" mask (0).
"""

from dataclasses import dataclass


@dataclass(frozen=True)
class Scope:
    key: str          # "<resource>:<action>"
    bit: int          # permanent power-of-two
    resource: str     # grouping label for the portal UI
    description: str


# Permanent bit registry. NEVER renumber or reuse a bit; append new ones.
# Scopes are organized by FUNCTION (not by game), since most endpoints are Trove.
_REGISTRY: tuple[Scope, ...] = (
    Scope("rotations:read", 1 << 0, "rotations",
          "Read rotation/timer data — server time, bonuses, merchants, biomes"),
    Scope("feeds:read", 1 << 1, "feeds",
          "Read relayed feeds — Trove news, Twitch/YouTube/Bilibili"),
    Scope("stats:read", 1 << 2, "stats",
          "Read raw game-stat data — stat tables (power rank / magic find / light) and classes"),
    Scope("gems:read", 1 << 3, "gems",
          "Use the gem tools — simulator (generate/augment), evaluator, and build optimizer"),
    Scope("misc:read", 1 << 4, "misc",
          "Misc tools — third-party modding software list and the time converter"),
    Scope("mods:read", 1 << 5, "mods",
          "Mod tools — decompile a .tmod file and build a .tmod from files"),
    Scope("updates:read", 1 << 6, "updates",
          "Browse archived game files — versions, directory structure, and single-file download"),
    Scope("codexes:read", 1 << 7, "codexes",
          "Read parsed game codexes — allies, mounts, dragons, mementos, recipes, items, fish, badges"),
    Scope("btt:read", 1 << 8, "btt",
          "Read BetterTroveTools releases — latest version per platform and channel"),
    Scope("leaderboards:read", 1 << 9, "leaderboards",
          "Read Trove in-game leaderboards — boards, entries, timestamps"),
    Scope("market:read", 1 << 10, "market",
          "Read Trove marketplace listings — listings, items, interest list, price history"),
)

ALL_SCOPES = 0  # sentinel mask meaning "every scope, present and future"

SCOPE_BITS: dict[str, int] = {s.key: s.bit for s in _REGISTRY}
# Bits are distinct powers of two, so summing == OR-ing them all together.
_KNOWN_MASK: int = sum(SCOPE_BITS.values())


def catalog() -> list[dict]:
    """Ordered scope metadata for clients: {key, bit, resource, description}."""
    return [
        {"key": s.key, "bit": s.bit, "resource": s.resource, "description": s.description}
        for s in _REGISTRY
    ]


def is_valid_mask(mask: int) -> bool:
    """Valid if it's 0 (all) or uses only known bits — no stray/unknown bits."""
    return mask == ALL_SCOPES or (mask >= 0 and (mask & ~_KNOWN_MASK) == 0)


def mask_grants(mask: int, scope: str) -> bool:
    """Does this mask grant the named scope? (0 grants everything.)"""
    if mask == ALL_SCOPES:
        return True
    bit = SCOPE_BITS.get(scope)
    return bit is not None and bool(mask & bit)


def decode(mask: int) -> list[str]:
    """Keys of the scopes set in the mask (empty list for 0 / all)."""
    if mask == ALL_SCOPES:
        return []
    return [s.key for s in _REGISTRY if mask & s.bit]
