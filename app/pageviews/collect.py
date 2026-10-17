"""The website's page-view collector: ``POST /s``, fed by ``site/static/stats.js``.

A page view is counted when the page's own script reports it, not when the server
renders HTML. Crawlers and link unfurlers that don't run JavaScript never show up,
prefetches aren't views, and the script can report foreground time on the page and
how fast it loaded.

The visitor hash, bot test (user agent, plus the network: cloud-hosted browsers are
scanners) and every label are computed here, in the website container; only the
digest and coarse labels go on to the API (via ``beacon``).
Always answers 204 so a blocked or malformed beacon tells the sender nothing.
"""
from __future__ import annotations

import hashlib
import json
import logging
import re
import time
from urllib.parse import unquote, urlsplit

from fastapi import APIRouter, Request, Response
from starlette.routing import Match

from app.core.config import settings
from app.core.middleware import _PAGE_PATHS, _PAGE_PREFIXES
from app.core.utils import client_ip, utcnow
from app.geo_maintenance import geoip
from app.pageviews import classify
from app.pageviews.beacon import beacon

logger = logging.getLogger("kiwi.pageviews")

router = APIRouter(include_in_schema=False)

_MAX_BODY = 4096
_PER_MINUTE = 120
_MAX_ENGAGED = 6 * 3600
_PERF_KEYS = ("ttfb", "fcp", "lcp", "dcl", "load")
_MAX_MS = 120_000
_MAX_CLS = 10_000     # CLS x1000
_PID = re.compile(r"^[A-Za-z0-9-]{8,40}$")
_LANGS = frozenset({"en", "de", "es", "fr", "ja", "ko", "pt-PT", "ru", "th", "zh-CN"})

_rate: dict[str, tuple[int, int]] = {}


def is_page_template(template: str) -> bool:
    """True for site PAGE routes, from the same list that grants the site CSP."""
    return template in _PAGE_PATHS or template.startswith(_PAGE_PREFIXES)


def visitor_hash(request: Request) -> str:
    """A cookieless, daily-rotating visitor id: ``sha256(secret | UTC-date | ip | ua)``.

    The date in the salt means the same visitor hashes differently each day, and no
    raw IP or User-Agent is kept. The salt is ``internal_key`` (falling back to
    ``secret_key``), shared by both containers."""
    ip = client_ip(request) or "unknown"
    ua = request.headers.get("user-agent", "")
    secret = settings.internal_key or settings.secret_key
    salt = f"{secret}|{utcnow():%Y-%m-%d}"
    return hashlib.sha256(f"{salt}|{ip}|{ua}".encode()).hexdigest()[:32]


def page_template(request: Request, path: str) -> str | None:
    """The page route ``path`` resolves to (e.g. ``/mods/{handle}/{slug}``), or None
    when it isn't a page this site serves."""
    scope = {"type": "http", "path": path, "method": "GET", "root_path": ""}
    for route in request.app.router.routes:
        match, _ = route.matches(scope)
        if match == Match.FULL:
            template = getattr(route, "path", "")
            return template if is_page_template(template) else None
    return None


def _over_limit(ip: str) -> bool:
    minute = int(time.time() // 60)
    seen_minute, count = _rate.get(ip, (minute, 0))
    count = count + 1 if seen_minute == minute else 1
    _rate[ip] = (minute, count)
    if len(_rate) > 20_000:
        for key in [k for k, (m, _) in _rate.items() if m != minute]:
            del _rate[key]
    return count > _PER_MINUTE


def _int(value, lo: int, hi: int) -> int:
    try:
        return max(lo, min(hi, int(value)))
    except (TypeError, ValueError, OverflowError):
        return lo


def build_event(request: Request, data: dict) -> dict | None:
    """The beacon payload as a stored event, or None to drop it."""
    pid = str(data.get("id", ""))
    if not _PID.match(pid):
        return None
    kind = data.get("t")
    if kind == "end":
        return {"kind": "end", "pid": pid, "engaged_s": _int(data.get("s"), 0, _MAX_ENGAGED)}
    if kind == "perf":
        # 0 = not measured, so it is left out rather than stored.
        perf = {k: v for k in _PERF_KEYS if (v := _int(data.get(k), 0, _MAX_MS))}
        return {"kind": "perf", "pid": pid, "perf": perf} if perf else None
    if kind == "cls":
        cls = _int(data.get("c"), -1, _MAX_CLS)
        return {"kind": "perf", "pid": pid, "perf": {"cls": cls}} if cls >= 0 else None
    if kind != "pv":
        return None

    page_url = str(data.get("u", ""))[:2000]
    try:
        parts = urlsplit(page_url)
    except ValueError:
        return None
    if not classify.is_own_host((parts.hostname or "").lower().removeprefix("www.")):
        return None
    path = unquote(parts.path or "/")[:500]
    template = page_template(request, path)
    if template is None:
        return None

    ua = request.headers.get("user-agent", "")
    width = _int(data.get("w"), 0, 20_000)
    touch = _int(data.get("tp"), 0, 20)
    channel, source = classify.classify(page_url, str(data.get("r", ""))[:2000])
    lang = data.get("l")
    return {
        "kind": "pv",
        "pid": pid,
        "route": template,
        "path": path,
        "visitor_hash": visitor_hash(request),
        "channel": channel,
        "source": source,
        "device": classify.device(ua, width, touch),
        "browser": classify.browser(ua),
        "os": classify.os_name(ua, touch),
        "lang": lang if lang in _LANGS else None,
    }


@router.post("/s", status_code=204)
async def collect(request: Request) -> Response:
    """Record one beacon from the site's stats script (best-effort, always 204)."""
    try:
        ua = request.headers.get("user-agent", "")
        origin = request.headers.get("origin", "")
        if not beacon.enabled or classify.is_bot(ua):
            return Response(status_code=204)
        if origin and not classify.is_own_host((urlsplit(origin).hostname or "").lower()):
            return Response(status_code=204)
        ip = client_ip(request) or "unknown"
        if geoip.asn(ip) in classify.CLOUD_ASNS or _over_limit(ip):
            return Response(status_code=204)
        body = await request.body()
        if len(body) > _MAX_BODY:
            return Response(status_code=204)
        data = json.loads(body or b"{}")
        if isinstance(data, dict):
            event = build_event(request, data)
            if event is not None:
                beacon.record(event)
    except Exception:  # noqa: BLE001 - analytics never fail a request
        logger.debug("page-view beacon dropped", exc_info=True)
    return Response(status_code=204)
