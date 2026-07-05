"""Mirror an external mod catalog into the Mods Hub as *stray* mods.

A stray mod is an imported, unclaimed mod: a ``ModProject`` with ``owner_id=None``,
``is_stray=True``, the original ``author`` name, the source it came from, and a single
mirrored release (the latest file). A site user can later *claim* it (admin-approved),
which hands it over and turns it into an ordinary mod.

The upstream catalog + its origin are an INTERNAL implementation detail and must never
surface in any user- or admin-visible place (UI, API field, or link) - see the stray
mods design note. The upstream API used here:
  - ``/api/mods-all`` -> the full catalog (per mod: id, name, author{ID,Username},
    description, image, type/subtype, totaldownloads, likes, downloads[]).
  - ``client/downloadfile.php?fileid=`` -> the raw ``.tmod``/``.zip`` bytes.

Per the chosen policy (see the modder/admin decisions):
  - **Bulk import** creates mods **approved** (visible) and **mirrors every file** into
    the shared CAS.
  - A later **resync** refreshes download counts + re-mirrors changed files on existing
    mods, and adds newly-discovered mods as **pending** (hidden) for admin approval.

The run is a throttled, resumable background job (idempotent by ``source_id``; a file is
only re-fetched when its Trovesaurus ``fileid`` changes), with progress in
``StrayImportState`` for the admin panel.
"""

from __future__ import annotations

import asyncio
import logging

import httpx

from app.core.utils import iso, utcnow
from app.trove import tmod
from app.trove.mods_hub import store
from app.trove.mods_hub.models import (
    ModImageAsset,
    ModProject,
    ModRelease,
    StrayImportState,
)
from app.trove.mods_hub.service import (
    STRAY_HANDLE,
    _clean_tags,
    _slugify,
    _unique_slug,
)

logger = logging.getLogger("kiwi.mods_hub.strayimport")

SOURCE = "trovesaurus"
CATALOG_URL = "https://trovesaurus.com/api/mods-all"
FILE_URL = "https://trovesaurus.com/client/downloadfile.php?fileid={fileid}"
MOD_PAGE_URL = "https://trovesaurus.com/mods/{id}"
USER_AGENT = "BetterTroveTools-API/1.0 (+https://trove.aallyn.net; mod mirror)"

_FILE_DELAY = 0.25                       # polite pause between file downloads (s)
_MAX_FILE_BYTES = 80 * 1024 * 1024       # skip absurd files
_PROGRESS_EVERY = 25                     # persist progress every N mods
_STALE_RUN_SECONDS = 3 * 3600            # a "running" flag older than this is stale (crash)

_task: asyncio.Task | None = None


# --- state -----------------------------------------------------------------

async def _state() -> StrayImportState:
    st = await StrayImportState.find_one(StrayImportState.key == SOURCE)
    if st is None:
        st = StrayImportState(key=SOURCE)
        await st.insert()
    return st


def state_dto(st: StrayImportState) -> dict:
    return {
        "running": st.running, "phase": st.phase, "total": st.total,
        "processed": st.processed, "imported": st.imported, "updated": st.updated,
        "pending_added": st.pending_added, "failed": st.failed,
        "last_error": st.last_error,
        "started_at": iso(st.started_at),
        "finished_at": iso(st.finished_at),
    }


async def get_state() -> dict:
    return state_dto(await _state())


# --- helpers ---------------------------------------------------------------

def _latest_download(downloads: list) -> dict | None:
    """The newest real file: highest ``fileid``, skipping ``extra`` (supplementary)
    files - matches how BTT picks the file to install."""
    valid = []
    for d in downloads or []:
        if not isinstance(d, dict):
            continue
        try:
            if int(d.get("extra", 0) or 0):
                continue
        except (TypeError, ValueError):
            pass
        valid.append(d)
    if not valid:
        return None
    valid.sort(key=lambda d: -int(d.get("fileid") or 0))
    return valid[0]


async def _download(client: httpx.AsyncClient, url: str) -> bytes | None:
    try:
        r = await client.get(url, follow_redirects=True)
        if r.status_code != 200:
            return None
        data = r.content
        if not data or len(data) > _MAX_FILE_BYTES:
            return None
        return data
    except Exception:
        logger.warning("trovesaurus: download failed: %s", url, exc_info=True)
        return None


async def _store_image(client: httpx.AsyncClient, url: str | None) -> str | None:
    """Mirror a mod's image into the CAS; returns its sha (or None)."""
    if not url:
        return None
    data = await _download(client, url)
    if data is None:
        return None
    sniffed = store.sniff_image(data)
    if sniffed is None:
        return None
    content_type, w, h = sniffed
    sha, _ = await store.put_blob(data)
    if await ModImageAsset.find_one(ModImageAsset.sha == sha) is None:
        await ModImageAsset(sha=sha, content_type=content_type, byte_size=len(data),
                            owner_id=None, width=w, height=h).insert()
    return sha


async def _fetch_release(client: httpx.AsyncClient, dl: dict) -> dict | None:
    """Download + parse one Trovesaurus file. Returns a parsed bundle (bytes +
    metadata) or None if the download failed. For a ``.tmod`` it reads the header's
    ``title`` (the required download filename) and ``author`` (the real author -
    Trovesaurus' listed author can differ / be stale)."""
    fileid = str(dl.get("fileid") or "")
    if not fileid:
        return None
    fmt = (dl.get("format") or "tmod").lower()
    if fmt not in ("tmod", "zip"):
        fmt = "tmod"
    data = await _download(client, FILE_URL.format(fileid=fileid))
    if data is None:
        return None
    props: dict = {}
    if fmt == "tmod":
        try:
            props = (tmod.read_tmod(data, metadata_only=True).get("properties") or {})
        except Exception:
            props = {}
    return {
        "fileid": fileid, "fmt": fmt, "data": data,
        "tag": str(dl.get("version") or fileid),
        "changelog": dl.get("changes") or "",
        "props": props,
        "title_prop": (props.get("title") or "").strip(),
        "tmod_author": (props.get("author") or "").strip() or None,
    }


def _resolve_author(parsed: dict | None, fallback: str) -> str:
    """The author to credit: the ``.tmod`` header's ``author`` (which may name
    several, comma-separated) when present; otherwise - notably for ``.zip`` mods
    that have no header - the Trovesaurus author name."""
    if parsed and parsed["fmt"] == "tmod" and parsed["tmod_author"]:
        return parsed["tmod_author"]
    return fallback


async def _write_release(proj: ModProject, parsed: dict) -> None:
    """Store a pre-fetched file into the project's single release (create or replace)."""
    fmt, data = parsed["fmt"], parsed["data"]
    sha, _ = await store.put_blob(data)
    # The .tmod download name MUST equal its internal title (Trove validates it).
    filename = (f"{parsed['title_prop']}.tmod" if fmt == "tmod" and parsed["title_prop"]
                else f"{_slugify(proj.title)}.{fmt}")
    rel = await ModRelease.find_one(ModRelease.project_id == proj.id)   # one release per stray mod
    if rel is None:
        await ModRelease(
            project_id=proj.id, owner_id=None, tag=parsed["tag"], branch="main", title="",
            changelog=parsed["changelog"], release_format=fmt, tmod_sha=sha, tmod_size=len(data),
            tmod_filename=filename, tmod_properties=parsed["props"],
            status="published", published_at=utcnow(),
        ).insert()
    else:
        rel.tag, rel.release_format, rel.tmod_sha = parsed["tag"], fmt, sha
        rel.tmod_size, rel.tmod_filename, rel.tmod_properties = len(data), filename, parsed["props"]
        if parsed["changelog"]:
            rel.changelog = parsed["changelog"]
        rel.updated_at = utcnow()
        await rel.save()


async def _upsert_mod(client: httpx.AsyncClient, mod: dict, *, resync: bool) -> str:
    """Create or refresh one stray mod. Returns an outcome label."""
    sid = str(mod.get("id") or "").strip()
    if not sid:
        return "failed"
    name = (mod.get("name") or "Unnamed mod").strip() or "Unnamed mod"
    author_data = mod.get("author") if isinstance(mod.get("author"), dict) else {}
    ts_author = (author_data.get("Username") or "Unknown").strip() or "Unknown"
    author_id = str(author_data.get("ID") or "")
    downloads = mod.get("downloads") or []
    latest = _latest_download(downloads)
    tags = [t for t in ((mod.get("type") or "").strip().lower(),
                        (mod.get("subtype") or "").strip().lower()) if t]
    total_dl = int(mod.get("totaldownloads") or 0)
    likes = int(mod.get("likes") or 0)

    existing = await ModProject.find_one(
        ModProject.source == SOURCE, ModProject.source_id == sid)

    if existing is not None:
        if existing.stray_status == "rejected" or not existing.is_stray:
            return "skipped"     # admin declined it, or it was already claimed
        existing.title = name
        existing.description = mod.get("description") or existing.description
        existing.tags = _clean_tags(tags)
        existing.download_count = total_dl
        existing.source_likes = likes
        new_file_id = str(latest.get("fileid")) if latest else None
        if latest and new_file_id and new_file_id != existing.source_file_id:
            parsed = await _fetch_release(client, latest)
            if parsed is not None:
                await _write_release(existing, parsed)
                existing.source_file_id = new_file_id
                # Author follows the new file: .tmod header, else Trovesaurus name.
                existing.author = _resolve_author(parsed, ts_author)
        existing.owner_username = existing.author or ts_author   # keep display in sync
        if existing.banner_sha is None:
            existing.banner_sha = await _store_image(client, mod.get("image"))
        existing.updated_at = utcnow()
        await existing.save()
        return "updated"

    # New mod - download + parse the file FIRST so we can credit the .tmod's author.
    if latest is None:
        return "skipped"         # nothing downloadable to mirror
    parsed = await _fetch_release(client, latest)
    if parsed is None:
        return "skipped"         # couldn't mirror the file; retry next run
    author = _resolve_author(parsed, ts_author)   # .tmod header author, else Trovesaurus
    slug = await _unique_slug(None, name)
    status = "pending" if resync else "approved"
    banner_sha = await _store_image(client, mod.get("image"))
    proj = ModProject(
        slug=slug, title=name, summary="", description=mod.get("description") or "",
        tags=_clean_tags(tags), owner_id=None, owner_username=author,
        owner_handle=STRAY_HANDLE, visibility=("public" if status == "approved" else "draft"),
        mode="releases", source_visibility="public", banner_sha=banner_sha,
        download_count=total_dl, is_stray=True, stray_status=status, author=author,
        source=SOURCE, source_id=sid, source_url=MOD_PAGE_URL.format(id=sid),
        source_author_id=author_id, source_likes=likes, source_file_id=parsed["fileid"],
    )
    await proj.insert()
    await _write_release(proj, parsed)
    return "pending" if status == "pending" else "imported"


# --- the job ---------------------------------------------------------------

async def run(resync: bool) -> None:
    st = await _state()
    st.running = True
    st.phase = "resyncing" if resync else "importing"
    st.total = st.processed = st.imported = st.updated = st.pending_added = st.failed = 0
    st.started_at = utcnow()
    st.finished_at = None
    st.last_error = None
    await st.save()
    try:
        timeout = httpx.Timeout(30.0, read=300.0)
        async with httpx.AsyncClient(timeout=timeout, headers={"User-Agent": USER_AGENT}) as client:
            r = await client.get(CATALOG_URL, follow_redirects=True)
            r.raise_for_status()
            data = r.json()
            mods = list(data.values()) if isinstance(data, dict) else data
            mods = [m for m in mods if isinstance(m, dict) and m.get("id")]
            st.total = len(mods)
            await st.save()
            for i, mod in enumerate(mods):
                try:
                    outcome = await _upsert_mod(client, mod, resync=resync)
                    if outcome == "imported":
                        st.imported += 1
                    elif outcome == "updated":
                        st.updated += 1
                    elif outcome == "pending":
                        st.pending_added += 1
                    elif outcome == "failed":
                        st.failed += 1
                except Exception:
                    st.failed += 1
                    logger.warning("trovesaurus: mod %s failed", mod.get("id"), exc_info=True)
                st.processed = i + 1
                if st.processed % _PROGRESS_EVERY == 0:
                    st.updated_at = utcnow()
                    await st.save()
        st.phase = "done"
    except Exception as e:
        st.phase = "error"
        st.last_error = str(e)[:500]
        logger.exception("trovesaurus import failed")
    finally:
        st.running = False
        st.finished_at = utcnow()
        st.updated_at = utcnow()
        await st.save()


async def start(resync: bool, *, force: bool = False) -> dict:
    """Kick off an import/resync as a background task. Refuses if one is already
    running (unless ``force``, or the running flag is stale from a crash)."""
    global _task
    st = await _state()
    if st.running and not force:
        age = (utcnow() - st.started_at).total_seconds() if st.started_at else 1e9
        if age < _STALE_RUN_SECONDS:
            return {"started": False, "reason": "An import is already running.", **state_dto(st)}
    _task = asyncio.create_task(run(resync))
    return {"started": True, "resync": resync}
