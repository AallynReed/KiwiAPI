"""Shared outbound-HTTP helpers for the simple upstream relays."""

from __future__ import annotations

import httpx

# Identifies our relays to upstreams. Bespoke clients (custom headers/UA) keep
# their own; this is the default for the plain relays.
KIWI_UA = "KiwiAPI/1.0"
# Browser-spoof UA for upstreams that reject non-browser clients.
BROWSER_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/122.0 Safari/537.36"
)


async def fetch(
    url: str, *, timeout: float = 15, headers: dict[str, str] | None = None,
) -> httpx.Response:
    """GET a URL, raising on a non-2xx. Caller takes ``.text`` or ``.json()``.

    The body is fully READ before the client closes (httpx reads eagerly on a
    non-streaming send), so the returned response is safe to use after this
    returns - but only for the already-materialised body. Streaming accessors
    (``.aiter_bytes()``, ``.aread()``) will fail on the closed transport; if you
    need those, open your own client instead of reaching for this helper.

    follow_redirects: some feeds 301 to a trailing-slash path; without it
    raise_for_status() passes the 3xx through and we'd parse the redirect page.
    """
    async with httpx.AsyncClient(
        timeout=timeout, follow_redirects=True,
        headers=headers or {"User-Agent": KIWI_UA},
    ) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp
