"""Trove server status prober - EU / US / PTS, with persisted history.

Every ``trove_status_probe_interval_seconds`` a background loop probes:

  • **auth** (shared) - HTTPS liveness of ``auth.trionworlds.com``, the
    account-auth gateway. Structured HTTP response (< 500) + valid TLS =
    reachable; timeout / refused / TLS error / 5xx = down. Catches full
    outages but stays up during world-only maintenance (Akamai-fronted).
  • **game per environment** (eu / us / pts) - probe of the glsserver port
    (6560) on each environment's game host. A bare TCP connect is NOT enough:
    a region in maintenance still completes the TCP handshake (and even answers
    the glsserver hello) before dropping the connection, so connect-only reads
    as a false "online" - seen directly in EU's maintenance capture (every
    attempt got SYN/ACK, the server echoed its fixed hello, emitted empty frames
    and sent FIN, and the client just retried). So the deep probe sends a
    well-formed glsserver opener and counts the region online only if the server
    HOLDS the connection open (a playable session socket); a server that drops
    right after the opener is maintenance. Refused/timeout while auth is up is
    also maintenance. Any anomaly in the deep probe falls back to the connect-only
    verdict, so it never does worse than the old check.

    The opener is a FRESH RANDOM ephemeral key each probe (see ``_random_opener``):
    Frida on the live client proved the real opener's 32-byte body is regenerated
    per connection, and a live glsserver holds the socket for any well-formed
    opener - so a random one behaves like a real client and never goes stale on a
    Trove protocol update. (The legacy mode, replaying a captured hello, is kept
    behind ``trove_status_game_random_opener=False`` but goes stale: the server
    starts rejecting an old captured opener as old-protocol, which once made a live
    EU read as down.)

Per-environment verdict (binary - the probe can't tell planned maintenance from
an outage, so an unreachable server is simply "down", shown red):
  • ``online``  - auth gateway reachable AND game socket alive
  • ``down``    - anything else (login gateway unreachable, or game socket
                  refused/dropped)
  • ``unknown`` - no probe completed yet

Each environment's status timeline is persisted as ``TroveStatusEvent``
segments (a new open segment opens on every status change, the prior one
closes). The /status page reads these to draw uptime/downtime history.

The current snapshot is cached in-process; the read endpoints serve the
cache and never block on a live probe.
"""
from __future__ import annotations

import asyncio
import json
import logging
import os
import time

import httpx

from app.core.config import settings
from app.core.redis import get_redis

logger = logging.getLogger("kiwi.trove.status")

# Environments we probe. Each maps to a (host, port) pulled from
# runtime_config at probe time so endpoints can be retargeted live.
# "eu"/"us" are the two public Live regions; "pts" is the test server.
_ENVIRONMENTS = ("eu", "us", "pts")

# Cached snapshot; None until the first probe completes.
_state: dict | None = None
_task: asyncio.Task | None = None

# Cross-process sharing: the prober runs only in the API process, so it mirrors each
# snapshot to Redis. The bot process (no prober) reads this for the live board + the
# status announcement via get_status_shared(). TTL a few probe intervals so a dead
# prober's snapshot eventually expires to "unknown" rather than going stale forever.
_REDIS_KEY = "kiwi:status:current"
_REDIS_TTL = 600


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
    except Exception as e:  # noqa: BLE001 - any failure is "down"
        latency = (time.monotonic() - t0) * 1000
        return {
            "online": False,
            "http_status": None,
            "latency_ms": round(latency, 1),
            "error": type(e).__name__,
        }


# Stop reading once the server has clearly engaged with a real session's worth
# of data - it's online, no need to keep draining.
_GLS_MAX_READ_BYTES = 32768
# A clean close is the maintenance signal ONLY if the server sent ~nothing real
# first (EU sent 39B: a 21B handshake + empty frames). If a server returns a
# substantial reply and *then* closes, treat it as engaged/online - don't read a
# data-bearing directory response as maintenance.
_GLS_SUBSTANTIVE_BYTES = 256

# glsserver opener = a 0x20-length-prefixed frame: 5-byte header + 32-byte body.
_GLS_OPENER_HEADER = b"\x20\x00\x00\x00\x00"


def _random_opener() -> bytes:
    """A fresh, well-formed glsserver opener: the frame header + 32 random bytes.

    Frida on the live client proved the real opener's 32-byte body is a
    per-connection RANDOM ephemeral key (a fresh X25519 pubkey every connection) -
    there is no fixed hello to capture. A live glsserver HOLDS the socket for ANY
    well-formed opener (verified live, EU+US), while a maintenance gateway drops it.
    So sending our own random opener is a liveness probe that behaves like a real
    client and NEVER goes stale on a protocol update - unlike replaying a captured
    hello, which the server eventually rejects as old-protocol (the bug that made a
    live EU read as down)."""
    return _GLS_OPENER_HEADER + os.urandom(32)


async def _glsserver_holds_session(reader, writer, hello: bytes, hold_seconds: float) -> bool | None:
    """Send the glsserver ``hello`` and judge whether the server HOLDS the
    connection (online) or drops it right after sending ~nothing (maintenance).

    Returns ``True`` (online), ``False`` (maintenance), or ``None`` (inconclusive
    → caller keeps the connect-only verdict).

    Calibrated against three captures: a region in MAINTENANCE answers the hello
    and sends FIN ~20-90ms later having emitted only a tiny (~39B, mostly empty)
    reply (EU). A LIVE gateway keeps the socket open for SECONDS waiting for the
    client to continue (US held 28s, PTS 6s) - so we hit the read timeout with no
    EOF. Bias is toward online: only a clean, near-empty, fast close is
    maintenance; a reset or a substantial reply is treated as online/inconclusive
    (a maintenance gateway closes gracefully, it doesn't RST)."""
    try:
        writer.write(hello)
        await writer.drain()
    except Exception:
        return None
    total = 0
    try:
        while True:
            chunk = await asyncio.wait_for(reader.read(4096), timeout=hold_seconds)
            if chunk == b"":
                # Server closed. Near-empty + fast close = maintenance (EU); a
                # close after a real payload means it engaged → online.
                return total > _GLS_SUBSTANTIVE_BYTES
            total += len(chunk)
            if total >= _GLS_MAX_READ_BYTES:
                return True   # actively streaming a session → online
    except asyncio.TimeoutError:
        return True           # held the socket open past the window → online
    except (ConnectionResetError, ConnectionError, BrokenPipeError):
        # A reset is NOT the observed maintenance signature (that's a graceful
        # FIN). Stay conservative - fall back to the connect-only verdict rather
        # than flag a live region down on an abrupt/transient reset.
        return None
    except Exception:
        return None           # anything else is inconclusive → fall back


async def _probe_game(
    host: str, port: int, *, deep: bool = False, hello_hex: str = "",
    hold_seconds: float = 2.0, random_opener: bool = False,
) -> dict:
    """Liveness of one environment's game glsserver (see module docstring for why
    a bare connect isn't enough)."""
    timeout = settings.trove_status_timeout_seconds
    t0 = time.monotonic()
    if not host or port <= 0:
        return {"online": False, "host": host, "port": port,
                "latency_ms": 0.0, "error": "not_configured"}
    try:
        reader, writer = await asyncio.wait_for(
            asyncio.open_connection(host, port), timeout=timeout,
        )
    except Exception as e:  # noqa: BLE001 - refused/timeout/dns = not reachable
        latency = (time.monotonic() - t0) * 1000
        return {"online": False, "host": host, "port": port,
                "latency_ms": round(latency, 1), "error": type(e).__name__}
    try:
        opener = b""
        opener_mode = None
        # A non-empty hello_hex ENABLES the deep probe for this env (PTS leaves it
        # empty → connect-only). With random_opener on we send a FRESH random opener
        # each probe - the real client uses a per-connection random ephemeral key,
        # so this behaves like a real client and never goes stale on a protocol
        # update. With it off we replay the captured hex (legacy; goes stale).
        if deep and hello_hex:
            if random_opener:
                opener = _random_opener()
                opener_mode = "random"
            else:
                try:
                    opener = bytes.fromhex(hello_hex.strip())
                    opener_mode = "replay"
                except ValueError:
                    opener = b""  # bad hex → skip the deep probe rather than misfire
        if not opener:
            # Connect-only (deep probe disabled / no hello): TCP accepted = online,
            # exactly the old behaviour.
            latency = (time.monotonic() - t0) * 1000
            return {"online": True, "host": host, "port": port,
                    "latency_ms": round(latency, 1), "error": None, "probe": "tcp"}
        held = await _glsserver_holds_session(reader, writer, opener, hold_seconds)
        latency = (time.monotonic() - t0) * 1000
        if held is None:
            # Inconclusive deep probe → keep the connect-only verdict (online) so
            # we never regress below the old check.
            return {"online": True, "host": host, "port": port,
                    "latency_ms": round(latency, 1), "error": None, "probe": "tcp_fallback"}
        return {"online": held, "host": host, "port": port,
                "latency_ms": round(latency, 1),
                "error": None if held else "glsserver_dropped",
                "probe": "glsserver", "opener": opener_mode}
    finally:
        try:
            writer.close()
            await writer.wait_closed()
        except Exception:
            pass


def _verdict(auth_online: bool, game_online: bool) -> str:
    # Binary: online only when the login gateway is reachable AND the region's
    # game socket is alive. Anything else is "down" (red). No "maintenance" state -
    # an unreachable server is just down.
    return "online" if (auth_online and game_online) else "down"


# Public Live regions that roll up into the homepage pill's "overall"
# status. PTS is excluded - it's a test server and shouldn't colour the
# public indicator.
_LIVE_REGIONS = ("eu", "us")


def _overall(environments: dict) -> str:
    """Roll the Live regions into one headline status for the homepage pill:
    ``online`` only when every Live region is online; otherwise ``down`` (any
    region unreachable). Consumers distinguish a full vs partial outage from the
    per-region statuses (e.g. headline "partially down" when some regions are
    still up). The per-region cards on /status carry the detail."""
    statuses = [
        environments[e]["status"] for e in _LIVE_REGIONS if e in environments
    ]
    if not statuses:
        return "unknown"
    return "online" if all(s == "online" for s in statuses) else "down"


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

    # Deep-probe knobs (live-tunable): replay the glsserver hello so a maintenance
    # server that still accepts TCP doesn't read as a false "online". The hello is
    # PER-ENVIRONMENT: EU/US are game glsservers that hold a hello-only probe open
    # (deep probe works), but PTS's gateway drops it even when up, so PTS ships an
    # empty hello → connect-only. An empty hello for an env = connect-only there.
    from app.admin import runtime_config
    deep = bool(await runtime_config.get_setting("trove_status_game_deep_probe"))
    hold_seconds = float(await runtime_config.get_setting("trove_status_game_hold_seconds"))
    random_opener = bool(await runtime_config.get_setting("trove_status_game_random_opener"))

    environments: dict[str, dict] = {}
    for env in _ENVIRONMENTS:
        host, port = await _env_endpoints(env)
        hello_hex = str(await runtime_config.get_setting(f"trove_status_{env}_hello_hex") or "")
        game = await _probe_game(
            host, port, deep=deep, hello_hex=hello_hex, hold_seconds=hold_seconds,
            random_opener=random_opener,
        )
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
    await _publish_snapshot(snapshot)
    return snapshot


async def _record_transition(env: str, status: str, online: bool) -> None:
    """Open a new status segment when ``env`` changes status, closing the
    previous open one. No-op when the status is unchanged. Best-effort -
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
            return  # unchanged - keep the open segment running
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
    """Latest cached snapshot, or an 'unknown' shell before the first probe.

    In-process only - returns the prober's cache, which is populated ONLY in the
    API process. Use ``get_status_shared`` from any other process (the bot)."""
    if _state is None:
        unknown_env = {"status": "unknown", "online": False, "game": None}
        return {
            "overall": "unknown",
            "auth": None,
            "environments": {env: dict(unknown_env) for env in _ENVIRONMENTS},
            "checked_at": None,
        }
    return _state


async def _publish_snapshot(snapshot: dict) -> None:
    """Mirror the latest snapshot to Redis so other processes can read it. Best-effort."""
    redis = get_redis()
    if redis is None:
        return
    try:
        await redis.set(_REDIS_KEY, json.dumps(snapshot, default=str), ex=_REDIS_TTL)
    except Exception:
        logger.warning("status: failed to cache snapshot in Redis", exc_info=True)


async def get_status_shared() -> dict:
    """Cross-process status snapshot: the prober's in-process cache when running in
    the API, else the snapshot the prober mirrored to Redis (read by the bot), else
    the 'unknown' shell. Use this anywhere outside the API process."""
    if _state is not None:
        return _state
    redis = get_redis()
    if redis is not None:
        try:
            raw = await redis.get(_REDIS_KEY)
            if raw:
                return json.loads(raw)
        except Exception:
            logger.warning("status: failed to read shared snapshot from Redis", exc_info=True)
    return get_status()


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
            # Push to the live event channel (SSE + the bot's status announcement).
            # Dedup makes this a no-op unless the overall status changed.
            try:
                from app.events import bus
                await bus.publish_type("server_status")
            except Exception:
                logger.warning("trove status: event publish failed", exc_info=True)
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
