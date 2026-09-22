"""Rebuild the decoded game data after each patch, and report how it went.

``ensure_built`` is the archiver's post-sync hook. A decoder runs when the patch
it last tried differs from the current one, or its code changed (a deploy that
fixes it reruns it by itself). A decoder that raises, decodes nothing, or shrinks
its output sharply keeps the previous file, and the failure lands in the dev
panel's Game data tab with its traceback.
"""
from __future__ import annotations

import asyncio
import logging
import secrets
import traceback
from datetime import UTC, datetime, timedelta
from typing import Any

from app.core.config import settings
from app.core.redis import get_redis
from app.core.utils import utcnow
from app.trove.decode import store
from app.trove.decode.models import DecoderRun
from app.trove.decode.registry import DECODERS, code_version, dump
from app.trove.decode.tree import ArchiveTree
from app.trove.updates.cas import ContentStore

logger = logging.getLogger("kiwi.trove.decode")

_LOCK_KEY = "kiwi:decode:running"
_LOCK_TTL = 1800
# A rebuild with fewer than this share of the previous entries is held back: a
# format change that makes a decoder silently match less must not blank a page.
SHRINK_LIMIT = 0.8


def _aware(dt: datetime | None) -> datetime | None:
    return dt.replace(tzinfo=UTC) if dt is not None and dt.tzinfo is None else dt


async def _patch(branch: str) -> tuple[int, str | None]:
    from app.trove.updates.models import UpdateBranch
    doc = await UpdateBranch.find_one(UpdateBranch.branch == branch)
    return (doc.current_ordinal, doc.current_version) if doc else (0, None)


async def _latest(name: str, *, ok: bool | None = None) -> DecoderRun | None:
    query: dict[str, Any] = {"name": name}
    if ok is not None:
        query["ok"] = ok
    return await DecoderRun.find(query).sort("-started_at").first_or_none()


def _interrupted(run: DecoderRun) -> bool:
    started = _aware(run.started_at)
    return run.finished_at is None and started is not None and \
        utcnow() - started > timedelta(seconds=_LOCK_TTL)


async def _needs_run(name: str, ordinal: int) -> bool:
    last = await _latest(name)
    if last is None or _interrupted(last):
        return True
    return last.ordinal != ordinal or last.code_version != code_version(DECODERS[name])


def enabled() -> bool:
    return store.runtime_dir() is not None


async def ensure_built(branch: str) -> None:
    """Post-sync hook: rebuild whatever is out of date for the live branch."""
    if branch != settings.trove_render_branch or not enabled():
        return
    ordinal, _ = await _patch(branch)
    if not ordinal:
        return
    todo = [n for n in DECODERS if await _needs_run(n, ordinal)]
    if todo:
        await run(todo, trigger="patch")


async def is_running() -> bool:
    redis: Any = get_redis()
    return bool(redis and await redis.exists(_LOCK_KEY))


async def run(names: list[str], *, trigger: str, allow_shrink: bool = False) -> bool:
    """Run ``names`` one after another. False if a run is already in progress."""
    redis: Any = get_redis()
    token = secrets.token_hex(16)
    if redis is not None and not await redis.set(_LOCK_KEY, token, nx=True, ex=_LOCK_TTL):
        return False
    try:
        branch = settings.trove_render_branch
        ordinal, tag = await _patch(branch)
        prefixes = sorted({p for n in names for p in DECODERS[n].PREFIXES})
        tree = await ArchiveTree.load(branch, prefixes, ContentStore(settings.trove_update_store_dir))
        for name in names:
            await _run_one(name, tree, ordinal, tag, trigger, allow_shrink)
    finally:
        if redis is not None:
            try:
                if await redis.get(_LOCK_KEY) == token:
                    await redis.delete(_LOCK_KEY)
            except Exception:
                pass
    return True


async def _previous_count(name: str) -> int | None:
    """Entries in the file being replaced: the last good rebuild, else the repo copy."""
    last = await _latest(name, ok=True)
    if last is not None and last.entries is not None:
        return last.entries
    module = DECODERS[name]
    try:
        return module.count(store.baseline(module.OUTPUT))
    except (OSError, ValueError, KeyError, TypeError):
        return None


async def _run_one(name: str, tree: ArchiveTree, ordinal: int, tag: str | None,
                   trigger: str, allow_shrink: bool) -> None:
    module = DECODERS[name]
    rec = DecoderRun(name=name, trigger=trigger, ordinal=ordinal, version_tag=tag,
                     code_version=code_version(module), started_at=utcnow())
    await rec.insert()
    try:
        data = await asyncio.to_thread(module.build, tree)
        n = module.count(data)
        if not n:
            raise ValueError("decoded nothing - the game files may have moved or changed format")
        prev = await _previous_count(name)
        if prev and n < prev * SHRINK_LIMIT and not allow_shrink:
            raise ValueError(
                f"output shrank from {prev} to {n} entries; kept the previous data. "
                "If the game really removed them, run it again with \"Accept smaller output\".")
        target = store.runtime_dir() / module.OUTPUT  # type: ignore[operator]
        await asyncio.to_thread(store.write, target, dump(module, data))
        rec.ok, rec.entries = True, n
        logger.info("decode[%s]: wrote %d entries from ordinal %s", name, n, ordinal)
    except asyncio.CancelledError:
        rec.ok, rec.error = False, "cancelled (the server shut down mid-run)"
        raise
    except Exception as exc:
        rec.ok = False
        rec.error = f"{type(exc).__name__}: {exc}"
        rec.traceback = traceback.format_exc()[-8000:]
        logger.exception("decode[%s] failed", name)
    finally:
        rec.finished_at = utcnow()
        await rec.save()


# ── dev panel ────────────────────────────────────────────────────────────────

def _run_dto(r: DecoderRun | None) -> dict | None:
    if r is None:
        return None
    started, finished = _aware(r.started_at), _aware(r.finished_at)
    return {
        "trigger": r.trigger, "ordinal": r.ordinal, "version_tag": r.version_tag,
        "code_version": r.code_version, "ok": r.ok, "entries": r.entries,
        "error": r.error, "traceback": r.traceback,
        "started_at": started.isoformat() if started else None,
        "finished_at": finished.isoformat() if finished else None,
        "duration_ms": int((finished - started).total_seconds() * 1000) if started and finished else None,
    }


def _state(last: DecoderRun | None) -> str:
    if last is None:
        return "never"
    if last.finished_at is None:
        return "interrupted" if _interrupted(last) else "running"
    return "ok" if last.ok else "failed"


async def status() -> dict:
    ordinal, tag = await _patch(settings.trove_render_branch)
    rows = []
    for name, module in DECODERS.items():
        last = await _latest(name)
        good = last if last is not None and last.ok else await _latest(name, ok=True)
        rows.append({
            "name": name, "title": module.TITLE, "output": module.OUTPUT,
            "state": _state(last), "code_version": code_version(module),
            "serving": "rebuild" if store.path(module.OUTPUT) != store.BASELINE_DIR / module.OUTPUT
            else "repo copy",
            "last": _run_dto(last), "last_ok": _run_dto(good),
        })
    return {
        "enabled": enabled(), "running": await is_running(),
        "branch": settings.trove_render_branch, "ordinal": ordinal, "version_tag": tag,
        "decoders": rows,
    }


async def failing_count() -> int:
    """Decoders whose most recent run failed (the sidebar badge)."""
    total = 0
    for name in DECODERS:
        last = await _latest(name)
        if last is not None and (last.ok is False or _interrupted(last)):
            total += 1
    return total
