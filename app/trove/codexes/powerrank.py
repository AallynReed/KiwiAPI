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

from app.trove.codexes.binfab import read_uleb

# tier value -> Power Rank. 800/802/804 and 600/620 are the handoff's signatures
# (A0 06 / A2 06 / A4 06 / D8 04 / EC 04); 0 is an explicit "no rank".
POWER_RANK_BY_TIER: dict[int, int] = {0: 0, 600: 50, 620: 50, 800: 5, 802: 20, 804: 75}

# A distinct structural context where field3 reads 0 but the PR is 30 - must be
# matched before the generic field3 scan (which would map the embedded
# `30 00 40 01` -> 0).
_PR_SPECIAL_30 = bytes.fromhex("2E0008300040011EE205")


def decode_power_rank(content: bytes) -> int | None:
    """Power Rank for a collection/equipment prefab, or None if no PR component is
    found. Returns the joined int (which may be 0 for an explicit zero rank)."""
    if _PR_SPECIAL_30 in content:
        return 30
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
                pr = POWER_RANK_BY_TIER.get(value)
                if pr is not None:
                    return pr
            i = j
        else:
            i += 1
    return None
