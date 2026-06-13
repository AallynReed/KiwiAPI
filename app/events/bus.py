"""Cross-worker fan-out for the live event stream, backed by Redis pub/sub.

The API runs several uvicorn workers in production (docker-compose.prod.yml:
``--workers 4``). A change captured in ONE worker must reach SSE clients connected
to ANY worker, so events travel over a Redis pub/sub channel: a worker
``publish()``es, every worker's ``_listen()`` task receives, and each forwards the
event to its locally-connected streams.

**Exactly-once across workers.** ``publish()`` only emits when the event's
*signature* (e.g. the challenge anchor + name) differs from the last one stored in
Redis - an atomic SET-with-GET compare-and-set. So every worker's safety-net
watcher can call ``publish()`` freely; only the first to observe a new signature
actually emits. This also collapses the capture-insert hook and the watcher into a
single event when they race.

**No Redis (single-worker dev).** Falls back to an in-process signature cache +
direct local broadcast, so the stream still works without Redis.
"""
import asyncio
import inspect
import json
import logging
from contextlib import asynccontextmanager

from app.core.config import settings
from app.core.redis import get_redis
from app.core.utils import utcnow

logger = logging.getLogger("kiwi.events")

# Local (this-worker) subscriber queues - one per open SSE connection.
_subscribers: set[asyncio.Queue] = set()
_listener: asyncio.Task | None = None
_watcher: asyncio.Task | None = None
# In-process dedup fallback used only when Redis is absent.
_local_sig: dict[str, str] = {}


def connection_count() -> int:
    """Open SSE streams on THIS worker (used for the per-worker capacity cap)."""
    return len(_subscribers)


@asynccontextmanager
async def subscribe():
    """Register a local queue for the lifetime of one SSE connection."""
    q: asyncio.Queue = asyncio.Queue(maxsize=64)
    _subscribers.add(q)
    try:
        yield q
    finally:
        _subscribers.discard(q)


def _broadcast_local(payload: dict) -> None:
    """Hand the event to every locally-connected stream. A full queue is dropped:
    one stuck/slow consumer must never back up the others."""
    for q in list(_subscribers):
        try:
            q.put_nowait(payload)
        except asyncio.QueueFull:
            pass


async def publish(event_type: str, signature: str, data: dict) -> bool:
    """Emit an event IFF its signature changed since the last emit of this type.

    Returns True if an event was actually published. Safe to call from every
    worker concurrently - the Redis compare-and-set makes the emit exactly-once."""
    payload = {"type": event_type, "data": data, "ts": int(utcnow().timestamp())}
    redis = get_redis()
    if redis is None:
        # Dev / no-Redis: in-process dedup + direct local fan-out.
        if _local_sig.get(event_type) == signature:
            return False
        _local_sig[event_type] = signature
        _broadcast_local(payload)
        return True
    try:
        # SET ... GET returns the previous value (Redis >= 6.2). If it already
        # equals our signature, another worker (or our own earlier publish) has
        # announced this exact state - skip.
        prev = await redis.set(f"events:sig:{event_type}", signature, get=True)
        if prev == signature:
            return False
        await redis.publish(settings.events_channel, json.dumps(payload, default=str))
        return True
    except Exception:
        logger.warning("event publish failed (%s)", event_type, exc_info=True)
        return False


# ── concrete event sources ──────────────────────────────────────────────────


async def _source_data(source) -> dict:
    """Read a source's current data (its data_fn may be sync or async)."""
    data = source.data_fn()
    return await data if inspect.isawaitable(data) else data


async def publish_source(source) -> bool:
    """Publish one ``EventSource``'s current state, IFF its signature changed.

    Used by the ingest endpoints (insert-driven) and the scheduler (time-driven);
    both go through the same dedup so a duplicate from a racing worker collapses."""
    data = await _source_data(source)
    sig = source.sig_fn(data)
    if sig is None:
        return False
    return await publish(source.type, sig, data)


async def publish_type(type_name: str) -> bool:
    """Publish the current state of the named source (by type). Used by the
    state-change producers (status prober, news refresher, giveaway worker) to
    fire their event - dedup makes it a no-op unless the signature changed."""
    from app.events.sources import SOURCES_BY_TYPE
    source = SOURCES_BY_TYPE.get(type_name)
    return await publish_source(source) if source is not None else False


async def publish_challenge() -> bool:
    """Announce the current hourly challenge (no-op until one is captured)."""
    from app.events.sources import SOURCES_BY_TYPE
    return await publish_source(SOURCES_BY_TYPE["challenge"])


async def publish_chaos() -> bool:
    """Announce the current chaos-chest item (no-op until one is known)."""
    from app.events.sources import SOURCES_BY_TYPE
    return await publish_source(SOURCES_BY_TYPE["chaos"])


async def current_snapshot() -> list[dict]:
    """The current state of every event source as event payloads, so a freshly
    connected SSE client has the latest values immediately (then live updates).
    Skips a source whose data isn't ready yet (signature is None)."""
    from app.events.sources import SOURCES
    out: list[dict] = []
    ts = int(utcnow().timestamp())
    for source in SOURCES:
        try:
            data = await _source_data(source)
            if source.sig_fn(data) is not None:
                out.append({"type": source.type, "data": data, "ts": ts})
        except Exception:
            logger.warning("snapshot: %s fetch failed", source.type, exc_info=True)
    return out


# ── background tasks (one of each per worker) ────────────────────────────────

async def _listen() -> None:
    """Subscribe to the Redis channel and fan published events out to this
    worker's connected streams."""
    redis = get_redis()
    if redis is None:
        return
    pubsub = redis.pubsub()
    await pubsub.subscribe(settings.events_channel)
    try:
        async for msg in pubsub.listen():
            if msg.get("type") != "message":
                continue  # skip the subscribe/unsubscribe confirmations
            try:
                payload = json.loads(msg["data"])
            except (ValueError, TypeError):
                continue
            _broadcast_local(payload)
    except asyncio.CancelledError:
        raise
    finally:
        try:
            await pubsub.unsubscribe(settings.events_channel)
            await pubsub.aclose()
        except Exception:
            pass


async def _watch() -> None:
    """Safety net: periodically re-check current challenge/chaos and publish on
    change. Catches updates that don't arrive via an insert (e.g. the chaos
    relay) and re-announces if the live publish was missed. Dedup makes it
    idempotent, so running it in every worker is harmless."""
    interval = max(5, settings.events_watch_seconds)
    while True:
        try:
            await publish_challenge()
            await publish_chaos()
        except asyncio.CancelledError:
            raise
        except Exception:
            logger.warning("event watch tick failed", exc_info=True)
        try:
            await asyncio.sleep(interval)
        except asyncio.CancelledError:
            raise


def start_event_bus() -> None:
    global _listener, _watcher
    if _listener is None:
        _listener = asyncio.create_task(_listen())
    if _watcher is None:
        _watcher = asyncio.create_task(_watch())


async def stop_event_bus() -> None:
    global _listener, _watcher
    for task in (_listener, _watcher):
        if task is not None:
            task.cancel()
            try:
                await task
            except asyncio.CancelledError:
                pass
    _listener = None
    _watcher = None
