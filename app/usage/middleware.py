import logging
import time

from fastapi import FastAPI, Request

from app.usage.models import UsageEvent
from app.usage.recorder import recorder

logger = logging.getLogger("kiwi.usage")


def add_usage_middleware(app: FastAPI) -> None:
    """Record one UsageEvent per authenticated API request.

    The API-token dependency stashes user/token ids on `request.state`; this
    middleware reads them after the response is produced, so only token-auth'd
    requests are logged (management/JWT routes are skipped). Events are queued on
    a buffered recorder and flushed in batches; queuing never breaks the request.
    """

    @app.middleware("http")
    async def record_usage(request: Request, call_next):
        start = time.perf_counter()
        response = await call_next(request)

        token_id = getattr(request.state, "usage_token_id", None)
        if token_id is not None:
            route = request.scope.get("route")
            try:
                recorder.record(
                    UsageEvent(
                        user_id=request.state.usage_user_id,
                        token_id=token_id,
                        method=request.method,
                        route=getattr(route, "path", request.url.path),
                        path=request.url.path,
                        status_code=response.status_code,
                        duration_ms=round((time.perf_counter() - start) * 1000, 2),
                    )
                )
            except Exception:
                logger.exception("Failed to queue usage event")

        return response
