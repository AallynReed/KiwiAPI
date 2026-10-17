"""Visitor IP -> ISO country code, from DB-IP's free "IP to Country Lite" database
(CC BY 4.0, credited on /privacy).

Each web worker memory-maps the .mmdb from ``settings.geoip_dir`` and checks once a
minute whether it changed or is due for the monthly refresh. One worker downloads
(a lock file keeps the rest out) and the others pick the new file up by mtime.
Lookups stay in memory: neither the IP nor the country is stored or logged.
"""
from __future__ import annotations

import asyncio
import gzip
import logging
import os
import time
from datetime import date, timedelta
from pathlib import Path

import httpx
import maxminddb

from app.core.config import settings

logger = logging.getLogger("kiwi.geoip")

_URL = "https://download.db-ip.com/free/dbip-country-lite-{month}.mmdb.gz"
_FILE = "dbip-country-lite.mmdb"
_MAX_AGE = 32 * 86400
_RETRY_AFTER = 6 * 3600
_LOCK_STALE = 600
_CHECK_EVERY = 60

_state: dict = {"reader": None, "mtime": 0.0, "failed_at": 0.0, "task": None}


def _path() -> Path:
    return Path(settings.geoip_dir) / _FILE


def country(ip: str | None) -> str | None:
    """Country code for ``ip`` (e.g. "FR"); None if private, unknown or no database yet."""
    reader = _state["reader"]
    if reader is None or not ip:
        return None
    try:
        rec = reader.get(ip)
    except ValueError:
        return None
    if not isinstance(rec, dict):
        return None
    code = (rec.get("country") or {}).get("iso_code")
    return code if isinstance(code, str) else None


def ensure_running() -> None:
    """Start this worker's refresh loop (idempotent). Called lazily by the gate, so
    nothing is downloaded until a country list is actually configured."""
    task = _state["task"]
    if task is None or task.done():
        _state["task"] = asyncio.get_running_loop().create_task(_loop())


async def stop() -> None:
    task = _state["task"]
    if task is not None:
        task.cancel()
        try:
            await task
        except asyncio.CancelledError:
            pass


async def _loop() -> None:
    while True:
        try:
            await _refresh()
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.exception("GeoIP refresh failed")
        await asyncio.sleep(_CHECK_EVERY)


async def _refresh() -> None:
    path = _path()
    try:
        age = time.time() - path.stat().st_mtime
    except FileNotFoundError:
        age = float("inf")
    if age > _MAX_AGE and time.time() - _state["failed_at"] > _RETRY_AFTER:
        path.parent.mkdir(parents=True, exist_ok=True)
        lock = path.with_suffix(".lock")
        if _claim(lock):
            try:
                await _download(path)
            except Exception:
                _state["failed_at"] = time.time()
                logger.warning("GeoIP database download failed", exc_info=True)
            finally:
                lock.unlink(missing_ok=True)
    _load(path)


def _claim(lock: Path) -> bool:
    try:
        os.close(os.open(lock, os.O_CREAT | os.O_EXCL | os.O_WRONLY))
        return True
    except FileExistsError:
        try:
            if time.time() - lock.stat().st_mtime > _LOCK_STALE:
                lock.unlink(missing_ok=True)
        except OSError:
            pass
        return False


async def _download(dest: Path) -> None:
    first = date.today().replace(day=1)
    months = [first, first - timedelta(days=1)]
    async with httpx.AsyncClient(timeout=60, follow_redirects=True) as client:
        for month in months:
            resp = await client.get(_URL.format(month=month.strftime("%Y-%m")))
            if resp.status_code == 404:
                continue
            resp.raise_for_status()
            await asyncio.to_thread(_install, resp.content, dest)
            logger.info("GeoIP database updated (%s)", month.strftime("%Y-%m"))
            return
    raise RuntimeError("no DB-IP country database published for this or last month")


def _install(gz: bytes, dest: Path) -> None:
    tmp = dest.with_name(f"{dest.name}.{os.getpid()}.tmp")
    try:
        tmp.write_bytes(gzip.decompress(gz))
        maxminddb.open_database(str(tmp)).close()
        os.replace(tmp, dest)
    finally:
        tmp.unlink(missing_ok=True)


def _load(path: Path) -> None:
    try:
        mtime = path.stat().st_mtime
    except FileNotFoundError:
        return
    if mtime == _state["mtime"]:
        return
    old = _state["reader"]
    _state["reader"] = maxminddb.open_database(str(path))
    _state["mtime"] = mtime
    if old is not None:
        old.close()
