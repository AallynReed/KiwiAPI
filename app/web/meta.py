"""Crawler-facing metadata routes for the website: robots.txt, sitemap.xml,
BingSiteAuth.xml. Not feature-gated (they must always answer), so they live on
their own router without the ``web_flags.resolve`` page dependency.

The sitemap enumerates the public modpack catalog over the internal API and is
memoised a few minutes so crawler hits don't re-page the catalog each time.
"""
import asyncio
import logging
import time

from fastapi import APIRouter, Request
from fastapi.responses import Response

from app.core.config import settings
from app.site.feature_map import SITEMAP_PAGES, robots_body
from app.web import feature_flags as web_flags
from app.web.pages import _all_public_modpack_cards

logger = logging.getLogger("kiwi.web.meta")

router = APIRouter(tags=["web"], include_in_schema=False)


@router.get("/robots.txt")
async def robots_txt(request: Request) -> Response:
    """Crawler directives, host-aware (see ``feature_map.robots_body``)."""
    return Response(
        robots_body(request.url.hostname or ""), media_type="text/plain",
        headers={"Cache-Control": "public, max-age=86400"},
    )


# Bing Webmaster Tools site-ownership verification. Bing fetches this XML from
# the site ROOT (not /static), so it needs a real root route.
_BING_VERIFY = (
    '<?xml version="1.0"?>\n'
    '<users>\n'
    '\t<user>FC86658CF71BBCB1184266DE6480D237</user>\n'
    '</users>\n'
)


@router.get("/BingSiteAuth.xml")
async def bing_site_auth() -> Response:
    """Bing Webmaster Tools ownership-verification file, served at the site root
    so Bing's fetcher (and IndexNow) can confirm the domain."""
    return Response(
        _BING_VERIFY, media_type="application/xml",
        headers={"Cache-Control": "public, max-age=86400"},
    )


# In-process cache for the rendered sitemap - the modpack section enumerates the
# catalog over the internal API, and Cloudflare won't reliably edge-cache a
# generated .xml, so the body is memoised for a few minutes.
_SITEMAP_TTL = 600.0
_SITEMAP_CACHE: dict = {"body": None, "at": -1e9}
_SITEMAP_LOCK = asyncio.Lock()


def _xml_loc(url: str) -> str:
    return url.replace("&", "&amp;").replace("<", "&lt;").replace(">", "&gt;")


async def _render_sitemap() -> str:
    """Build the sitemap XML: static feature pages (each gated by its master
    toggle) plus every public modpack page. Individual mod detail pages are
    excluded (their above-the-fold content is JS-rendered, so as raw HTML they
    read thin). The modpack section rides the Mods Hub master toggle."""
    base = settings.app_url.rstrip("/")
    flags = await web_flags._fetch()
    # (loc, lastmod-iso-or-None)
    entries: list[tuple[str, str | None]] = [
        (base + path, None)
        for path, attr in SITEMAP_PAGES
        if attr is None or flags.get(attr, True)
    ]
    if flags.get("mods_hub_enabled", True):
        packs = await _all_public_modpack_cards()
        entries += [
            (f"{base}/modpacks/{c['handle']}/{c['slug']}", c.get("updated_at"))
            for c in packs if c.get("handle") and c.get("slug")
        ]

    def _url(loc: str, lastmod: str | None) -> str:
        inner = f"<loc>{_xml_loc(loc)}</loc>"
        if lastmod:
            inner += f"<lastmod>{lastmod}</lastmod>"
        return f"  <url>{inner}</url>\n"

    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
        + "".join(_url(loc, lm) for loc, lm in entries)
        + "</urlset>\n"
    )


@router.get("/sitemap.xml")
async def sitemap_xml() -> Response:
    """XML sitemap of the public, indexable pages: the static feature pages plus
    every public modpack. Cached in-process for a few minutes so crawler hits
    don't re-enumerate the catalog each time."""
    now = time.monotonic()
    if _SITEMAP_CACHE["body"] is None or now - float(_SITEMAP_CACHE["at"]) > _SITEMAP_TTL:
        async with _SITEMAP_LOCK:
            now = time.monotonic()
            if _SITEMAP_CACHE["body"] is None or now - float(_SITEMAP_CACHE["at"]) > _SITEMAP_TTL:
                _SITEMAP_CACHE["body"] = await _render_sitemap()
                _SITEMAP_CACHE["at"] = now
    return Response(
        _SITEMAP_CACHE["body"], media_type="application/xml",
        headers={"Cache-Control": "public, max-age=600"},
    )
