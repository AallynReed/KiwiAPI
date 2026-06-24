"""Live event stream (Server-Sent Events).

A push replacement for polling the challenge / chaos-chest endpoints at the top
of the hour. A client opens ONE long-lived GET and receives an event the instant
a capture lands - plus a snapshot of the current state on connect, so there is
nothing to poll.

    GET /v1/events/stream      (scope: events:read; tokenless allowed)

Wire format (text/event-stream):

    retry: 5000

    event: challenge
    data: {"type":"challenge","data":{...},"ts":1718200000}

    event: chaos
    data: {"type":"chaos","data":{...},"ts":1718200000}

    : ping            <- keep-alive comment every ~20s

Event types:
  - ``challenge`` - an hourly challenge was captured
  - ``chaos``     - the chaos-chest item
  - ``mod_release`` - a new mod release was published on the Mods Hub. ``data`` =
    ``{project:{slug,title,owner}, release:{id,tag,title,branch,format,size,changelog,
    published_at}, download_url, page_url}``. Discrete (not in the on-connect snapshot) -
    you only receive ones published while connected; hook it to mirror/announce releases.

The SSE ``event:`` field carries the type, and the JSON ``data:`` payload repeats it as
``{type, data, ts}`` so both EventSource listeners and raw parsers work. Reconnect on
disconnect - the on-connect snapshot re-primes the singleton (challenge/chaos) state.
"""
import asyncio
import json
import logging

from fastapi import APIRouter, Depends, Request
from starlette.responses import StreamingResponse

from app.core.config import settings
from app.core.dependencies import AccessContext, public_scope
from app.core.errors import APIError, ErrorCode
from app.events import bus

logger = logging.getLogger("kiwi.events.router")

router = APIRouter(prefix="/v1/events", tags=["events"])

_EVENTS = Depends(public_scope("events:read"))


def _sse(payload: dict) -> str:
    """One SSE message: ``event:`` = the type, ``data:`` = the JSON envelope."""
    return f"event: {payload['type']}\ndata: {json.dumps(payload, default=str)}\n\n"


@router.get("/stream", summary="Subscribe to the live event stream (SSE)")
async def stream(request: Request, ctx: AccessContext = _EVENTS) -> StreamingResponse:
    """Open a Server-Sent Events stream of live game-data updates.

    On connect you receive a snapshot of the current ``challenge`` and ``chaos``
    state, then one event per change as it happens (no polling) - plus discrete
    ``mod_release`` events when a mod release is published on the Mods Hub. Each
    message is an ``event: <type>`` line plus a JSON ``data:`` payload
    ``{type, data, ts}``. A ``: ping`` keep-alive comment arrives roughly every 20
    seconds; reconnect on disconnect and the snapshot re-primes you."""
    if bus.connection_count() >= settings.events_max_connections:
        raise APIError(
            503, ErrorCode.service_unavailable,
            "The event stream is at capacity; retry shortly.",
        )

    heartbeat = max(5, settings.events_heartbeat_seconds)

    async def gen():
        # Advise the browser EventSource auto-reconnect delay (ms).
        yield "retry: 5000\n\n"
        # Snapshot current state so a fresh client needs no polling.
        try:
            for ev in await bus.current_snapshot():
                yield _sse(ev)
        except Exception:
            logger.warning("event stream: snapshot failed", exc_info=True)
        async with bus.subscribe() as q:
            while True:
                if await request.is_disconnected():
                    break
                try:
                    ev = await asyncio.wait_for(q.get(), timeout=heartbeat)
                except TimeoutError:
                    yield ": ping\n\n"   # keep-alive comment (also surfaces dead sockets)
                    continue
                yield _sse(ev)

    return StreamingResponse(
        gen(),
        media_type="text/event-stream",
        headers={
            "Cache-Control": "no-cache",
            "Connection": "keep-alive",
            # Tell nginx (and other buffering proxies) to flush the stream live.
            "X-Accel-Buffering": "no",
        },
    )
