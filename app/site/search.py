"""Site-wide search: one query, grouped by subject.

Fans a query out across everything the site can answer for - pages and their tabs,
the codex (collectibles / items / recipes / styles), players and mods - and returns
each subject with a count plus its rows.

Two shapes, one code path:

- the navbar dropdown asks for a *preview*: a few rows per subject, so it can show
  "the best answers across everything" while you type;
- the ``/search`` page asks for *one subject*, paged, plus the counts for every other
  subject so the sidebar can show its badges.

Subjects run CONCURRENTLY. Serially this is the slowest backend plus the second
slowest plus…, and the dropdown is on the critical path of every keystroke.

A subject that errors returns empty rather than failing the whole search - a search
box that dies because one backend is briefly unhappy is worse than one that quietly
answers with less.
"""

from __future__ import annotations

import asyncio
import logging

from app.site import search_index as index
from app.site.search_index import (
    CODEX_TYPE_SUBJECT,
    SUBJECT_COLLECTIONS,
    SUBJECT_ITEMS,
    SUBJECT_MODPACKS,
    SUBJECT_MODS,
    SUBJECT_ORDER,
    SUBJECT_PAGES,
    SUBJECT_PLAYERS,
    SUBJECT_RECIPES,
    SUBJECT_STYLES,
)

logger = logging.getLogger("kiwi.site.search")

# Below this a query matches so much that the results are noise, and the backends do
# real work to produce it.
MIN_QUERY = 2
PREVIEW_PER_SUBJECT = 4
PAGE_SIZE = 30

# subject -> the feature flag that has to be on for it to be searchable at all.
_SUBJECT_FLAG: dict[str, str | None] = {
    SUBJECT_PAGES: None,
    SUBJECT_COLLECTIONS: "codexes_enabled",
    SUBJECT_ITEMS: "codexes_enabled",
    SUBJECT_RECIPES: "codexes_enabled",
    SUBJECT_STYLES: "codexes_enabled",
    SUBJECT_PLAYERS: "leaderboards_enabled",
    SUBJECT_MODS: "mods_hub_enabled",
    SUBJECT_MODPACKS: "mods_hub_enabled",
}

_CODEX_TYPES_BY_SUBJECT: dict[str, list[str]] = {}
for _type, _subject in CODEX_TYPE_SUBJECT.items():
    _CODEX_TYPES_BY_SUBJECT.setdefault(_subject, []).append(_type)


def available_subjects(flags: dict[str, bool]) -> list[str]:
    """Subjects whose feature is enabled, in display order."""
    return [s for s in SUBJECT_ORDER
            if _SUBJECT_FLAG[s] is None or flags.get(_SUBJECT_FLAG[s], False)]


# --- per-subject backends ----------------------------------------------------
#
# Each returns `(rows, total)`. Rows are already display-shaped, so the client
# renders one row template for every subject.

async def _pages(query: str, flags: dict[str, bool], limit: int, offset: int):
    hits = index.search_destinations(query, flags, limit=200)
    rows = [index.to_row(d) for d in hits]
    return rows[offset:offset + limit], len(rows)


async def _codex(query: str, subject: str, branch: str, limit: int, offset: int):
    from urllib.parse import quote

    from app.trove.codexes import read as codexes_read

    types = _CODEX_TYPES_BY_SUBJECT.get(subject) or []
    # One query per subject rather than per type: `query_entries` filters a single
    # type, so a subject spanning 13 of them (collections) would otherwise be 13
    # round-trips for one column of results.
    rows, total = await codexes_read.query_entries(
        branch, search=query, limit=limit, offset=offset, sort="name",
        codex_type=types[0] if len(types) == 1 else None,
    )
    if len(types) > 1:
        allowed = set(types)
        rows = [r for r in rows if r.get("codex_type") in allowed]
    return [
        {
            "name": r.get("name") or "",
            # The codex page deep-links by HASH, not query string (`#type=…&q=…`), and
            # has no "open this entry" parameter - so link to its type tab filtered to
            # the exact name, which puts the entry at the top of the grid. A `?type=`
            # link would be silently ignored and land on the unfiltered page.
            "path": "/codexes#type=" + quote(str(r.get("codex_type") or ""), safe="")
                    + "&q=" + quote(r.get("name") or "", safe=""),
            "detail": r.get("category") or "",
            "kind": r.get("codex_type") or "",
            # A ready-made thumbnail URL rather than the raw blueprint: the dropdown
            # and the results page then render one row shape for every subject and
            # neither has to know how a codex model becomes an image.
            "image": ("/site/codexes/render?dim=64&blueprint="
                      + quote(r["blueprint"], safe="")) if r.get("blueprint") else None,
        }
        for r in rows
    ], total


async def _players(query: str, limit: int, offset: int):
    from urllib.parse import quote

    from app.trove.leaderboards import pg_store as lb_store

    names, total = await lb_store.search_players(query, limit=limit + offset)
    window = names[offset:offset + limit]
    # The player page takes the name as a PATH segment (`/player/{name}`), so it has to
    # be percent-encoded - Trove names allow characters that would otherwise split the
    # path or be read as a query.
    return [
        {"name": n, "path": "/player/" + quote(n, safe=""), "detail": "", "kind": "player"}
        for n in window
    ], total


async def _mods(query: str, limit: int, offset: int, *, modpacks: bool):
    from urllib.parse import quote

    # Imported here, not at module scope: the site router imports this module, so a
    # top-level import of anything that reaches back would be circular.
    if modpacks:
        from app.trove.modpacks import service as packs_service
        rows, total = await packs_service.list_public(q=query, limit=limit, offset=offset)
        root, kind = "/modpacks", "modpack"
    else:
        from app.trove.mods_hub import service as mods_service
        rows, total = await mods_service.list_public(q=query, limit=limit, offset=offset)
        root, kind = "/mods", "mod"
    # Both pages address a project as `/<root>/{handle}/{slug}` - the handle is part of
    # the identity, so a slug alone does not resolve. A row missing either is dropped
    # rather than linked somewhere that 404s.
    out: list[dict] = []
    for r in rows:
        handle, slug = r.get("handle"), r.get("slug")
        if not handle or not slug:
            continue
        sha = r.get("banner_sha") or r.get("preview_sha")
        out.append({
            "name": r.get("title") or slug,
            "path": f"{root}/{handle}/{slug}",
            "detail": r.get("owner_username") or handle,
            "kind": kind,
            "image": ("/site/mods/image/" + quote(str(sha), safe="")) if sha else None,
        })
    return out, total


async def _run(subject: str, query: str, flags: dict[str, bool], branch: str,
               limit: int, offset: int):
    if subject == SUBJECT_PAGES:
        return await _pages(query, flags, limit, offset)
    if subject in _CODEX_TYPES_BY_SUBJECT:
        return await _codex(query, subject, branch, limit, offset)
    if subject == SUBJECT_PLAYERS:
        return await _players(query, limit, offset)
    if subject in (SUBJECT_MODS, SUBJECT_MODPACKS):
        return await _mods(query, limit, offset, modpacks=subject == SUBJECT_MODPACKS)
    return [], 0


async def _safe(subject: str, query: str, flags: dict[str, bool], branch: str,
                limit: int, offset: int):
    try:
        return await _run(subject, query, flags, branch, limit, offset)
    except Exception:  # noqa: BLE001 - one unhappy backend must not kill the search
        logger.warning("site search: subject %r failed for %r", subject, query,
                       exc_info=True)
        return [], 0


# --- public ------------------------------------------------------------------

async def search(query: str, flags: dict[str, bool], *, branch: str,
                 subject: str | None = None, limit: int = PAGE_SIZE,
                 offset: int = 0) -> dict:
    """`{query, subjects: [{key,label,count}], subject, items, count, total}`.

    With no ``subject`` this is the preview: a few rows from every subject, merged in
    subject order. With one, that subject is paged and the others contribute only
    their counts (for the sidebar badges).
    """
    query = (query or "").strip()
    subjects = available_subjects(flags)
    empty = {"query": query, "subjects": [], "subject": subject, "items": [],
             "count": 0, "total": 0}
    if len(query) < MIN_QUERY or not subjects:
        return empty
    if subject is not None and subject not in subjects:
        return empty

    # Everything is fetched at preview width; the selected subject additionally gets
    # its real page. That keeps this to one fan-out instead of a count pass plus a
    # rows pass.
    per = PREVIEW_PER_SUBJECT if subject is None else PREVIEW_PER_SUBJECT
    results = await asyncio.gather(*[
        _safe(s, query, flags, branch, per, 0) for s in subjects
    ])
    counts = {s: total for s, (_rows, total) in zip(subjects, results, strict=True)}

    if subject is None:
        items: list[dict] = []
        for s, (rows, _total) in zip(subjects, results, strict=True):
            for row in rows:
                items.append({**row, "subject": s})
        return {
            "query": query,
            "subjects": [{"key": s, "label": index.SUBJECT_LABELS[s], "count": counts[s]}
                         for s in subjects if counts[s]],
            "subject": None,
            "items": items,
            "count": len(items),
            "total": sum(counts.values()),
        }

    rows, total = await _safe(subject, query, flags, branch, limit, offset)
    counts[subject] = max(counts.get(subject, 0), total)
    return {
        "query": query,
        "subjects": [{"key": s, "label": index.SUBJECT_LABELS[s], "count": counts[s]}
                     for s in subjects],
        "subject": subject,
        "items": [{**row, "subject": subject} for row in rows],
        "count": len(rows),
        "total": total,
    }
