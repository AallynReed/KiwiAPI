"""Trove file-placement rules for the Mods Hub (ported from BetterTroveTools).

Trove only loads override files that sit under one of its known top-level
folders at the SAME relative path the base game uses. So:

  - **Only files inside a known Trove folder are compiled.** A file at the repo
    root, or inside a non-Trove folder (``bin/``, ``.git`` housekeeping, …), is
    ignored by the build. (`Directories` in BTT = these 11 folders.)
  - A handful of **non-content extensions are never compiled** (archives,
    binaries, configs, docs…), matching BTT's ``ignored_extensions``.
  - **Auto-fix:** a file whose name matches a real game file but sits at the
    wrong path is *misplaced*; we look the name up in the game's file index
    (the updates archive's ``UpdateState``) and move it to the game's path.

The compile filter needs no game data (pure path rules); the misplaced check +
auto-fix need the updates archive populated, and degrade to "unavailable" if it
isn't.
"""

from __future__ import annotations

# The 11 top-level folders Trove overrides load from (BTT's Directories enum).
TROVE_DIRECTORIES: frozenset[str] = frozenset({
    "audio", "blueprints", "fonts", "languages", "models", "movies",
    "particles", "prefabs", "shadersunified", "textures", "ui",
})

# Extensions never included in a build (archives, binaries, tooling, docs).
COMPILE_IGNORED_EXTENSIONS: frozenset[str] = frozenset({
    ".tfi", ".tfa", ".exe", ".dll", ".tmod", ".zip", ".cfg",
    ".txt", ".log", ".ini", ".toml", ".json", ".xml", ".dat",
})

# The live game branch whose file tree defines correct placement.
LIVE_BRANCH = "live-us"


def _ext(path: str) -> str:
    name = path.rsplit("/", 1)[-1]
    return ("." + name.rsplit(".", 1)[1].lower()) if "." in name else ""


def is_compilable(path: str) -> bool:
    """True iff the file is inside a known Trove folder and not an ignored type."""
    parts = path.split("/")
    if len(parts) < 2:                       # root file -> not compiled
        return False
    if parts[0].lower() not in TROVE_DIRECTORIES:
        return False
    return _ext(path) not in COMPILE_IGNORED_EXTENSIONS


def skip_reason(path: str) -> str:
    """Why a non-compilable file is skipped (for the warning UI)."""
    parts = path.split("/")
    if len(parts) < 2:
        return "root file (only files inside a Trove folder are compiled)"
    top = parts[0].lower()
    if top not in TROVE_DIRECTORIES:
        return f"'{parts[0]}' is not a Trove folder"
    if _ext(path) in COMPILE_IGNORED_EXTENSIONS:
        return f"'{_ext(path)}' files aren't included in a build"
    return "ignored"


def classify(paths: list[str]) -> tuple[list[str], list[dict]]:
    """Split paths into (compilable, skipped[{path, reason}])."""
    compilable: list[str] = []
    skipped: list[dict] = []
    for p in paths:
        if is_compilable(p):
            compilable.append(p)
        else:
            skipped.append({"path": p, "reason": skip_reason(p)})
    return compilable, skipped


# Cached game-file index, rebuilt only when the live branch's current build
# changes (keyed by ordinal). Building it scans the whole UpdateState tree, so we
# do it at most once per game update.
_GAME_MAP_CACHE: dict = {"ordinal": None, "map": {}, "all": {}}


async def game_file_map(branch: str = LIVE_BRANCH) -> dict[str, str]:
    """``{filename.lower(): canonical game path}`` from the updates archive's
    current tree. Empty dict when the archive is off/unpopulated, in which case
    the misplaced check + auto-fix are simply unavailable (the compile filter
    still works - it needs no game data)."""
    from app.core.database import get_db
    from app.trove.updates.models import UpdateBranch

    ub = await UpdateBranch.find_one(UpdateBranch.branch == branch)
    if ub is None or not ub.current_ordinal:
        return {}
    if _GAME_MAP_CACHE["ordinal"] == ub.current_ordinal and _GAME_MAP_CACHE["map"]:
        return _GAME_MAP_CACHE["map"]
    fmap: dict[str, str] = {}
    # Projected raw-collection scan (path only) - avoid Beanie hydration on a
    # potentially huge tree (see the leaderboards hydration lesson).
    cursor = get_db()["update_state"].find({"branch": branch}, {"path": 1, "_id": 0})
    async for doc in cursor:
        path = doc.get("path")
        if not path:
            continue
        name = path.rsplit("/", 1)[-1].lower()
        if name not in fmap:                  # first occurrence wins (matches BTT)
            fmap[name] = path
        _GAME_MAP_CACHE["all"].setdefault(name, []).append(path)
    _GAME_MAP_CACHE["ordinal"] = ub.current_ordinal
    _GAME_MAP_CACHE["map"] = fmap
    return fmap


async def game_file_paths(branch: str = LIVE_BRANCH) -> dict[str, list[str]]:
    """``{filename.lower(): [every archived path with that name]}``.

    ``game_file_map`` keeps only the first sighting, which is fine for "does this file
    exist in the game", but not for "which of these is THIS creature's". Trove reuses a
    filename across skins and NPC sets - ``equipment_helm_crimefighter.blueprint`` exists
    both under the beequeen skin and under a merchant-hub NPC - so assembling a creature
    has to choose, and choosing by insertion order attaches a merchant NPC to a costume's
    head. Pair this with ``nearest_path``."""
    await game_file_map(branch)               # populates both caches together
    return _GAME_MAP_CACHE["all"]


def nearest_path(candidates: list[str], hint: str) -> str | None:
    """The candidate that lives closest to ``hint`` - the creature's own prefab path.

    Scored on how many directory segments the two share, so a part under
    ``…/skins/crimefighter_beequeen/`` wins for a prefab under the same skin folder over
    an identically-named file in an unrelated NPC set. Ties break lexicographically so
    the answer is stable rather than dependent on archive order."""
    if not candidates:
        return None
    if len(candidates) == 1 or not hint:
        return sorted(candidates)[0]
    want = {s for s in hint.replace("\\", "/").lower().split("/")[:-1] if s}
    def score(p: str) -> tuple:
        segs = [s for s in p.replace("\\", "/").lower().split("/")[:-1] if s]
        return (-len(want & set(segs)), p)
    return sorted(candidates, key=score)[0]


def find_misplaced(paths: list[str], game_map: dict[str, str]) -> list[dict]:
    """Files whose NAME matches a real game file but whose path differs - they
    won't override anything where they sit. Returns ``[{path, expected}]`` (the
    expected path = where the game keeps that file). Covers files in the wrong
    Trove subfolder AND game files stranded at the repo root / in ``bin``."""
    if not game_map:
        return []
    out: list[dict] = []
    for p in paths:
        name = p.rsplit("/", 1)[-1].lower()
        expected = game_map.get(name)
        if expected and expected.lower() != p.lower():
            out.append({"path": p, "expected": expected})
    return out
