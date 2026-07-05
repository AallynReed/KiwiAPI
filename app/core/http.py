"""Shared outbound-HTTP helpers for the simple upstream relays."""

from __future__ import annotations

from typing import Any

import httpx

# Identifies our relays to upstreams. Bespoke clients (custom headers/UA) keep
# their own; this is the default for the plain relays.
KIWI_UA = "KiwiAPI/1.0"
# Browser-spoof UA for upstreams that reject non-browser clients.
BROWSER_UA = (
    "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 "
    "(KHTML, like Gecko) Chrome/122.0 Safari/537.36"
)


async def fetch_text(
    url: str, *, timeout: float = 15, headers: dict[str, str] | None = None,
) -> str:
    # follow_redirects: some feeds 301 to a trailing-slash path; without it
    # raise_for_status() passes the 3xx through and we'd parse the redirect page.
    async with httpx.AsyncClient(
        timeout=timeout, follow_redirects=True,
        headers=headers or {"User-Agent": KIWI_UA},
    ) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp.text


async def fetch_json(
    url: str, *, timeout: float = 15, headers: dict[str, str] | None = None,
) -> Any:
    async with httpx.AsyncClient(
        timeout=timeout, follow_redirects=True,
        headers=headers or {"User-Agent": KIWI_UA},
    ) as client:
        resp = await client.get(url)
        resp.raise_for_status()
        return resp.json()
