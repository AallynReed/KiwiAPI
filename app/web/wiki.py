"""The wiki host (``settings.wiki_url``), served by the website container.

A separate app mounted by hostname in ``app/web/main.py``, so none of the main
site's routes answer here and none of these answer on the main site. Like the
rest of this container it holds no database: page text comes from the API's
``/site/wiki/*`` over ``internal_get``, and class data is read from the game-data
files the ``/classes`` page already renders from.

URL map:
  /                      home
  /classes, /class/<n>   generated class pages + their editable write-ups
  /<slug>                an article
  /-/...                 tools: edit, history, search, recent, pages, suggestions
"""
import logging
from pathlib import Path
from urllib.parse import quote

from fastapi import Depends, FastAPI, HTTPException, Request
from fastapi.responses import HTMLResponse, RedirectResponse, Response
from fastapi.staticfiles import StaticFiles
from fastapi.templating import Jinja2Templates
from starlette.exceptions import HTTPException as StarletteHTTPException

from app.core.config import settings
from app.core.csp import SITE_CSP
from app.core.internal_api import internal_get
from app.site import classes_page
from app.site.feature_map import robots_body
from app.trove import stats as trove_stats
from app.web import feature_flags as web_flags

logger = logging.getLogger("kiwi.web.wiki")

WIKI = settings.wiki_url.rstrip("/")
WIKI_HOST = WIKI.split("://", 1)[-1].split("/", 1)[0].lower()


def _context(request: Request) -> dict:
    return {"wiki_url": WIKI, "site_url": settings.app_url.rstrip("/"),
            "path": request.url.path}


_TEMPLATES = Jinja2Templates(
    directory=str(Path(settings.site_root) / "templates"),
    context_processors=[_context],
)


async def _gate(request: Request) -> None:
    """404 the whole wiki while its master switch is off."""
    flags = await web_flags._fetch()
    if not flags.get("wiki_enabled", True):
        raise HTTPException(status_code=404)


app = FastAPI(openapi_url=None, docs_url=None, redoc_url=None)


@app.middleware("http")
async def _headers(request: Request, call_next):
    # Set before the outer security layer, which only fills what is missing.
    response = await call_next(request)
    response.headers.setdefault("Content-Security-Policy", SITE_CSP)
    response.headers.setdefault("Cache-Control", "no-cache")
    return response


_STATIC = Path(settings.site_root) / "static"
if _STATIC.is_dir():
    app.mount("/static", StaticFiles(directory=str(_STATIC)), name="wiki_static")


@app.exception_handler(StarletteHTTPException)
async def _http_error(request: Request, exc: StarletteHTTPException):
    if exc.status_code == 404:
        return _render(request, "wiki/missing.html", {"title": "Not found", "slug": None}, 404)
    return Response(status_code=exc.status_code)


def _render(request: Request, name: str, ctx: dict, status: int = 200) -> HTMLResponse:
    return _TEMPLATES.TemplateResponse(request, name, ctx, status_code=status)


# ── classes ────────────────────────────────────────────────────────────────

def _class_slug(c: dict) -> str:
    """URL name: the display name ("Pirate Captain" -> pirate-captain), since the
    game's internal names (piratelord) aren't what players call them."""
    return "-".join("".join(ch if ch.isalnum() else " " for ch in c["name"].lower()).split())


def _classes() -> list[dict]:
    return sorted((trove_stats.all_classes() or {}).get("items") or [], key=lambda c: c["name"])


def _find_class(name: str) -> dict | None:
    name = name.lower()
    for c in _classes():
        if name in (_class_slug(c), c["tech_name"]):
            return c
    return None


def _class_cards() -> list[dict]:
    return [{"url": f"/class/{_class_slug(c)}", "tech": c["tech_name"], "name": c["name"],
             "damage_type": c.get("damage_type") or "",
             "dmg_class": "physical" if (c.get("damage_type") or "").lower() == "physical" else "magic"}
            for c in _classes()]


async def _page(slug: str) -> dict | None:
    return await internal_get("/site/wiki/page", {"slug": slug}, timeout=3.0)


def _excerpt(body: str, n: int = 180) -> str:
    text = " ".join(line.strip("#>*-_ ") for line in (body or "").splitlines() if line.strip())
    return text[:n].rsplit(" ", 1)[0] + "…" if len(text) > n else text


# ── routes ─────────────────────────────────────────────────────────────────

@app.get("/robots.txt")
async def robots() -> Response:
    return Response(robots_body(WIKI_HOST), media_type="text/plain",
                    headers={"Cache-Control": "public, max-age=86400"})


@app.get("/sitemap.xml", dependencies=[Depends(_gate)])
async def sitemap() -> Response:
    urls = [WIKI + "/", WIKI + "/classes"] + [WIKI + c["url"] for c in _class_cards()]
    data = await internal_get("/site/wiki/pages", timeout=5.0) or {}
    urls += [f"{WIKI}/{quote(p['slug'])}" for p in data.get("items", [])
             if "/" not in p["slug"]]
    body = ('<?xml version="1.0" encoding="UTF-8"?>\n'
            '<urlset xmlns="http://www.sitemaps.org/schemas/sitemap/0.9">\n'
            + "".join(f"  <url><loc>{u}</loc></url>\n" for u in urls)
            + "</urlset>\n")
    return Response(body, media_type="application/xml",
                    headers={"Cache-Control": "public, max-age=600"})


@app.get("/health", include_in_schema=False)
async def health() -> dict:
    return {"status": "ok"}


@app.get("/", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def home(request: Request) -> HTMLResponse:
    return _render(request, "wiki/home.html", {"title": "Kiwi Wiki", "classes": _class_cards()})


@app.get("/classes", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def classes(request: Request) -> HTMLResponse:
    return _render(request, "wiki/classes.html", {"title": "Classes", "classes": _class_cards()})


@app.get("/class/{name}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def class_page(request: Request, name: str) -> Response:
    c = _find_class(name)
    if c is None:
        raise HTTPException(status_code=404)
    if name != _class_slug(c):
        return RedirectResponse(f"/class/{_class_slug(c)}", status_code=301)
    slug = f"class/{c['tech_name']}"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    return _render(request, "wiki/class.html", {
        "title": c["name"],
        "slug": slug,
        "d": classes_page._detail(c),
        # Just what the level slider needs to recompute the sheet client-side.
        "class_data": {"stats": c.get("stats") or [], "levels": c.get("levels") or {}},
        "page": live,
        "rev": page["rev"] if page else 0,
        "description": _excerpt(live["body"]) if live and live.get("body")
        else f"{c['name']} in Trove: base stats, level scaling, subclass and abilities.",
    })


@app.get("/-/{tool}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def tool(request: Request, tool: str) -> HTMLResponse:
    titles = {"search": "Search", "recent": "Recent changes", "pages": "All pages",
              "suggestions": "Suggestions", "new": "New page"}
    if tool not in titles:
        raise HTTPException(status_code=404)
    if tool == "new":
        return _render(request, "wiki/edit.html", {"title": titles[tool], "slug": "", "fixed_title": None})
    return _render(request, "wiki/tool.html", {
        "title": titles[tool], "tool": tool,
        "classes": [{"name": c["name"], "url": c["url"]} for c in _class_cards()] if tool == "search" else None,
    })


@app.get("/-/suggestions/{sid}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def suggestion(request: Request, sid: str) -> HTMLResponse:
    return _render(request, "wiki/tool.html", {"title": "Review suggestion", "tool": "suggestion", "sid": sid})


def _fixed_title(slug: str) -> str | None:
    if slug.startswith("class/"):
        c = trove_stats.class_by_tech_name(slug[6:])
        if c is None:
            raise HTTPException(status_code=404)
        return c["name"]
    return None


def page_url(slug: str) -> str:
    """Where a stored slug is read: class/<tech> lives at /class/<display name>."""
    if slug.startswith("class/"):
        c = trove_stats.class_by_tech_name(slug[6:])
        return f"/class/{_class_slug(c)}" if c else "/classes"
    return "/" + slug


@app.get("/-/edit/{slug:path}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def edit(request: Request, slug: str) -> HTMLResponse:
    slug = slug.lower()
    return _render(request, "wiki/edit.html", {"title": "Edit", "slug": slug, "fixed_title": _fixed_title(slug)})


@app.get("/-/history/{slug:path}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def history(request: Request, slug: str) -> HTMLResponse:
    slug = slug.lower()
    title = _fixed_title(slug)
    if title is None:
        page = await _page(slug)
        title = page["title"] if page else slug
    return _render(request, "wiki/tool.html", {"title": f"History: {title}", "tool": "history",
                                               "slug": slug, "page_title": title,
                                               "page_url": page_url(slug)})


@app.get("/{slug}", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def article(request: Request, slug: str) -> Response:
    if slug != slug.lower():
        return RedirectResponse("/" + slug.lower(), status_code=301)
    page = await _page(slug)
    if page is None or page.get("deleted"):
        return _render(request, "wiki/missing.html",
                       {"title": slug.replace("-", " ").capitalize(), "slug": slug,
                        "deleted": bool(page and page.get("deleted")),
                        "rev": page["rev"] if page else 0}, 404)
    return _render(request, "wiki/article.html", {
        "title": page["title"], "slug": slug, "page": page, "rev": page["rev"],
        "description": _excerpt(page["body"]),
    })
