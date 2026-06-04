"""Idempotency-Key support for unsafe requests.

A client may send an ``Idempotency-Key`` header on a POST/PUT/PATCH/DELETE. If the
same key is replayed for the same credential + method + path — e.g. after a
network timeout where the client never saw the response — the original response
is returned instead of performing the operation again.

Backed by Redis and **fail-open**: if Redis is unavailable or anything goes wrong
with the bookkeeping, the request simply proceeds normally (we never fail a real
request because of idempotency plumbing). Only successful (2xx) responses are
cached for replay; a non-2xx releases the key so the client can retry.
"""

import base64
import hashlib
import json
import logging

from fastapi import FastAPI, Request
from starlette.responses import JSONResponse, Response

from app.core.config import settings
from app.core.errors import ErrorCode
from app.core.redis import get_redis

logger = logging.getLogger("kiwi.idempotency")

_METHODS = {"POST", "PUT", "PATCH", "DELETE"}
_HEADER = "idempotency-key"
_MAX_KEY_LEN = 200


def _storage_key(request: Request, key: str) -> str:
    # Namespace by the credential (so one user's key can't collide with another's)
    # plus method + path, so the same key on a different operation is independent.
    cred = hashlib.sha256(request.headers.get("authorization", "").encode()).hexdigest()[:16]
    raw = f"{cred}:{request.method}:{request.url.path}:{key}"
    return "idem:" + hashlib.sha256(raw.encode()).hexdigest()


def _conflict() -> JSONResponse:
    return JSONResponse(
        status_code=409,
        content={
            "error": {
                "code": ErrorCode.conflict.value,
                "message": "A request with this Idempotency-Key is already in progress.",
                "details": None,
            }
        },
    )


def add_idempotency_middleware(app: FastAPI) -> None:
    @app.middleware("http")
    async def idempotency(request: Request, call_next):
        key = request.headers.get(_HEADER)
        redis = get_redis()
        if not key or len(key) > _MAX_KEY_LEN or request.method not in _METHODS or redis is None:
            return await call_next(request)

        sk = _storage_key(request, key)
        try:
            claimed = await redis.set(sk, '{"s":"lock"}', nx=True, ex=settings.idempotency_lock_seconds)
            if not claimed:
                stored = await redis.get(sk)
                data = json.loads(stored) if stored else {}
                if data.get("s") == "done":
                    body = base64.b64decode(data["b"])
                    return Response(
                        content=body,
                        status_code=data["c"],
                        media_type=data.get("ct"),
                        headers={"Idempotent-Replay": "true"},
                    )
                return _conflict()  # still in flight
        except Exception:
            logger.warning("idempotency lookup failed; proceeding without it", exc_info=True)
            return await call_next(request)

        # We own the key — run the operation and capture its response.
        response = await call_next(request)
        try:
            body = b"".join([chunk async for chunk in response.body_iterator])
        except Exception:
            await _release(redis, sk)
            return response

        if 200 <= response.status_code < 300:
            record = json.dumps({
                "s": "done",
                "c": response.status_code,
                "ct": response.headers.get("content-type"),
                "b": base64.b64encode(body).decode(),
            })
            try:
                await redis.set(sk, record, ex=settings.idempotency_result_seconds)
            except Exception:
                logger.warning("idempotency store failed", exc_info=True)
        else:
            await _release(redis, sk)  # let the client retry a failed write

        # The body_iterator was consumed; rebuild an equivalent response.
        return Response(
            content=body,
            status_code=response.status_code,
            headers=dict(response.headers),
            media_type=response.media_type,
        )


async def _release(redis, sk: str) -> None:
    try:
        await redis.delete(sk)
    except Exception:
        pass
