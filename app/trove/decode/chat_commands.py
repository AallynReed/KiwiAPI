"""chat_commands.json - the chat commands the in-game help lists.

`prefabs/help.binfab` (root) 0 rows `{0 command, 1 description (English text, not a
locale key), 2 argument names}`.
"""
from __future__ import annotations

from app.trove.decode.tree import GameTree
from app.trove.decode.wire import Obj, parse

TITLE = "Chat commands"
OUTPUT = "chat_commands.json"
PREFIXES = ("prefabs/",)
INDENT, FINAL_NEWLINE = 1, True


def build(tree: GameTree) -> dict:
    root = parse(tree.read("prefabs/help.binfab") or b"").root
    rows = []
    for row in (root.leaf.get(0) if root is not None else None) or []:
        r = row.leaf if isinstance(row, Obj) else {}
        if isinstance(r.get(0), str) and r[0]:
            rows.append({"command": r[0], "description": r.get(1) if isinstance(r.get(1), str) else "",
                         "args": [a for a in r.get(2) or [] if isinstance(a, str) and a]})
    return {"commands": rows}


def count(data: dict) -> int:
    return len(data["commands"])
