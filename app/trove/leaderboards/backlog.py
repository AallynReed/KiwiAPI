"""Server-side ingest backlog for leaderboards.

Every dump the API receives is gzip-saved to disk keyed by its anchor
(``<anchor>.cfg.gz``), so the entire history can be RE-INGESTED later from the
admin panel with **no browser upload** - the server reads from disk and paces
itself (one dump at a time), which sidesteps the upload + memory pile-up that
crashed the box during mass uploads. Files dropped into the folder by hand
(``<unix>.cfg`` or ``.cfg.gz``) are picked up the same way.

Layout: ``{backlog_dir}/leaderboards/<anchor>.cfg.gz``.

Re-ingest does PURE inserts (``allow_backfill=True``, no per-file warm / archive
move) and runs the heavy compute ONCE at the end. Progress is published to Redis
so the admin panel can poll it live (falls back to an in-process copy without
Redis). A Redis single-flight guard stops two re-ingests overlapping.
"""
import asyncio
import gzip
import json
import logging
import os
import re
import time
from pathlib import Path

from app.core.config import settings
from app.core.redis import get_redis

logger = logging.getLogger("kiwi.trove.leaderboards.backlog")

_TYPE = "leaderboards"
# Plausible unix-seconds anchor embedded in a filename (2020-01-01 .. 2035-01-01).
_ANCHOR_RE = re.compile(r"(\d{9,11})")
_STATUS_KEY = "lb:reingest:status"
_RUNNING_KEY = "lb:reingest:running"

# In-process progress fallback when Redis is absent (single-worker dev).
_local_status: dict = {"running": False}


def _dir() -> Path:
    return Path(settings.backlog_dir) / _TYPE


def _anchor_of(name: str) -> int | None:
    m = _ANCHOR_RE.search(name)
    if not m:
        return None
    n = int(m.group(1))
    return n if 1577836800 <= n <= 2051222400 else None


# ── save (called from the ingest endpoint) ───────────────────────────────────

def _write_gz(path: Path, data: bytes) -> None:
    with gzip.open(path, "wb", compresslevel=6) as f:
        f.write(data)


def _prune(d: Path, days: int) -> None:
    cutoff = time.time() - days * 86400
    for p in d.iterdir():
        try:
            if p.is_file() and p.stat().st_mtime < cutoff:
                p.unlink()
        except OSError:
            pass


async def save(anchor: int, text: str) -> None:
    """Gzip-write the raw dump to ``<anchor>.cfg.gz`` (atomic via temp+rename).
    Best-effort - a disk failure must never break the ingest itself."""
    if not settings.backlog_enabled or not anchor:
        return
    try:
        d = _dir()
        await asyncio.to_thread(d.mkdir, parents=True, exist_ok=True)
        final = d / f"{anchor}.cfg.gz"
        tmp = d / f".{anchor}.cfg.gz.tmp"
        await asyncio.to_thread(_write_gz, tmp, text.encode("utf-8"))
        await asyncio.to_thread(os.replace, tmp, final)
        if settings.backlog_retention_days > 0:
            await asyncio.to_thread(_prune, d, settings.backlog_retention_days)
    except Exception:
        logger.warning("backlog save failed for anchor=%s", anchor, exc_info=True)


# ── list / read ──────────────────────────────────────────────────────────────

def list_files() -> list[tuple[int, Path]]:
    """``(anchor, path)`` for every backlog file with a parseable anchor, oldest
    first. Tolerates both the auto-saved ``.cfg.gz`` and hand-dropped ``.cfg``."""
    d = _dir()
    if not d.is_dir():
        return []
    out: list[tuple[int, Path]] = []
    for p in d.iterdir():
        if not p.is_file() or p.name.startswith("."):
            continue
        a = _anchor_of(p.name)
        if a is not None:
            out.append((a, p))
    out.sort(key=lambda t: t[0])
    return out


def info() -> dict:
    """Where the server is scanning + how many files it sees - surfaced to the
    admin panel so a wrong path / missing bind-mount is obvious instead of a
    silent empty backlog."""
    d = _dir()
    return {
        "backlog_dir": str(d),
        "backlog_dir_exists": d.is_dir(),
        "backlog_files": len(list_files()),
    }


def _read_text(path: Path) -> str:
    if path.name.endswith(".gz"):
        with gzip.open(path, "rt", encoding="utf-8", errors="replace") as f:
            return f.read()
    return path.read_text(encoding="utf-8", errors="replace")


# ── progress status ──────────────────────────────────────────────────────────

async def _set_status(status: dict) -> None:
    global _local_status
    _local_status = status
    r = get_redis()
    if r is not None:
        try:
            await r.set(_STATUS_KEY, json.dumps(status), ex=86400)
        except Exception:
            pass


async def get_status() -> dict:
    """Latest re-ingest progress (shared via Redis across workers)."""
    r = get_redis()
    if r is not None:
        try:
            raw = await r.get(_STATUS_KEY)
            if raw:
                return json.loads(raw)
        except Exception:
            pass
    return dict(_local_status)


# ── re-ingest runner ─────────────────────────────────────────────────────────

async def reingest(*, clear_first: bool = False) -> None:
    """Background: ingest every backlog file oldest-first as a PURE insert (no
    per-file warm / archive move), then run the deferred compute ONCE. Single-
    flight guarded; publishes live progress."""
    from app.trove.leaderboards import service as lb_service
    from app.trove.leaderboards import detection, activity, class_activity

    r = get_redis()
    if r is not None:
        # Single-flight: only one re-ingest at a time across all workers. Short
        # TTL refreshed each file (heartbeat) so a crashed run self-clears in
        # ~15 min instead of blocking a re-trigger for hours.
        got = await r.set(_RUNNING_KEY, "1", nx=True, ex=900)
        if not got:
            logger.info("backlog re-ingest already running - skipping duplicate")
            return

    files = list_files()
    status = {
        "running": True, "total": len(files), "done": 0, "ok": 0, "failed": 0,
        "started_at": int(time.time()), "finished_at": None,
        "last_anchor": None, "clear_first": clear_first, "errors": [],
    }
    await _set_status(status)
    try:
        if clear_first:
            await lb_service.reset_all()
        for anchor, path in files:
            try:
                text = await asyncio.to_thread(_read_text, path)
                summary = await lb_service.insert_dump(
                    text, timestamp=anchor, allow_backfill=True,
                )
                status["ok"] += 1
                status["last_anchor"] = summary.get("created_at", anchor)
            except Exception as exc:  # noqa: BLE001 - one bad file mustn't stop the run
                status["failed"] += 1
                if len(status["errors"]) < 25:
                    status["errors"].append({"anchor": anchor, "error": str(exc)[:200]})
                logger.warning("backlog re-ingest failed for %s", path, exc_info=True)
            status["done"] += 1
            await _set_status(status)
            if r is not None:
                try:
                    await r.expire(_RUNNING_KEY, 900)   # heartbeat the single-flight lock
                except Exception:
                    pass

        # Deferred compute, ONCE, now that every file has landed.
        status["phase"] = "recomputing"
        await _set_status(status)
        try:
            await activity.backfill_history_chunked(total_days=730, force=True)
        except Exception:
            logger.warning("backlog re-ingest: activity backfill failed", exc_info=True)
        try:
            await class_activity.backfill_class_history_chunked(total_days=730, force=True)
        except Exception:
            logger.warning("backlog re-ingest: class activity backfill failed", exc_info=True)
        detection.trigger_warmer()
    finally:
        status["running"] = False
        status["phase"] = "done"
        status["finished_at"] = int(time.time())
        await _set_status(status)
        if r is not None:
            try:
                await r.delete(_RUNNING_KEY)
            except Exception:
                pass
        logger.info("backlog re-ingest done: %d ok, %d failed of %d",
                    status["ok"], status["failed"], status["total"])
