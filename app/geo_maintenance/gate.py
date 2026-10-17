"""Website side: serve the maintenance page to visitors from the configured countries.

The list comes from the API (memoised briefly). If the API can't be reached the last
list stays in force, so a blip neither opens nor closes the gate.
"""
import time
from pathlib import Path

from fastapi import FastAPI, Request
from fastapi.responses import HTMLResponse

from app.core.config import settings
from app.core.internal_api import internal_get
from app.core.utils import client_ip
from app.geo_maintenance import geoip

PAGE = (Path(__file__).parent / "maintenance.html").read_text(encoding="utf-8")
_HEADERS = {
    "Retry-After": "120",
    "Cache-Control": "no-store",
    "Content-Security-Policy": (
        "default-src 'none'; style-src 'self' 'unsafe-inline'; font-src 'self'; "
        "img-src 'self'; base-uri 'none'; form-action 'none'; frame-ancestors 'none'"
    ),
}
_EXEMPT = ("/static/", "/health")
_TTL = 30.0
_cache: dict = {"at": -1e9, "countries": frozenset()}


async def countries() -> frozenset[str]:
    if not settings.internal_key:
        return frozenset()
    now = time.monotonic()
    if now - _cache["at"] > _TTL:
        _cache["at"] = now
        data = await internal_get("/internal/geo-maintenance", timeout=3.0,
                                  headers={"X-Internal-Key": settings.internal_key})
        if isinstance(data, dict) and isinstance(data.get("countries"), list):
            _cache["countries"] = frozenset(data["countries"])
    return _cache["countries"]


def add_geo_maintenance_middleware(app: FastAPI) -> None:
    @app.middleware("http")
    async def geo_maintenance(request: Request, call_next):
        if not request.url.path.startswith(_EXEMPT):
            blocked = await countries()
            if blocked:
                geoip.ensure_running()
                if geoip.country(client_ip(request)) in blocked:
                    return HTMLResponse(PAGE, status_code=503, headers=_HEADERS)
        return await call_next(request)
