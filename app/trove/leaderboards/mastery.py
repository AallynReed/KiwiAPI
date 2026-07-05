"""Trove / Geode Mastery point-to-level conversion.

Trove stores Mastery on the leaderboards as a running *points* total, not a
level - the client is the thing that turns points into the level the player
sees. This is a faithful port of the reference implementation used by
RenewedTroveTools (``utils/trove/mastery.py``): the cost of each level rises in
fixed tiers, and past level 300 it grows linearly.

    levels 2-5    cost 25 each
    levels 6-10   cost 50 each
    levels 11-20  cost 75 each
    levels 21-300 cost 100 each
    levels 301+   cost 150 + ceil((level - 300) * 0.5) each

``points_to_mr`` / ``mr_to_points`` are kept byte-for-byte compatible with the
reference so the numbers match what players see in-game; ``level_from_points``
is the friendly wrapper the API actually calls.
"""

from math import ceil

# points_to_mr caps its loop here, so this is the highest level it can report.
MAX_LEVEL = 1001


def points_to_mr(points):
    """Reference port: return ``(level, points_into_level, next_level_cost)``.

    ``points_into_level`` is how far the total has climbed into the current
    level; ``next_level_cost`` is the full cost of the *following* level.
    """
    i = 1
    increment = 25
    while True:
        if i == 1001:
            break
        i += 1
        if i <= 5:
            increment = 25
        elif 6 <= i <= 10:
            increment = 50
        elif 11 <= i <= 20:
            increment = 75
        elif 21 <= i <= 300:
            increment = 100
        elif i > 300:
            increment = 150 + ceil((i - 300) * 0.5)
        points -= increment
        if points <= 0:
            if points < 0:
                points += increment
                i -= 1
            break
    return i, points, increment


def mr_to_points(level):
    """Reference port: total points required to reach ``level`` (plus the cost of
    the next level), i.e. ``(next_level_cost, total_points_for_level)``."""
    points = 0
    i = 1
    increment = 25
    while True:
        i += 1
        if i <= 5:
            increment = 25
        elif 6 <= i <= 10:
            increment = 50
        elif 11 <= i <= 20:
            increment = 75
        elif 21 <= i <= 300:
            increment = 100
        elif i > 300:
            increment = 150 + ceil((i - 300) * 0.5)
        if i == level + 1:
            if i - 1 > 300:
                increment = 150 + ceil((i - 1 - 300) * 0.5)
            break
        points += increment
    return increment, points


def level_from_points(points: int) -> tuple[int, int, int]:
    """Turn a raw Mastery points total into
    ``(level, points_into_level, points_to_next_level)``.

    ``points_to_next_level`` is what's still owed to tick over to the next
    level (0 only at the theoretical cap). Negative/zero input -> level 1.
    """
    points = max(0, int(points))
    level, into_level, next_cost = points_to_mr(points)
    to_next = max(0, next_cost - into_level)
    return level, into_level, to_next
