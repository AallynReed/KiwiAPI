"""Badge requirements from `prefabs/meta/badges.binfab`.

Badges are the only collectible whose *earning condition* is data rather than
prose: each badge group carries N ranks (bronze → trovium) and each rank states a
`completion_kind` plus a typed payload (a metric threshold, a Shadow Tower boss on
a difficulty, a Paragon level across N classes, …). The codex showed the badge but
never what you had to do to get it; this decodes that.

File layout, read structurally (no name guessing):

    <hdr 3e ae> <uleb reward_count>
    reward_count x [ <index pattern> <len>id <len>$name_key <len>badge_id ]
    <uleb badge_count>
    badge_count x [ <index pattern> <len>badge_id .. <rank_count> ranks... ]

Each rank opens with a `BE 02` marker; inside it `08 AE 03 08 <len>kind` names the
completion kind and the payload follows at a kind-specific marker. Values are
ZigZag varints.

`completion_kind == "metric"` stores an *enum ordinal*, not a name. That enum is
compiled into the client, so `parse_metric_names` reads the ordered string table
out of `Trove_x64.exe` (which the archive already mirrors) instead of shipping a
hardcoded copy that silently drifts a rank's requirement onto the wrong metric
every time Trion appends a metric. Without the exe, metric rows still decode their
index + amount and are flagged `metric_name_unresolved` rather than guessed.

Pure + stdlib-only.
"""

from __future__ import annotations

import re

from app.trove.codexes.binfab import read_varint, unzig

# Rank ordinal -> the game's rank name. 7 ranks exist; a badge uses the first N.
RANK_NAMES: dict[int, str] = {
    1: "bronze", 2: "silver", 3: "gold", 4: "platinum",
    5: "diamond", 6: "obsidian", 7: "trovium",
}

# Shadow Tower difficulty ordinal -> name (the payload is 0-based).
_ST_DIFFICULTY: dict[int, str] = {0: "Normal", 1: "Hard", 2: "Ultra"}

# Boss prefab -> display name. Only these two exist; an unknown prefab keeps its
# raw path rather than being renamed into something invented.
_ST_BOSSES: dict[str, str] = {
    "quest/shadow_tower_boss_hydra.prefab": "Shadow Hydrakken",
    "quest/shadow_tower_boss_dreadnought.prefab": "Darknik Dreadnought",
}

_RANK_MARKER = b"\xbe\x02"
_KIND_MARKER = b"\x08\xae\x03\x08"
_PAYLOAD_MARKER = b"\xae\x01\x1e\x00"
_PAYLOAD_MARKER_STR = b"\xae\x01\x1e\x08"

_IDENT_RE = re.compile(rb"[A-Za-z_][A-Za-z0-9_]*")
_CAMEL_RE = re.compile(r"([a-z0-9])([A-Z])")


# --- metric enum (from the client executable) -------------------------------

EXE_PATH = "Trove_x64.exe"
_METRIC_SENTINEL = b"KPlayerMetric\x00"


def parse_metric_names(exe: bytes) -> list[str]:
    """The `PlayerMetric` enum names from the client executable, INDEXED BY ORDINAL.

    The names sit in one NUL-padded, contiguous ASCII table that ends at the enum's
    `KPlayerMetric` sentinel, so we anchor on the sentinel and walk BACKWARDS over
    the alignment padding, taking the identifier that ends at each step.

    Two details the layout forces:

    - The table is preceded by a float constant pool whose last byte (``0x3F`` of a
      ``1.0f``) is *printable*, so the raw printable run at the table's head reads
      ``"?EnemyKills"``. A token with leading junk therefore means "this is the
      first entry" - we keep its identifier suffix and stop, rather than discarding
      it (which silently dropped ``EnemyKills`` and shifted every later name by one).
    - Ordinal 0 is an unnamed slot with no string in the table, so `names[0]` is
      ``""`` and the first real metric is ordinal 1. Cross-checked against three
      unambiguous badges (``blocks`` -> BlocksDestroyed, ``boxesopened`` ->
      BoxesOpened, ``quest`` -> QuestsCompleted); without it every metric name lands
      two rows off and reports a plausible but wrong requirement.

    Empty list if the layout doesn't match (=> metric rows report
    `metric_name_unresolved` rather than a guessed name).
    """
    end = exe.find(_METRIC_SENTINEL)
    if end < 0:
        return []
    names: list[str] = []
    pos = end
    while pos > 0:
        stop = pos - 1
        while stop > 0 and exe[stop] == 0:      # skip the alignment padding
            stop -= 1
        if stop <= 0:
            break
        start = stop
        while start > 0 and 0x20 <= exe[start - 1] < 0x7F:
            start -= 1
        run = exe[start:stop + 1]
        token = run if _IDENT_RE.fullmatch(run) else None
        if token is None:                        # leading junk => head of the table
            match = re.search(rb"[A-Za-z_][A-Za-z0-9_]*$", run)
            if match and len(match.group(0)) >= 3:
                names.append(match.group(0).decode("ascii"))
            break
        if len(token) < 3:
            break
        names.append(token.decode("ascii"))
        pos = start
    if not names:
        return []
    names.reverse()
    return ["", *names]                          # index == ordinal; 0 is unnamed


def display_metric(name: str) -> str:
    """`FlawlessHydraUltraKills` -> `Flawless Hydra Ultra Kills`."""
    return _CAMEL_RE.sub(r"\1 \2", name or "").strip()


# --- low-level reads --------------------------------------------------------

def _read_zigzag(data: bytes, pos: int, limit: int) -> tuple[int | None, int]:
    value, nxt = read_varint(data[:limit], pos)
    return (unzig(value), nxt) if value is not None else (None, pos)


def _read_string(data: bytes, pos: int, limit: int) -> tuple[str | None, int]:
    """A single-byte-length-prefixed string (every id in this file is < 128 bytes)."""
    if pos >= limit:
        return None, pos
    length = data[pos]
    pos += 1
    if pos + length > limit:
        return None, pos
    return data[pos:pos + length].decode("latin1"), pos + length


def _index_pattern(index: int) -> bytes:
    """The 1-based row marker for element `index` of a table.

    Rows are keyed `field = 16*index - 12` written as a varint, followed by `0x08`.
    Mirrors the encoder the file was written with (values past 128 carry into a
    second byte)."""
    code1 = index * 16 - 12
    code2 = code1 // 128
    code3 = code2 // 128
    if code1 > 128:
        code1 -= (code1 // 128 - 1) * 128
    if code2 >= 256:
        code2 -= (code2 // 128 - 1) * 128
    out = bytes([code1])
    if code2 > 0:
        out += bytes([code2])
    if code3 > 0:
        out += bytes([code3])
    return out + b"\x08"


def _find_before(data: bytes, needle: bytes, start: int, end: int) -> int:
    at = data.find(needle, start)
    return at if 0 <= at < end else -1


# --- layout -----------------------------------------------------------------

def _layout(data: bytes) -> dict:
    """`{reward_ids, groups, errors}` - the reward table then the badge groups.

    Each group records where its ranks start and where the next group begins, so a
    rank scan can never run past its own badge."""
    out: dict = {"reward_ids": [], "groups": [], "errors": []}
    n = len(data)
    if n < 16:
        out["errors"].append("badges.binfab is too short")
        return out

    count, pos = read_varint(data, 2)
    if count is None or not 0 < count <= 5000:
        out["errors"].append("reward count is out of range")
        return out

    for i in range(1, count + 1):
        found = data.find(_index_pattern(i), pos)
        if found < 0:
            out["errors"].append(f"reward row {i} not found")
            return out
        p = found + len(_index_pattern(i))
        reward_id, p = _read_string(data, p, n)
        if reward_id is None:
            out["errors"].append(f"reward id {i} unreadable")
            return out
        out["reward_ids"].append(reward_id)
        p += 1
        for _field in range(2):                 # $name key + the badge id it belongs to
            value, p = _read_string(data, p, n)
            if value is None:
                out["errors"].append(f"reward metadata {i} unreadable")
                return out
            p += 1
        pos = p

    badge_count, _ = read_varint(data, pos + 4)
    if badge_count is None or not 0 < badge_count <= 1000:
        out["errors"].append("badge group count is out of range")
        return out

    search = pos + 4
    for i in range(1, badge_count + 1):
        pattern = _index_pattern(i)
        found = data.find(pattern, search)
        if found < 0:
            out["errors"].append(f"badge group {i} not found")
            return out
        p = found + len(pattern)
        badge_id, p = _read_string(data, p, n)
        if badge_id is None:
            out["errors"].append(f"badge group id {i} unreadable")
            return out
        p += 3
        if p >= n:
            out["errors"].append(f"badge group {i} rank count missing")
            return out
        rank_count = data[p]
        p += 2
        out["groups"].append({
            "index": i, "start": found, "ranks_start": p,
            "badge_id": badge_id, "rank_count": rank_count,
        })
        search = p

    groups = out["groups"]
    for i, group in enumerate(groups):
        group["end"] = groups[i + 1]["start"] if i + 1 < len(groups) else n
    return out


# --- payload decoders -------------------------------------------------------

def _payload_at(data: bytes, marker: bytes, start: int, end: int) -> int:
    at = _find_before(data, marker, start, end)
    return -1 if at < 0 else at + len(marker)


def _blocked(reason: str) -> dict:
    return {"status": "blocked", "blocker": "source_parse_incomplete", "reason": reason}


def _metric(data: bytes, start: int, end: int, metric_names: list[str]) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0:
        return _blocked("metric payload marker not found")
    encoded, p = read_varint(data, at)
    if encoded is None or p >= end or data[p] != 0x10:
        return _blocked("metric amount field missing")
    amount, _ = _read_zigzag(data, p + 1, end)
    # The stored value is the ordinal doubled (the field's own ZigZag framing).
    index = encoded // 2 if encoded % 2 == 0 else None
    key = metric_names[index] if index is not None and index < len(metric_names) else ""
    return {
        "decoder": "metric", "requirement_key": key,
        "label": display_metric(key), "amount": amount,
        "context": {"metric_index": index, "encoded_metric": encoded},
        "status": "decoded" if key else "blocked",
        "blocker": "" if key else "metric_name_unresolved",
        "reason": "" if key else "metric ordinal has no name in the client enum table",
    }


def _dragon_souls(data: bytes, start: int, end: int, badge_id: str) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0:
        return _blocked("dragon souls payload marker not found")
    shape, p = read_varint(data, at)
    if shape is None or p >= end or data[p] != 0x10:
        return _blocked("dragon souls amount field missing")
    amount, _ = _read_zigzag(data, p + 1, end)
    label = re.sub(r"_badge$", "", badge_id)
    label = re.sub(r"^dragon_", "", label).replace("_", " ").title()
    return {
        "decoder": "dragonsouls", "requirement_key": f"{badge_id}:dragonsouls",
        "label": f"{label} Dragon Souls", "amount": amount,
        "context": {"shape": shape}, "status": "decoded", "blocker": "", "reason": "",
    }


def _st_boss(data: bytes, start: int, end: int) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER_STR, start, end)
    if at < 0:
        return _blocked("Shadow Tower boss payload marker not found")
    boss, p = _read_string(data, at, end)
    if boss is None or p >= end or data[p] != 0x10:
        return _blocked("Shadow Tower boss payload incomplete")
    amount, p = _read_zigzag(data, p + 1, end)
    if amount is None or p >= end or data[p] != 0x20:
        return _blocked("Shadow Tower difficulty field missing")
    difficulty, _ = _read_zigzag(data, p + 1, end)
    diff_name = _ST_DIFFICULTY.get(difficulty or 0, "Unknown")
    boss_name = _ST_BOSSES.get(boss, boss)
    return {
        "decoder": "stboss", "requirement_key": f"{boss}:{diff_name}",
        "label": f"Defeat the {boss_name} on {diff_name} difficulty",
        "amount": amount, "difficulty": (difficulty or 0) + 1,
        "context": {"boss_prefab": boss, "difficulty": diff_name},
        "status": "decoded", "blocker": "", "reason": "",
    }


_SCALAR_LABELS = {"friends": "Friends added", "loyalty": "Loyalty",
                  "referafriend": "Referrals"}


def _scalar(data: bytes, start: int, end: int, kind: str) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0:
        return _blocked("scalar payload marker not found")
    amount, _ = _read_zigzag(data, at, end)
    return {
        "decoder": "scalar", "requirement_key": kind,
        "label": _SCALAR_LABELS.get(kind, kind), "amount": amount,
        "context": {}, "status": "decoded", "blocker": "", "reason": "",
    }


def _two_values(data: bytes, start: int, end: int) -> tuple[int, int] | None:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0:
        return None
    first, p = _read_zigzag(data, at, end)
    if first is None or p >= end or data[p] != 0x10:
        return None
    second, _ = _read_zigzag(data, p + 1, end)
    return None if second is None else (first, second)


def _upgrade_gems(data: bytes, start: int, end: int) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0 or at >= end:
        return _blocked("upgrade gems payload marker not found")
    scope = unzig(data[at])
    p = at + 1
    if p >= end or data[p] != 0x10:
        return _blocked("upgrade gems amount field missing")
    amount, _ = _read_zigzag(data, p + 1, end)
    all_stats = scope == 1
    return {
        "decoder": "upgradegems",
        "requirement_key": "UpgradeGems:" + ("all_gem_stats" if all_stats else "single_gem_stat"),
        "label": ("Fully upgrade all stats to 100% on max level Stellar Gems" if all_stats
                  else "Fully upgrade max level Stellar Gems to 100%"),
        "amount": amount, "context": {"scope": scope},
        "status": "decoded", "blocker": "", "reason": "",
    }


def _subclass(data: bytes, start: int, end: int) -> dict:
    values = _two_values(data, start, end)
    if values is None:
        return _blocked("subclass payload incomplete")
    level, power_rank = values
    label = (f"Upgrade 15 classes to Level {level} + Power Rank {power_rank}" if level > 0
             else f"Upgrade 15 classes to Power Rank {power_rank}")
    return {
        "decoder": "subclass",
        "requirement_key": f"SubClassEquipped:level={level}:power_rank={power_rank}",
        "label": label, "amount": 15,
        "context": {"level": level, "power_rank": power_rank, "class_count": 15},
        "status": "decoded", "blocker": "", "reason": "",
    }


def _multi_class(data: bytes, start: int, end: int) -> dict:
    values = _two_values(data, start, end)
    if values is None:
        return _blocked("multi-class payload incomplete")
    level, classes = values
    return {
        "decoder": "multi_class_paragon",
        "requirement_key": f"minprestigelevelsacrossclasses:level={level}:classes={classes}",
        "label": f"Reach Paragon Level {level} with {classes} classes", "amount": classes,
        "context": {"paragon_level": level, "class_count": classes},
        "status": "decoded", "blocker": "", "reason": "",
    }


def _entitlement(data: bytes, start: int, end: int) -> dict:
    if b"PLAYERCLASS_CRIMEFIGHTER" not in data[start:end]:
        return _blocked("entitlement class identifier not found")
    return {
        "decoder": "entitlement", "requirement_key": "PLAYERCLASS_CRIMEFIGHTER",
        "label": "Unlock the Vanguardian class", "amount": 0,
        "context": {"entitlement": "PLAYERCLASS_CRIMEFIGHTER"},
        "collection": "collections/badge/hero_unlocked",
        "status": "decoded", "blocker": "", "reason": "",
    }


def _tinyquest_pet_levels(data: bytes, start: int, end: int) -> dict:
    values = _two_values(data, start, end)
    if values is None:
        return _blocked("tinyquest pet-level payload incomplete")
    level, count = values
    noun = "Ally" if count == 1 else "Allies"
    return {
        "decoder": "tinyquest_pet_levels", "requirement_key": "tinyquesttotalpetsoflevel",
        "label": f"Reach level {level} with {count} {noun}", "amount": count,
        "context": {"ally_level": level, "ally_count": count},
        "status": "decoded", "blocker": "", "reason": "",
    }


def _tinyquest_buffed_pets(data: bytes, start: int, end: int) -> dict:
    at = _payload_at(data, _PAYLOAD_MARKER, start, end)
    if at < 0:
        return _blocked("tinyquest buffed-pets payload marker not found")
    amount, _ = _read_zigzag(data, at, end)
    noun = "Ally" if amount == 1 else "Allies"
    return {
        "decoder": "tinyquest_buffed_pets", "requirement_key": "tinyquestconcurrentbuffedpets",
        "label": f"Buff {amount} {noun} at once", "amount": amount,
        "context": {"ally_count": amount},
        "status": "decoded", "blocker": "", "reason": "",
    }


def _decode_payload(data: bytes, kind: str, after_kind: int, rank_start: int,
                    rank_end: int, badge_id: str, metric_names: list[str]) -> dict:
    if kind == "metric":
        return _metric(data, after_kind, rank_end, metric_names)
    if kind == "dragonsouls":
        return _dragon_souls(data, after_kind, rank_end, badge_id)
    if kind == "STBossKilled":
        return _st_boss(data, after_kind, rank_end)
    if kind in ("friends", "loyalty", "referafriend"):
        return _scalar(data, after_kind, rank_end, kind)
    if kind == "UpgradeGems":
        return _upgrade_gems(data, after_kind, rank_end)
    if kind == "SubClassEquipped":
        return _subclass(data, after_kind, rank_end)
    if kind == "minprestigelevelsacrossclasses":
        return _multi_class(data, after_kind, rank_end)
    if kind == "entitlementUnlocked":
        return _entitlement(data, after_kind, rank_end)
    if kind == "tinyquesttotalpetsoflevel":
        return _tinyquest_pet_levels(data, rank_start, rank_end)
    if kind == "tinyquestconcurrentbuffedpets":
        return _tinyquest_buffed_pets(data, rank_start, rank_end)
    if kind == "none":
        # A descriptive badge (awarded by an event, not a tracked counter). Decoded,
        # just with nothing to count - not an error.
        return {"decoder": "descriptive", "requirement_key": "none", "label": "",
                "amount": 0, "context": {}, "status": "decoded",
                "blocker": "descriptive", "reason": "no scalar requirement"}
    return {"status": "blocked", "blocker": "unsupported_completion_kind",
            "requirement_key": kind, "reason": f"completion kind {kind!r} is not decoded"}


# --- rewards / collection targets -------------------------------------------

def _reward_ids_in(segment: bytes, known: set[str]) -> list[str]:
    """Reward ids referenced inside one rank's byte span, in source order.

    A reference is a wire key with low nibble 4 followed by a length byte, so we
    only accept a candidate that is BOTH shaped like that field and a member of the
    file's own reward table - a substring that merely looks like an id is dropped."""
    out: list[str] = []
    seen: set[str] = set()
    pos = 0
    n = len(segment)
    while pos + 2 <= n:
        if (segment[pos] & 0x0F) != 4:
            pos += 1
            continue
        length = segment[pos + 1]
        if length < 1 or pos + 2 + length > n:
            pos += 1
            continue
        candidate = segment[pos + 2:pos + 2 + length].decode("latin1")
        if candidate in known:
            if candidate not in seen:
                seen.add(candidate)
                out.append(candidate)
            pos += length + 2
        else:
            pos += 1
    return out


def _collection_for(reward_id: str) -> str:
    """The `collections/badge/…` entry a reward id names.

    `_plat`/`_plat_v2` are the file's own historical spellings of `_platinum`, and
    `hero_unlocked_badge` is the one id that doesn't take the `badge_` prefix - both
    are renames the game itself applies, not similarity matching."""
    if not reward_id:
        return ""
    ident = re.sub(r"_plat_v2$", "_platinum", reward_id)
    ident = re.sub(r"_plat$", "_platinum", ident)
    ident = re.sub(r"^badge_", "", ident)
    if ident == "hero_unlocked_badge":
        ident = "hero_unlocked"
    return "collections/badge/" + ident


def _rank_reward(rewards: list[str], rank_name: str) -> str:
    """The reward whose id ends in this rank's name, else the first one listed."""
    suffix = "_" + rank_name
    for reward in rewards:
        if reward.endswith(suffix):
            return reward
    return rewards[0] if rewards else ""


# --- public -----------------------------------------------------------------

def parse_badges(data: bytes, metric_names: list[str] | None = None) -> dict:
    """`{rows, errors}` for `prefabs/meta/badges.binfab`.

    Each row is one (badge, rank): the collection path it awards, the rank name and
    tier, the completion kind, a decoded requirement key + human label + amount, and
    a `status` of `decoded` or `blocked` with the reason. A row that can't be decoded
    is still emitted (blocked) - the rank exists whether or not we can read its
    payload, and dropping it would silently understate a badge's ladder.
    """
    metric_names = metric_names or []
    layout = _layout(data)
    if layout["errors"]:
        return {"rows": [], "errors": layout["errors"]}

    known_rewards = set(layout["reward_ids"])
    rows: list[dict] = []

    for group in layout["groups"]:
        cursor = group["ranks_start"]
        for rank in range(1, group["rank_count"] + 1):
            rank_start = _find_before(data, _RANK_MARKER, cursor, group["end"])
            if rank_start < 0:
                break
            nxt = (_find_before(data, _RANK_MARKER, rank_start + 2, group["end"])
                   if rank < group["rank_count"] else -1)
            rank_end = nxt if nxt >= 0 else group["end"]
            rows.append(_decode_rank(data, group, rank, rank_start, rank_end,
                                     known_rewards, metric_names))
            cursor = rank_end

    rows.sort(key=lambda r: (r["collection"], r["rank"]))
    errors = [r["reason"] for r in rows
              if r["status"] == "blocked" and r["blocker"] != "descriptive"]
    return {"rows": rows, "errors": sorted({e for e in errors if e})}


def _decode_rank(data: bytes, group: dict, rank: int, rank_start: int, rank_end: int,
                 known_rewards: set[str], metric_names: list[str]) -> dict:
    rank_name = RANK_NAMES.get(rank, f"rank{rank}")
    rewards = _reward_ids_in(data[rank_start:rank_end], known_rewards)
    reward_id = _rank_reward(rewards, rank_name)
    row = {
        "collection": _collection_for(reward_id),
        "badge_id": group["badge_id"],
        "reward_id": reward_id,
        "rewards": rewards,
        "rank": rank,
        "rank_name": rank_name,
        "tier": rank if 1 <= rank <= 7 else 0,
        "completion_kind": "",
        "decoder": "",
        "requirement_key": "",
        "label": "",
        "amount": None,
        "difficulty": 0,
        "context": {},
        "offset": rank_start,
        "status": "blocked",
        "blocker": "source_parse_incomplete",
        "reason": "",
    }

    kind_at = _find_before(data, _KIND_MARKER, rank_start, rank_end)
    if kind_at < 0:
        row["reason"] = f"{group['badge_id']} rank {rank}: completion kind marker not found"
        return row
    kind, after_kind = _read_string(data, kind_at + len(_KIND_MARKER), rank_end)
    if kind is None:
        row["reason"] = f"{group['badge_id']} rank {rank}: completion kind unreadable"
        return row
    row["completion_kind"] = kind

    decoded = _decode_payload(data, kind, after_kind, rank_start, rank_end,
                              group["badge_id"], metric_names)
    for key, value in decoded.items():
        if key == "collection":
            row["collection"] = value
        elif key in row:
            row[key] = value
    if row["status"] == "blocked" and not row["reason"]:
        row["reason"] = f"{group['badge_id']} rank {rank}: {row['blocker']}"
    return row
