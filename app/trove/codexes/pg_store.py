"""Postgres data-access for the codexes scope (the ``codex_entry`` table).

Raw asyncpg, mirroring the leaderboards / market ``pg_store`` style. The read side
(``app/trove/codexes/read.py``) and the indexer call these; the JSONB ``data``
column round-trips as a Python dict via the pool's jsonb codec. Reads run only in
the API process (which has Postgres).
"""
from __future__ import annotations

from app.core.postgres import acquire
from app.trove.codexes.models import COLUMNS

DEFAULT_SORT = "name"

# Public sort key -> SQL ORDER BY. Keys are the whitelist the router validates
# against (`read.SORTS`); nullable scalars sort NULLS LAST so unset values sink.
SORTS: dict[str, str] = {
    "name": "name ASC",
    "-name": "name DESC",
    "category": "category ASC, name ASC",
    "-category": "category DESC, name ASC",
    "mastery": "mastery ASC NULLS LAST, name ASC",
    "-mastery": "mastery DESC NULLS LAST, name ASC",
    "mastery_geode": "mastery_geode ASC NULLS LAST, name ASC",
    "-mastery_geode": "mastery_geode DESC NULLS LAST, name ASC",
    "power_rank": "power_rank ASC NULLS LAST, name ASC",
    "-power_rank": "power_rank DESC NULLS LAST, name ASC",
    "indexed_at": "indexed_at ASC, name ASC",
    "-indexed_at": "indexed_at DESC, name ASC",
}

_SELECT_COLS = ("codex_type", "path", "name", "category", "description", "tradable",
                "mastery", "mastery_geode", "power_rank", "blueprint", "data", "indexed_at")

_INSERT = (
    f"INSERT INTO codex_entry ({', '.join(COLUMNS)}) "
    f"VALUES ({', '.join(f'${i + 1}' for i in range(len(COLUMNS)))}) "
    "ON CONFLICT (branch, path) DO UPDATE SET "
    + ", ".join(f"{c} = EXCLUDED.{c}" for c in COLUMNS if c not in ("branch", "path"))
)


def order_by(sort: str) -> str:
    return SORTS.get(sort, SORTS[DEFAULT_SORT])


def _escape_like(term: str) -> str:
    """Escape LIKE metacharacters so user input matches literally (ESCAPE '\\')."""
    return term.replace("\\", "\\\\").replace("%", "\\%").replace("_", "\\_")


def build_filter(
    branch: str, *, codex_type: str | None = None, search: str | None = None,
    category: str | None = None, tradable: bool | None = None,
) -> tuple[str, list]:
    """Build the ``WHERE`` body (no leading WHERE) + positional args. Every filter
    is optional and ANDed; ``search`` is a case-insensitive name/description
    substring. Pure - unit-tested without a DB."""
    conds = ["branch = $1"]
    args: list = [branch]

    def add(expr: str, val) -> None:
        args.append(val)
        conds.append(expr.format(n=len(args)))

    if codex_type:
        add("codex_type = ${n}", codex_type)
    if category:
        add("category = ${n}", category)
    if tradable is not None:
        add("tradable = ${n}", tradable)
    if search:
        args.append("%" + _escape_like(search) + "%")
        n = len(args)
        conds.append(f"(name ILIKE ${n} ESCAPE '\\' OR description ILIKE ${n} ESCAPE '\\')")
    return " AND ".join(conds), args


# --- writes (indexer) -------------------------------------------------------

async def upsert_entries(rows: list[tuple]) -> int:
    """Bulk UPSERT codex rows by ``(branch, path)`` - rewrites every column from the
    freshly parsed prefab (the source binfab is the truth)."""
    if not rows:
        return 0
    async with acquire() as con:
        await con.executemany(_INSERT, rows)
    return len(rows)


async def delete_entries(branch: str, paths: list[str]) -> int:
    """Delete the given source paths for a branch (removed/stale prefabs)."""
    if not paths:
        return 0
    async with acquire() as con:
        await con.execute("DELETE FROM codex_entry WHERE branch = $1 AND path = ANY($2)", branch, paths)
    return len(paths)


async def existing_shas(branch: str) -> dict[str, str]:
    """path -> current ``content_sha256`` for a branch (to skip unchanged prefabs
    and prune removed ones on a full reindex)."""
    async with acquire() as con:
        rows = await con.fetch("SELECT path, content_sha256 FROM codex_entry WHERE branch = $1", branch)
    return {r["path"]: r["content_sha256"] for r in rows}


async def has_any(branch: str) -> bool:
    """Whether the branch has any indexed entry (drives full-build vs delta)."""
    async with acquire() as con:
        return bool(await con.fetchval("SELECT EXISTS (SELECT 1 FROM codex_entry WHERE branch = $1)", branch))


async def branch_count(branch: str) -> int:
    """Number of indexed entries for a branch."""
    async with acquire() as con:
        return int(await con.fetchval("SELECT count(*) FROM codex_entry WHERE branch = $1", branch) or 0)


async def get_parser_version(branch: str) -> int:
    """Parser version that last (re)built the branch (0 if never recorded)."""
    async with acquire() as con:
        return int(await con.fetchval("SELECT parser_version FROM codex_meta WHERE branch = $1", branch) or 0)


async def set_parser_version(branch: str, version: int) -> None:
    """Record the parser version that just (re)built the branch."""
    async with acquire() as con:
        await con.execute(
            "INSERT INTO codex_meta (branch, parser_version, updated_at) VALUES ($1, $2, now()) "
            "ON CONFLICT (branch) DO UPDATE SET parser_version = EXCLUDED.parser_version, updated_at = now()",
            branch, version,
        )


async def touch_meta(branch: str) -> None:
    """Bump a branch's ``updated_at`` (no parser-version change) - the change signal
    a delta reindex emits so caches keyed on it (the rig map) know to refresh."""
    async with acquire() as con:
        await con.execute("UPDATE codex_meta SET updated_at = now() WHERE branch = $1", branch)


async def meta_signature(branch: str) -> tuple:
    """``(parser_version, updated_at)`` for a branch - a cheap cache key that changes
    on every (re)index. ``(0, None)`` if the branch has no meta row yet."""
    async with acquire() as con:
        row = await con.fetchrow(
            "SELECT parser_version, updated_at FROM codex_meta WHERE branch = $1", branch)
    return (row["parser_version"], row["updated_at"]) if row else (0, None)


# --- rig map (rig_binding table) --------------------------------------------

async def replace_rig_bindings(branch: str, rows: list[tuple]) -> int:
    """Atomically replace a branch's rig bindings. ``rows`` = ``(branch, blueprint,
    skeleton, ap_key)`` tuples (built by ``reindex_rigs`` from the prefab binfabs)."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute("DELETE FROM rig_binding WHERE branch = $1", branch)
            if rows:
                await con.executemany(
                    "INSERT INTO rig_binding (branch, blueprint, skeleton, ap_key) "
                    "VALUES ($1, $2, $3, $4) ON CONFLICT (branch, blueprint) DO UPDATE "
                    "SET skeleton = EXCLUDED.skeleton, ap_key = EXCLUDED.ap_key",
                    rows,
                )
    return len(rows)


async def upsert_rig_bindings(rows: list[tuple]) -> int:
    """Insert/update rig bindings (the delta path) without clearing the branch.
    ``rows`` = ``(branch, blueprint, skeleton, ap_key)``."""
    if not rows:
        return 0
    async with acquire() as con:
        await con.executemany(
            "INSERT INTO rig_binding (branch, blueprint, skeleton, ap_key) "
            "VALUES ($1, $2, $3, $4) ON CONFLICT (branch, blueprint) DO UPDATE "
            "SET skeleton = EXCLUDED.skeleton, ap_key = EXCLUDED.ap_key",
            rows,
        )
    return len(rows)


async def rig_binding_count(branch: str) -> int:
    """Number of rig bindings for a branch (0 => needs a first build)."""
    async with acquire() as con:
        return int(await con.fetchval("SELECT count(*) FROM rig_binding WHERE branch = $1", branch) or 0)


async def get_rig_version(branch: str) -> int:
    """Rig-extractor version that last (re)built the branch's rig map (0 if never)."""
    async with acquire() as con:
        return int(await con.fetchval("SELECT rig_version FROM codex_meta WHERE branch = $1", branch) or 0)


async def set_rig_version(branch: str, version: int) -> None:
    """Record the rig-extractor version that just rebuilt the branch's rig map."""
    async with acquire() as con:
        await con.execute(
            "INSERT INTO codex_meta (branch, rig_version, updated_at) VALUES ($1, $2, now()) "
            "ON CONFLICT (branch) DO UPDATE SET rig_version = EXCLUDED.rig_version, updated_at = now()",
            branch, version,
        )


async def load_rig_map(branch: str) -> dict[str, tuple[str, str]]:
    """``blueprint basename -> (skeleton stem, AP key)`` for every skeleton-binding
    creature/costume/mob in a branch - the authoritative map the Mods Hub viewer
    resolves a mod's blueprints against."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT blueprint, skeleton, ap_key FROM rig_binding WHERE branch = $1", branch)
    return {r["blueprint"]: (r["skeleton"], r["ap_key"]) for r in rows}


# --- reads (router via read.py) ---------------------------------------------

async def query_entries(
    branch: str, *, codex_type: str | None = None, search: str | None = None,
    category: str | None = None, tradable: bool | None = None,
    sort: str = DEFAULT_SORT, limit: int = 50, offset: int = 0,
) -> tuple[list[dict], int]:
    """Filtered, sorted, paged entries + the total match count."""
    where, args = build_filter(branch, codex_type=codex_type, search=search,
                               category=category, tradable=tradable)
    async with acquire() as con:
        total = await con.fetchval(f"SELECT count(*) FROM codex_entry WHERE {where}", *args)
        rows = await con.fetch(
            f"SELECT {', '.join(_SELECT_COLS)} FROM codex_entry WHERE {where} "
            f"ORDER BY {order_by(sort)} LIMIT ${len(args) + 1} OFFSET ${len(args) + 2}",
            *args, limit, offset,
        )
    return [dict(r) for r in rows], int(total or 0)


async def type_counts(branch: str) -> list[dict]:
    """Per-type entry counts for a branch, ordered by type."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT codex_type AS type, count(*) AS count FROM codex_entry "
            "WHERE branch = $1 GROUP BY codex_type ORDER BY codex_type",
            branch,
        )
    return [{"type": r["type"], "count": int(r["count"])} for r in rows]


async def list_categories(branch: str, codex_type: str | None = None) -> list[dict]:
    """Distinct non-empty categories (+ counts) for filter dropdowns, A->Z."""
    conds = ["branch = $1", "category <> ''"]
    args: list = [branch]
    if codex_type:
        args.append(codex_type)
        conds.append(f"codex_type = ${len(args)}")
    async with acquire() as con:
        rows = await con.fetch(
            f"SELECT category, count(*) AS count FROM codex_entry "
            f"WHERE {' AND '.join(conds)} GROUP BY category ORDER BY category",
            *args,
        )
    return [{"category": r["category"], "count": int(r["count"])} for r in rows]


async def get_entry(branch: str, codex_type: str, path: str) -> dict | None:
    """A single entry by ``(branch, codex_type, path)``."""
    async with acquire() as con:
        row = await con.fetchrow(
            f"SELECT {', '.join(_SELECT_COLS)} FROM codex_entry "
            "WHERE branch = $1 AND codex_type = $2 AND path = $3",
            branch, codex_type, path,
        )
    return dict(row) if row else None


async def reset(branch: str | None = None) -> int:
    """Wipe a branch's entries (or all). Returns the prior row count - for a clean
    rebuild from the archive."""
    async with acquire() as con:
        if branch is None:
            n = await con.fetchval("SELECT count(*) FROM codex_entry")
            await con.execute("TRUNCATE codex_entry")
        else:
            n = await con.fetchval("SELECT count(*) FROM codex_entry WHERE branch = $1", branch)
            await con.execute("DELETE FROM codex_entry WHERE branch = $1", branch)
    return int(n or 0)
