"""Collectible Power Rank, ported from BTT (`decode_known_power_rank`).

Power Rank lives in a nested `_powerrank` component: field 3 (key `0x30`, wt0
varint) holds a small *tier* value, immediately followed by field 4 == 1
(`0x40 0x01`). The tier -> PR mapping is data (the join table the handoff describes
as `prefabs/meta/powerrank.binfab`), live-confirmed in-game. The handoff's
collection-side hex signatures are just instances of this block, e.g.
`30 A0 06 40 01` = `0x30` varint 800 (`A0 06`) `0x40 0x01` -> PR 5.

We decode the component directly and join it to the tier map, rather than treating
the standalone `powerrank.binfab` as proof of any one collectible's rank.
"""

from __future__ import annotations

from app.trove.codexes.binfab import read_uleb, unzig

# tier value -> Power Rank. 800/802/804 and 600/620 are the handoff's signatures
# (A0 06 / A2 06 / A4 06 / D8 04 / EC 04); 0 is an explicit "no rank".
POWER_RANK_BY_TIER: dict[int, int] = {0: 0, 600: 50, 620: 50, 800: 5, 802: 20, 804: 75}

# A distinct structural context where field3 reads 0 but the PR is 30 - must be
# matched before the generic field3 scan (which would map the embedded
# `30 00 40 01` -> 0).
_PR_SPECIAL_30 = bytes.fromhex("2E0008300040011EE205")


def parse_power_rank_table(data: bytes) -> dict[int, int]:
    """`prefabs/meta/powerrank.binfab` -> `{tier marker: Power Rank}`.

    Rows are `1E <uleb index> 00 <uleb marker> 10 <uleb value>`. Reading the real join
    table replaces a hardcoded tier list that returned None for anything outside the six
    tiers it knew - so a collectible on any other tier reported no Power Rank at all.
    The hardcoded map stays as a fallback for when the file isn't in the archive."""
    table: dict[int, int] = {}
    n = len(data)
    pos = 0
    while pos < n:
        marker_at = data.find(b"\x1e", pos)
        if marker_at < 0:
            break
        cursor = marker_at + 1
        try:
            _index, cursor = read_uleb(data, cursor)
            if cursor >= n or data[cursor] != 0x00:
                pos = marker_at + 1
                continue
            cursor += 1
            tier, cursor = read_uleb(data, cursor)
            if cursor >= n or data[cursor] != 0x10:
                pos = marker_at + 1
                continue
            cursor += 1
            value, cursor = read_uleb(data, cursor)
        except (IndexError, ValueError):
            pos = marker_at + 1
            continue
        rank = unzig(value)
        if rank > 0:                       # a zero row states no rank, not a mapping
            table.setdefault(tier, rank)
        pos = cursor
    return table


def decode_power_rank(content: bytes, table: dict[int, int] | None = None) -> int | None:
    """Power Rank for a collection/equipment prefab, or None if no PR component is
    found. Returns the joined int (which may be 0 for an explicit zero rank).

    ``table`` is the decoded `powerrank.binfab` join table; without it the built-in
    tier map is used, which only covers the six tiers that were reverse-engineered by
    hand."""
    if _PR_SPECIAL_30 in content:
        return 30
    joined = {**POWER_RANK_BY_TIER, **(table or {})}
    n = len(content)
    i = 0
    while i < n - 2:
        if content[i] == 0x30:
            try:
                value, j = read_uleb(content, i + 1)
            except (IndexError, ValueError):
                i += 1
                continue
            if j + 1 < n and content[j] == 0x40 and content[j + 1] == 0x01:
                pr = joined.get(value)
                if pr is not None:
                    return pr
            i = j
        else:
            i += 1
    return None
