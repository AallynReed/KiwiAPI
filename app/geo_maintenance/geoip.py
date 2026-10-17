"""Visitor IP lookups from DB-IP's free "Lite" databases (CC BY 4.0, credited on
/privacy): the country, for the regional maintenance gate, and the network (ASN),
so cloud-hosted bots stay out of the page-view stats.

Each web worker memory-maps the .mmdb files from ``settings.geoip_dir`` and checks
once a minute whether one changed or is due for the monthly refresh. One worker
downloads (a lock file keeps the rest out) and the others pick the new file up by
mtime. A database is only fetched once something asks for it. Lookups stay in
memory: neither the IP nor what it maps to is stored or logged.
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

_URL = "https://download.db-ip.com/free/dbip-{kind}-lite-{month}.mmdb.gz"
_MAX_AGE = 32 * 86400
_RETRY_AFTER = 6 * 3600
_LOCK_STALE = 600
_CHECK_EVERY = 60


class _Database:
    """One DB-IP Lite database, kept fresh by a per-worker refresh loop."""

    def __init__(self, kind: str) -> None:
        self.kind = kind
        self.state: dict = {"reader": None, "mtime": 0.0, "failed_at": 0.0, "task": None}

    def path(self) -> Path:
        return Path(settings.geoip_dir) / f"dbip-{self.kind}-lite.mmdb"

    def get(self, ip: str | None) -> dict | None:
        reader = self.state["reader"]
        if reader is None or not ip:
            return None
        try:
            rec = reader.get(ip)
        except ValueError:
            return None
        return rec if isinstance(rec, dict) else None

    def ensure_running(self) -> None:
        task = self.state["task"]
        if task is None or task.done():
            self.state["task"] = asyncio.get_running_loop().create_task(self._loop())

    async def stop(self) -> None:
        task = self.state["task"]
        if task is not None:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass

    async def _loop(self) -> None:
        while True:
            try:
                await self._refresh()
            except asyncio.CancelledError:
                raise
            except Exception:
                logger.exception("GeoIP %s refresh failed", self.kind)
            await asyncio.sleep(_CHECK_EVERY)

    async def _refresh(self) -> None:
        path = self.path()
        try:
            age = time.time() - path.stat().st_mtime
        except FileNotFoundError:
            age = float("inf")
        if age > _MAX_AGE and time.time() - self.state["failed_at"] > _RETRY_AFTER:
            path.parent.mkdir(parents=True, exist_ok=True)
            lock = path.with_suffix(".lock")
            if _claim(lock):
                try:
                    await self._download(path)
                except Exception:
                    self.state["failed_at"] = time.time()
                    logger.warning("GeoIP %s database download failed", self.kind, exc_info=True)
                finally:
                    lock.unlink(missing_ok=True)
        self._load(path)

    async def _download(self, dest: Path) -> None:
        first = date.today().replace(day=1)
        months = [first, first - timedelta(days=1)]
        async with httpx.AsyncClient(timeout=60, follow_redirects=True) as client:
            for month in months:
                resp = await client.get(_URL.format(kind=self.kind, month=month.strftime("%Y-%m")))
                if resp.status_code == 404:
                    continue
                resp.raise_for_status()
                await asyncio.to_thread(_install, resp.content, dest)
                logger.info("GeoIP %s database updated (%s)", self.kind, month.strftime("%Y-%m"))
                return
        raise RuntimeError(f"no DB-IP {self.kind} database published for this or last month")

    def _load(self, path: Path) -> None:
        try:
            mtime = path.stat().st_mtime
        except FileNotFoundError:
            return
        if mtime == self.state["mtime"]:
            return
        old = self.state["reader"]
        self.state["reader"] = maxminddb.open_database(str(path))
        self.state["mtime"] = mtime
        if old is not None:
            old.close()


_COUNTRY = _Database("country")
_ASN = _Database("asn")
_state = _COUNTRY.state


def country(ip: str | None) -> str | None:
    """Country code for ``ip`` (e.g. "FR"); None if private, unknown or no database yet."""
    code = ((_COUNTRY.get(ip) or {}).get("country") or {}).get("iso_code")
    return code if isinstance(code, str) else None


def asn(ip: str | None) -> int | None:
    """The autonomous system (network) ``ip`` belongs to; None if unknown or no database yet."""
    n = (_ASN.get(ip) or {}).get("autonomous_system_number")
    return n if isinstance(n, int) else None


def ensure_running() -> None:
    """Start this worker's country refresh loop (idempotent). Called lazily by the
    gate, so nothing is downloaded until a country list is actually configured."""
    _COUNTRY.ensure_running()


def ensure_asn() -> None:
    """Start this worker's ASN refresh loop (idempotent)."""
    _ASN.ensure_running()


async def stop() -> None:
    await _COUNTRY.stop()
    await _ASN.stop()


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


def _install(gz: bytes, dest: Path) -> None:
    tmp = dest.with_name(f"{dest.name}.{os.getpid()}.tmp")
    try:
        tmp.write_bytes(gzip.decompress(gz))
        maxminddb.open_database(str(tmp)).close()
        os.replace(tmp, dest)
    finally:
        tmp.unlink(missing_ok=True)
