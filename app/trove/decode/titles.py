"""titles.json - every title the client can show, with how to get it.

No prefab defines titles; the client's catalogue is the text table
`languages/en/prefabs_titles.binfab`, keyed `$prefabs_titles_<id>` (the title text)
or `$prefabs_titles_<id>_name`, with `_male_name`/`_female_name` for the gendered
pairs and `_description` for how it is earned. Title ids named elsewhere (recipe
requirements, title items) are only partly in this table - the rest reach the
client from the server - so the table is taken as the list, and nothing is joined
to it by guessing.
"""
from __future__ import annotations

import re

from app.trove.decode.common import locale
from app.trove.decode.tree import GameTree

TITLE = "Titles"
OUTPUT = "titles.json"
PREFIXES = ("languages/en/",)
INDENT, FINAL_NEWLINE = 4, False

KEY_PREFIX = "$prefabs_titles_"
PART = re.compile(r"^(.*?)_(male_name|female_name|name|description)$")


def build(tree: GameTree) -> list[dict]:
    titles: dict[str, dict[str, str]] = {}
    for key, text in locale(tree.read("languages/en/prefabs_titles.binfab")).items():
        if not key.startswith(KEY_PREFIX):
            continue
        rest = key[len(KEY_PREFIX):]
        m = PART.match(rest)
        tid, part = (m.group(1), m.group(2)) if m else (rest, "name")
        titles.setdefault(tid, {})[part] = text
    out = []
    for tid, parts in titles.items():
        row = {"id": tid, "name": parts.get("name", "")}
        if parts.get("male_name") or parts.get("female_name"):
            row["male"], row["female"] = parts.get("male_name", ""), parts.get("female_name", "")
            row["name"] = row["name"] or row["male"]
        if parts.get("description"):
            row["description"] = parts["description"]
        if row["name"]:
            out.append(row)
    return sorted(out, key=lambda r: (r["name"].lstrip(", ").lower(), r["id"]))


def count(data: list) -> int:
    return len(data)
