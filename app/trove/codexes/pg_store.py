"""Postgres data-access for the codexes scope (the ``codex_entry`` table).

Raw asyncpg, mirroring the leaderboards / market ``pg_store`` style. The read side
(``app/trove/codexes/read.py``) and the indexer call these; the JSONB ``data``
column round-trips as a Python dict via the pool's jsonb codec. Reads run only in
the API process (which has Postgres).
"""
from __future__ import annotations

from app.core.postgres import acquire
from app.trove.codexes.models import (
    ABILITY_COLUMNS,
    COLUMNS,
    LINK_COLUMNS,
    REQUIREMENT_COLUMNS,
    STAT_COLUMNS,
    UPGRADE_COLUMNS,
)

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


# --- child tables (stats / abilities / links) --------------------------------
#
# All three are OWNED by a source prefab, so they are replaced scoped by path rather
# than upserted: a prefab that LOSES a stat in a game update has to actually lose the
# row, and an upsert can only ever add or overwrite. `paths` is the full set that was
# re-parsed - including prefabs that now yield nothing - so their rows are cleared too.

def _insert(table: str, columns: tuple[str, ...]) -> str:
    return (f"INSERT INTO {table} ({', '.join(columns)}) "
            f"VALUES ({', '.join(f'${i + 1}' for i in range(len(columns)))}) "
            "ON CONFLICT DO NOTHING")


_STAT_INSERT = _insert("codex_stat", STAT_COLUMNS)
_ABILITY_INSERT = _insert("codex_ability", ABILITY_COLUMNS)
_LINK_INSERT = _insert("codex_link", LINK_COLUMNS)
_REQUIREMENT_INSERT = _insert("codex_requirement", REQUIREMENT_COLUMNS)
_UPGRADE_INSERT = _insert("codex_upgrade", UPGRADE_COLUMNS)


async def replace_children(branch: str, paths: list[str], *, stats: list[tuple],
                           abilities: list[tuple], links: list[tuple]) -> None:
    """Re-state the stat/ability/link rows of specific prefabs, in one transaction."""
    if not paths:
        return
    async with acquire() as con:
        async with con.transaction():
            await con.execute(
                "DELETE FROM codex_stat WHERE branch = $1 AND path = ANY($2::text[])",
                branch, paths)
            await con.execute(
                "DELETE FROM codex_ability WHERE branch = $1 AND path = ANY($2::text[])",
                branch, paths)
            await con.execute(
                "DELETE FROM codex_link WHERE branch = $1 AND src_path = ANY($2::text[])",
                branch, paths)
            if stats:
                await con.executemany(_STAT_INSERT, stats)
            if abilities:
                await con.executemany(_ABILITY_INSERT, abilities)
            if links:
                await con.executemany(_LINK_INSERT, links)


async def replace_links_for(branch: str, relation: str, rows: list[tuple]) -> int:
    """Atomically replace every edge of one relation for a branch.

    Used for the relations that come from a single shared table rather than from a
    per-prefab parse (`unlocks`, `upgrade_cost`, `member_of`) - those have no owning
    prefab to scope a delete by, so the relation itself is the scope."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute(
                "DELETE FROM codex_link WHERE branch = $1 AND rel = $2", branch, relation)
            if rows:
                await con.executemany(_LINK_INSERT, rows)
    return len(rows)


async def replace_requirements(branch: str, rows: list[tuple]) -> int:
    """Atomically replace a branch's badge requirements (one source file, so the whole
    branch is the right scope)."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute("DELETE FROM codex_requirement WHERE branch = $1", branch)
            if rows:
                await con.executemany(_REQUIREMENT_INSERT, rows)
    return len(rows)


async def replace_upgrades(branch: str, rows: list[tuple]) -> int:
    """Atomically replace a branch's progression-tree nodes."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute("DELETE FROM codex_upgrade WHERE branch = $1", branch)
            if rows:
                await con.executemany(_UPGRADE_INSERT, rows)
    return len(rows)


async def delete_entries(branch: str, paths: list[str]) -> int:
    """Delete the given source paths for a branch (removed/stale prefabs).

    Cascades to the child tables by hand - there is no FK, because a link's target is
    allowed to be a path that was never indexed as an entry, and an FK would forbid
    exactly the edges that are most worth keeping."""
    if not paths:
        return 0
    async with acquire() as con:
        async with con.transaction():
            await con.execute(
                "DELETE FROM codex_entry WHERE branch = $1 AND path = ANY($2::text[])",
                branch, paths)
            await con.execute(
                "DELETE FROM codex_stat WHERE branch = $1 AND path = ANY($2::text[])",
                branch, paths)
            await con.execute(
                "DELETE FROM codex_ability WHERE branch = $1 AND path = ANY($2::text[])",
                branch, paths)
            await con.execute(
                "DELETE FROM codex_link WHERE branch = $1 AND src_path = ANY($2::text[])",
                branch, paths)
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

_RIG_INSERT = (
    "INSERT INTO rig_binding (branch, prefab, blueprint, skeleton, ap_key) "
    "VALUES ($1, $2, $3, $4, $5) ON CONFLICT (branch, prefab, blueprint) DO UPDATE "
    "SET skeleton = EXCLUDED.skeleton, ap_key = EXCLUDED.ap_key"
)


async def replace_rig_bindings(branch: str, rows: list[tuple]) -> int:
    """Atomically replace a branch's rig bindings. ``rows`` = ``(branch, prefab,
    blueprint, skeleton, ap_key)`` tuples (built by ``reindex_rigs`` from the prefab
    binfabs)."""
    async with acquire() as con:
        async with con.transaction():
            await con.execute("DELETE FROM rig_binding WHERE branch = $1", branch)
            if rows:
                await con.executemany(_RIG_INSERT, rows)
    return len(rows)


async def replace_prefab_rig_bindings(
    branch: str, prefabs: list[str], rows: list[tuple],
) -> int:
    """Re-state the bindings of specific PREFABS (the delta path): drop what those
    prefabs said before, then insert what they say now, in one transaction.

    Scoped by prefab rather than upserted blindly, so a creature that LOSES a part in a
    game update actually loses it - the row can't linger and get assembled onto the
    model forever. ``prefabs`` is the full set that was re-parsed (including any that
    now yield no bindings at all), not just the ones that produced ``rows``."""
    if not prefabs:
        return 0
    async with acquire() as con:
        async with con.transaction():
            await con.execute(
                "DELETE FROM rig_binding WHERE branch = $1 AND prefab = ANY($2::text[])",
                branch, prefabs,
            )
            if rows:
                await con.executemany(_RIG_INSERT, rows)
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


async def load_rig_bindings(branch: str) -> list[tuple[str, str, str, str]]:
    """Every ``(prefab, blueprint basename, skeleton stem, AP key)`` binding in a branch
    - the authoritative map the Mods Hub viewer resolves a mod's blueprints against, and
    the embed groups a native creature's parts by.

    Returned flat and ORDERED (prefab, blueprint): ``rig_index`` builds both of its
    indexes from one pass, and a stable order makes the "which creature owns this part"
    answer stable across processes when a part is shared by more than one prefab."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT prefab, blueprint, skeleton, ap_key FROM rig_binding "
            "WHERE branch = $1 ORDER BY prefab, blueprint", branch)
    return [(r["prefab"], r["blueprint"], r["skeleton"], r["ap_key"]) for r in rows]


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


async def all_recipes(branch: str) -> list[dict]:
    """Every recipe entry for a branch - ``(path, name, category, data)`` only.
    The crafting calculator inverts these into an ``output_path -> recipe`` map to
    walk dependency trees, so it needs the whole set at once (not paged)."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT path, name, category, data FROM codex_entry "
            "WHERE branch = $1 AND codex_type = 'recipe'",
            branch,
        )
    return [dict(r) for r in rows]


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


def _item_role_rank(path: str) -> int:
    """Lower = more representative of the *tradable item* of a given name. A display
    name can sit on several prefab roles - the plain ``item/`` token (its inventory
    icon), the equip-variant ``item/mount/`` form, the ``collections/`` body (the 3D
    model you receive), plus ``_debug``/``_notrade`` flag variants. A market listing
    sells the item, so the item token's icon is the right thumbnail; the others are
    only used when nothing better carries the name. Pure - unit-tested without a DB."""
    p = (path or "").lower()
    is_item = p.startswith("prefabs/item/")
    if is_item and "/mount/" not in p:
        # Base item token beats its own _debug/_notrade alt-flag copies.
        return 1 if ("_debug" in p or "_notrade" in p) else 0
    if is_item:                                  # prefabs/item/mount/… (equip variant)
        return 2
    if p.startswith("prefabs/collections/"):     # the 3D collection/mount body
        return 3
    return 4


async def blueprints_for_names(branch: str, names: list[str]) -> dict[str, str]:
    """Map each requested display name (case-insensitive) to the blueprint that best
    represents the tradable item of that name.

    A name that appears on several prefab roles resolves to the highest-ranked one
    (see ``_item_role_rank``) - so a piñata renders its inventory icon, not its mount
    body. A name with no blueprinted entry is omitted, and one whose *top* role still
    disagrees on the blueprint (two genuinely different items share a name and role)
    is dropped as ambiguous: a wrong thumbnail is worse than none. Keys are the
    lower-cased names for stable lookup.
    """
    if not names:
        return {}
    lowered = list({n.lower() for n in names})
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT lower(name) AS lname, blueprint, path FROM codex_entry "
            "WHERE branch = $1 AND blueprint IS NOT NULL AND blueprint <> '' "
            "AND lower(name) = ANY($2::text[])",
            branch, lowered,
        )
    # Collect every (role rank, blueprint) candidate per name...
    cands: dict[str, list[tuple[int, str]]] = {}
    for r in rows:
        cands.setdefault(r["lname"], []).append(
            (_item_role_rank(r["path"]), r["blueprint"]))
    # ...then take the best-ranked role; keep it only if that role is unanimous.
    out: dict[str, str] = {}
    for lname, items in cands.items():
        best = min(rank for rank, _ in items)
        top_bps = {bp for rank, bp in items if rank == best}
        if len(top_bps) == 1:
            out[lname] = next(iter(top_bps))
        # else: conflicting blueprints even at the top role -> ambiguous, omit
    return out


async def get_entry(branch: str, codex_type: str, path: str) -> dict | None:
    """A single entry by ``(branch, codex_type, path)``."""
    async with acquire() as con:
        row = await con.fetchrow(
            f"SELECT {', '.join(_SELECT_COLS)} FROM codex_entry "
            "WHERE branch = $1 AND codex_type = $2 AND path = $3",
            branch, codex_type, path,
        )
    return dict(row) if row else None


async def links_for(branch: str, path: str, *, direction: str = "out",
                    relation: str | None = None, limit: int = 200) -> list[dict]:
    """Edges touching `path`, joined to the far end's entry so a caller gets a usable
    row (name, type, blueprint) rather than a bare path.

    ``direction="out"`` reads `path` as the source ("what does this recipe use");
    ``"in"`` reads it as the target ("what recipes produce this item"). The far end is
    LEFT-joined - a link whose target isn't an indexed prefab is still a real edge and
    is returned with empty display fields rather than dropped.
    """
    outward = direction != "in"
    near, far = ("src_path", "dst_path") if outward else ("dst_path", "src_path")
    conds = ["l.branch = $1", f"l.{near} = $2"]
    args: list = [branch, path]
    if relation:
        args.append(relation)
        conds.append(f"l.rel = ${len(args)}")
    args.append(limit)
    async with acquire() as con:
        rows = await con.fetch(
            f"SELECT l.rel, l.{far} AS path, l.ord, l.qty, l.data, "
            f"       e.codex_type, e.name, e.category, e.blueprint "
            f"FROM codex_link l "
            f"LEFT JOIN codex_entry e ON e.branch = l.branch AND e.path = l.{far} "
            f"WHERE {' AND '.join(conds)} "
            f"ORDER BY l.rel, l.ord LIMIT ${len(args)}",
            *args,
        )
    return [dict(r) for r in rows]


async def entries_with_stat(branch: str, stat_key: str, *, codex_type: str | None = None,
                            limit: int = 50, offset: int = 0) -> tuple[list[dict], int]:
    """Entries granting a stat, strongest first - the query the JSONB blob couldn't
    serve. A prefab granting the stat more than once contributes its BEST row, so an
    entry appears once and sorts by what it actually gives."""
    conds = ["s.branch = $1", "s.stat_key = $2"]
    args: list = [branch, stat_key]
    if codex_type:
        args.append(codex_type)
        conds.append(f"e.codex_type = ${len(args)}")
    where = " AND ".join(conds)
    async with acquire() as con:
        total = await con.fetchval(
            f"SELECT count(DISTINCT s.path) FROM codex_stat s "
            f"JOIN codex_entry e ON e.branch = s.branch AND e.path = s.path WHERE {where}",
            *args)
        rows = await con.fetch(
            f"SELECT DISTINCT ON (s.path) s.path, s.stat_key, s.stat_name, s.value, "
            f"       s.is_percent, s.slot_key, e.codex_type, e.name, e.category, e.blueprint "
            f"FROM codex_stat s "
            f"JOIN codex_entry e ON e.branch = s.branch AND e.path = s.path "
            f"WHERE {where} ORDER BY s.path, s.value DESC NULLS LAST",
            *args)
    ordered = sorted([dict(r) for r in rows],
                     key=lambda r: (r["value"] is None, -(r["value"] or 0), r["name"]))
    return ordered[offset:offset + limit], int(total or 0)


async def requirements_for(branch: str, collection: str) -> list[dict]:
    """The full rank ladder the given badge belongs to, bronze first.

    Each RANK is its own collection (`…/blocks_bronze`, `…/blocks_silver`, …), so
    filtering on the collection alone returns exactly one row - a "ranks" list with
    one entry in it. The ladder is keyed by `badge_id`, so this resolves the badge
    from whichever rank was asked about and returns all of its ranks, which is what
    makes the section worth showing."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT rank, rank_name, badge_id, collection, completion_kind, "
            "       requirement_key, label, amount, difficulty, status, context "
            "FROM codex_requirement "
            "WHERE branch = $1 AND badge_id = ("
            "    SELECT badge_id FROM codex_requirement "
            "    WHERE branch = $1 AND collection = $2 LIMIT 1) "
            "ORDER BY rank",
            branch, collection)
    return [dict(r) for r in rows]


async def upgrade_system(branch: str, system_key: str) -> list[dict]:
    """Every node of one progression tree, in rank then key order."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT system_kind, system_key, node_key, rank, source_path, costs, requires "
            "FROM codex_upgrade WHERE branch = $1 AND system_key = $2 "
            "ORDER BY rank NULLS LAST, node_key",
            branch, system_key)
    return [dict(r) for r in rows]


async def upgrade_systems(branch: str) -> list[dict]:
    """The progression systems present in a branch, with node counts."""
    async with acquire() as con:
        rows = await con.fetch(
            "SELECT system_kind, system_key, count(*) AS nodes FROM codex_upgrade "
            "WHERE branch = $1 GROUP BY system_kind, system_key "
            "ORDER BY system_kind, system_key", branch)
    return [{"system_kind": r["system_kind"], "system_key": r["system_key"],
             "nodes": int(r["nodes"])} for r in rows]


async def child_counts(branch: str) -> dict[str, int]:
    """Row counts of every codex table for a branch - the admin/status readout."""
    async with acquire() as con:
        row = await con.fetchrow(
            "SELECT (SELECT count(*) FROM codex_entry WHERE branch = $1) AS entries, "
            "       (SELECT count(*) FROM codex_stat WHERE branch = $1) AS stats, "
            "       (SELECT count(*) FROM codex_ability WHERE branch = $1) AS abilities, "
            "       (SELECT count(*) FROM codex_link WHERE branch = $1) AS links, "
            "       (SELECT count(*) FROM codex_requirement WHERE branch = $1) AS requirements, "
            "       (SELECT count(*) FROM codex_upgrade WHERE branch = $1) AS upgrades",
            branch)
    return {k: int(v or 0) for k, v in dict(row).items()} if row else {}


# Every codex table, so a reset clears the whole set rather than leaving orphaned
# stats/links behind pointing at entries that no longer exist.
_ALL_TABLES = ("codex_stat", "codex_ability", "codex_link", "codex_requirement",
               "codex_upgrade", "codex_entry")


async def reset(branch: str | None = None) -> int:
    """Wipe a branch's codex (or all of it). Returns the prior entry count - for a
    clean rebuild from the archive."""
    async with acquire() as con:
        async with con.transaction():
            if branch is None:
                n = await con.fetchval("SELECT count(*) FROM codex_entry")
                await con.execute("TRUNCATE " + ", ".join(_ALL_TABLES))
            else:
                n = await con.fetchval(
                    "SELECT count(*) FROM codex_entry WHERE branch = $1", branch)
                for table in _ALL_TABLES:
                    await con.execute(f"DELETE FROM {table} WHERE branch = $1", branch)
    return int(n or 0)
