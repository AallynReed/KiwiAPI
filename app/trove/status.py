"""Trove server status prober — EU / US / PTS, with persisted history.

Every ``trove_status_probe_interval_seconds`` a background loop probes:

  • **auth** (shared) — HTTPS liveness of ``auth.trionworlds.com``, the
    account-auth gateway. Structured HTTP response (< 500) + valid TLS =
    reachable; timeout / refused / TLS error / 5xx = down. Catches full
    outages but stays up during world-only maintenance (Akamai-fronted).
  • **game per environment** (eu / us / pts) — TCP-connect probe of the
    glsserver port (6560) on each environment's game host. Accepted =
    that environment's worlds are playable; refused/timeout while auth is
    up = that environment is in maintenance.

Per-environment verdict:
  • ``down``        — auth gateway unreachable (can't even log in)
  • ``maintenance`` — auth up, game socket refused
  • ``online``      — auth up + game socket accepted
  • ``unknown``     — no probe completed yet

Each environment's status timeline is persisted as ``TroveStatusEvent``
segments (a new open segment opens on every status change, the prior one
closes). The /status page reads these to draw uptime/downtime history.

The current snapshot is cached in-process; the read endpoints serve the
cache and never block on a live probe.
"""
from __future__ import annotations

import asyncio
import logging
import time

import httpx

from app.core.config import settings

logger = logging.getLogger("kiwi.trove.status")

# Environments we probe. Each maps to a (host, port) pulled from
# runtime_config at probe time so endpoints can be retargeted live.
# "eu"/"us" are the two public Live regions; "pts" is the test server.
_ENVIRONMENTS = ("eu", "us", "pts")

# Cached snapshot; None until the first probe completes.
_state: dict | None = None
_task: asyncio.Task | None = None


async def _probe_auth() -> dict:
    """HTTPS liveness of the shared account-auth gateway."""
    url = settings.trove_status_auth_url
    timeout = settings.trove_status_timeout_seconds
    t0 = time.monotonic()
    try:
        async with httpx.AsyncClient(timeout=timeout, follow_redirects=False) as client:
            resp = await client.get(url, headers={"User-Agent": "KiwiAPI-status/1.0"})
        latency = (time.monotonic() - t0) * 1000
        online = resp.status_code < 500
        return {
            "online": online,
            "http_status": resp.status_code,
            "latency_ms": round(latency, 1),
            "error": None if online else f"HTTP {resp.status_code}",
        }
    except Exception as e:  # noqa: BLE001 — any failure is "down"
        latency = (time.monotonic() - t0) * 1000
        return {
            "online": False,
            "http_status": None,
            "latency_ms": round(latency, 1),
            "error": type(e).__name__,
        }


async def _probe_game(host: str, port: int) -> dict:
    """TCP-connect liveness of one environment's game glsserver."""
    timeout = settings.trove_status_timeout_seconds
    t0 = time.monotonic()
    writer = None
    if not host or port <= 0:
        return {"online": False, "host": host, "port": port,
                "latency_ms": 0.0, "error": "not_configured"}
    try:
        _reader, writer = await asyncio.wait_for(
            asyncio.open_connection(host, port), timeout=timeout,
        )
        latency = (time.monotonic() - t0) * 1000
        return {"online": True, "host": host, "port": port,
                "latency_ms": round(latency, 1), "error": None}
    except Exception as e:  # noqa: BLE001
        latency = (time.monotonic() - t0) * 1000
        return {"online": False, "host": host, "port": port,
                "latency_ms": round(latency, 1), "error": type(e).__name__}
    finally:
        if writer is not None:
            try:
                writer.close()
                await writer.wait_closed()
            except Exception:
                pass


def _verdict(auth_online: bool, game_online: bool) -> str:
    if not auth_online:
        return "down"
    return "online" if game_online else "maintenance"


# Public Live regions that roll up into the homepage pill's "overall"
# status. PTS is excluded — it's a test server and shouldn't colour the
# public indicator.
_LIVE_REGIONS = ("eu", "us")


def _overall(environments: dict) -> str:
    """Roll the Live regions into one headline status for the homepage
    pill: all regions online → ``online``; every region fully down →
    ``down``; anything mixed/partial (e.g. EU in maintenance while US is
    up) → ``maintenance``. The per-region cards on /status carry the
    detail."""
    statuses = [
        environments[e]["status"] for e in _LIVE_REGIONS if e in environments
    ]
    if not statuses:
        return "unknown"
    if all(s == "online" for s in statuses):
        return "online"
    if all(s == "down" for s in statuses):
        return "down"
    return "maintenance"


async def _env_endpoints(env: str) -> tuple[str, int]:
    """(host, port) for an environment, from runtime_config (live-tunable)."""
    from app.admin import runtime_config
    host = str(await runtime_config.get_setting(f"trove_status_{env}_host")).strip()
    port = int(await runtime_config.get_setting(f"trove_status_{env}_port"))
    return host, port


async def probe_once() -> dict:
    """Run auth + per-env game probes, persist any status transitions,
    update the in-process cache, and return the snapshot."""
    auth = await _probe_auth()

    environments: dict[str, dict] = {}
    for env in _ENVIRONMENTS:
        host, port = await _env_endpoints(env)
        game = await _probe_game(host, port)
        status = _verdict(auth["online"], game["online"])
        environments[env] = {
            "status": status,
            "online": game["online"],
            "game": game,
        }
        await _record_transition(env, status, game["online"])

    snapshot = {
        # Top-level overall = rollup of the public Live regions (eu+us),
        # which the homepage pill reads. PTS never drives it.
        "overall": _overall(environments),
        "auth": auth,
        "environments": environments,
        "checked_at": int(time.time()),
    }
    global _state
    _state = snapshot
    return snapshot


async def _record_transition(env: str, status: str, online: bool) -> None:
    """Open a new status segment when ``env`` changes status, closing the
    previous open one. No-op when the status is unchanged. Best-effort —
    persistence failure must not break probing."""
    try:
        from app.trove.models import TroveStatusEvent
        now = int(time.time())
        last = await (
            TroveStatusEvent.find(TroveStatusEvent.env == env)
            .sort("-started_at")
            .first_or_none()
        )
        if last is not None and last.ended_at is None and last.status == status:
            return  # unchanged — keep the open segment running
        if last is not None and last.ended_at is None:
            # Close the previous open segment.
            last.ended_at = now
            await last.save()
        await TroveStatusEvent(
            env=env, status=status, online=online, started_at=now, ended_at=None,
        ).insert()
        logger.info("trove status: %s -> %s", env, status)
    except Exception:
        logger.warning("trove status: failed to record %s transition", env, exc_info=True)


def get_status() -> dict:
    """Latest cached snapshot, or an 'unknown' shell before the first probe."""
    if _state is None:
        unknown_env = {"status": "unknown", "online": False, "game": None}
        return {
            "overall": "unknown",
            "auth": None,
            "environments": {env: dict(unknown_env) for env in _ENVIRONMENTS},
            "checked_at": None,
        }
    return _state


async def get_history(env: str, days: int) -> dict:
    """Status-timeline segments for one environment over the last ``days``
    days, plus a computed uptime fraction. Segments are clamped to the
    window; the open (current) segment is reported with ``ended_at=null``."""
    from app.trove.models import TroveStatusEvent

    env = env if env in _ENVIRONMENTS else "eu"
    days = max(1, min(days, 90))
    now = int(time.time())
    window_start = now - days * 86400

    # Pull every segment that overlaps the window: started before window end
    # AND (still open OR ended after window start).
    rows = await (
        TroveStatusEvent.find(TroveStatusEvent.env == env)
        .sort("+started_at")
        .to_list()
    )
    segments = []
    up_seconds = 0
    covered_seconds = 0
    for r in rows:
        seg_start = r.started_at
        seg_end = r.ended_at if r.ended_at is not None else now
        if seg_end <= window_start or seg_start >= now:
            continue  # outside the window
        clamped_start = max(seg_start, window_start)
        clamped_end = min(seg_end, now)
        dur = max(0, clamped_end - clamped_start)
        covered_seconds += dur
        if r.status == "online":
            up_seconds += dur
        segments.append({
            "status": r.status,
            "online": r.online,
            "started_at": clamped_start,
            "ended_at": None if r.ended_at is None else clamped_end,
            "duration_seconds": dur,
        })

    uptime = (up_seconds / covered_seconds) if covered_seconds > 0 else None
    # Outage list (down/maintenance segments) for a readable incident log.
    outages = [
        {"status": s["status"], "started_at": s["started_at"],
         "ended_at": s["ended_at"], "duration_seconds": s["duration_seconds"]}
        for s in segments if s["status"] != "online"
    ]
    return {
        "env": env,
        "days": days,
        "window_start": window_start,
        "window_end": now,
        "uptime": None if uptime is None else round(uptime, 5),
        "covered_seconds": covered_seconds,
        "segments": segments,
        "outages": outages,
    }


async def _loop() -> None:
    await asyncio.sleep(3.0)  # let init_db / redis settle
    while True:
        try:
            r = await probe_once()
            logger.info(
                "trove status: eu=%s us=%s pts=%s (auth=%s)",
                r["environments"]["eu"]["status"],
                r["environments"]["us"]["status"],
                r["environments"]["pts"]["status"],
                r["auth"]["online"],
            )
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("trove status probe failed", exc_info=True)
        try:
            await asyncio.sleep(settings.trove_status_probe_interval_seconds)
        except asyncio.CancelledError:
            raise


def start_status_prober() -> None:
    global _task
    if _task is None:
        _task = asyncio.create_task(_loop())


async def stop_status_prober() -> None:
    global _task
    if _task is not None:
        _task.cancel()
        try:
            await _task
        except asyncio.CancelledError:
            pass
        _task = None
