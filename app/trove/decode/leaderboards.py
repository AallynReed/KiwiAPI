"""leaderboards.json - every leaderboard the client defines.

`prefabs/leaderboard/leaderboards.binfab` (root):

- 1 categories `{0 id, 1 KLeaderboardCategoryType, 3 name key, 4 board ids}`, in
  tab order. Favorites and the two contest tabs list no boards: the server fills
  them.
- 2 boards `{0 id (the board uuid the API returns), 1 name key, 2 icon path,
  3 source {0 kind, 1 [common, own]}}`. Common field 5 is when it resets: 0 never,
  1 daily, 2 weekly - every board titled "daily", "weekly" or "this week" has 1 or 2
  and every lifetime total 0. Common fields 6 and 7 are unproven and left out.
  Own fields by kind:
  - `PlayerMetric` / `playermetric` {0 PlayerMetric ordinal}; `classmetric` adds
    1 the class index. Metric names come from Trove_x64.exe, labels from
    `$Metrics_<name>`.
  - `MetricCollection` (Effort) {0 class index, 1 [{0 metric, 1 points each}]}.
  - `Composite` {1 board ids, 3 KLeaderboardCompositionType: Sum, Max, Min}.
  - `PowerRankClass` {0 class index}; `delveclassdepth` {0 delve type, 1 class
    index, 2 KLeaderboardDelveClassRepresentation}; `DelveDepth`, `DelveThreeTier`
    and `Tower` name their delve or floor, which the board's own name spells out.
  A class index is the boards' release order (`stats._BOARD_CLASS_ORDER`); it names
  the class in the title of all 90 class boards, and is written out as `class`.
"""
from __future__ import annotations

from typing import Any

from app.trove import stats
from app.trove.codexes.badges import EXE_PATH, parse_metric_names
from app.trove.decode.badges import Text
from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, parse

TITLE = "Leaderboards"
OUTPUT = "leaderboards.json"
PREFIXES = ("prefabs/leaderboard/", "languages/en/", EXE_PATH)
INDENT, FINAL_NEWLINE = 1, True

TABLE = "prefabs/leaderboard/leaderboards.binfab"
R_CATEGORIES, R_BOARDS = 1, 2
C_ID, C_NAME, C_BOARDS = 0, 3, 4
B_ID, B_NAME, B_ICON, B_SOURCE = 0, 1, 2, 3
S_KIND, S_FIELDS = 0, 1
COMMON_RESET = 5
RESETS = {0: "never", 1: "daily", 2: "weekly"}
COMPOSITIONS = ("sum", "max", "min")
METRIC_KINDS = {"playermetric", "classmetric"}
CLASS_FIELD = {"classmetric": 1, "metriccollection": 0, "powerrankclass": 0, "delveclassdepth": 1}


def _leaf(v: Any) -> dict:
    return v.leaf if isinstance(v, Obj) else v if isinstance(v, dict) else {}


def _rows(v: Any) -> list[dict]:
    return [_leaf(x) for x in v] if isinstance(v, list) else []


class _Metrics:
    def __init__(self, tree: GameTree, text: Text):
        exe = tree.read(EXE_PATH)
        self.names = parse_metric_names(exe) if exe else []
        self.text = text

    def __call__(self, mid: Any) -> dict:
        name = self.names[mid] if isinstance(mid, int) and 0 < mid < len(self.names) else ""
        out: dict[str, Any] = {"id": mid}
        if name:
            out.update(name=name, label=self.text(f"$Metrics_{name}") or name)
        return out


def _board(row: dict, text: Text, metric: _Metrics) -> dict | None:
    bid, source = row.get(B_ID), row.get(B_SOURCE)
    if not isinstance(bid, int) or not isinstance(source, Obj):
        return None
    src = source.leaf
    kind = src.get(S_KIND)
    kind = kind if isinstance(kind, str) else ""
    parts = src.get(S_FIELDS)
    parts = parts if isinstance(parts, list) else []
    common = _leaf(parts[0]) if parts else {}
    own = _leaf(parts[1]) if len(parts) > 1 else {}
    icon = row.get(B_ICON)
    board: dict[str, Any] = {"id": bid, "name": text(row.get(B_NAME)), "icon": icon if isinstance(icon, str) else "",
                             "kind": kind, "resets": RESETS.get(common.get(COMMON_RESET, -1), "")}
    low = kind.lower()
    ci = own.get(CLASS_FIELD[low]) if low in CLASS_FIELD else None
    if isinstance(ci, int) and 0 <= ci < len(stats._BOARD_CLASS_ORDER):
        board["class"] = stats._BOARD_CLASS_ORDER[ci]
    if low in METRIC_KINDS:
        board["metric"] = metric(own.get(0))
    elif low == "metriccollection":
        board["points"] = [{**metric(p.get(0)), "points": p.get(1)} for p in _rows(own.get(1))
                           if isinstance(p.get(1), (int, float))]
    elif low == "composite":
        ids = own.get(1)
        how = own.get(3)
        board["combines"] = [i for i in ids if isinstance(i, int)] if isinstance(ids, list) else []
        if isinstance(how, int) and 0 <= how < len(COMPOSITIONS):
            board["composition"] = COMPOSITIONS[how]
    return board


def build(tree: GameTree) -> dict:
    root = parse(tree.read(TABLE) or b"").root or Obj()
    text = Text(tree)
    metric = _Metrics(tree, text)
    boards = [b for b in (_board(r, text, metric) for r in _rows(root.get(R_BOARDS))) if b]
    known = {b["id"] for b in boards}
    categories = []
    for c in _rows(root.get(R_CATEGORIES)):
        ids = [i for i in c.get(C_BOARDS) or [] if isinstance(i, int) and i in known]
        if ids:
            categories.append({"id": c.get(C_ID), "name": text(c.get(C_NAME)), "boards": ids})
    return {"categories": categories, "boards": boards}


def count(data: dict) -> int:
    return len(data["boards"])
