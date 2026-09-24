"""The wiki host (``settings.wiki_url``), served by the website container.

A separate app mounted by hostname in ``app/web/main.py``, so none of the main
site's routes answer here and none of these answer on the main site. Like the
rest of this container it holds no database: page text comes from the API's
``/site/wiki/*`` over ``internal_get``, and class data is read from the game-data
files (``app/site/classes_page.py`` builds the detail model).

URL map:
  /                      home
  /classes, /class/<n>   generated class pages + their editable write-ups
  /<plural>, /<kind>/<n> generated game-data pages + write-ups, one pair per kind in
                         app/wiki/entities.KINDS: allies, mounts, dragons, wings, boats, sails,
                         auras, Mag Riders, flasks, tomes, fishing poles, costumes, bomb skins, style slots
                         (/styles, /style/<slot>), bosses, NPC groups (/npcs, /npc/<group>),
                         placeable groups (/placeables, /placeable/<group>),
                         companions, fish, mementos, badges, and crafting stations (/recipes, /station/<n>)
  /delve-modifiers       generated data page (app/wiki/data_pages.py) + its write-up
  /gems                  the How Gems Work guide (moved from the main site) + its write-up
  /stat-modifiers        how the game combines stat modifiers (docs/stat-modifiers.md) + its write-up
  /titles                every title in the game's title text table + its write-up
  /pvp-stats             the PvP stat curves (pvp.json, docs/pvp-stats.md) + its write-up
  /<slug>                an article
  /-/...                 tools: edit, history, search, recent, pages, suggestions
"""
import logging
from pathlib import Path
from urllib.parse import quote, urlencode

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
from app.trove.decode import store as gamedata
from app.web import feature_flags as web_flags
from app.wiki import data_pages, entities, media, pvp_stats

logger = logging.getLogger("kiwi.web.wiki")

WIKI = settings.wiki_url.rstrip("/")
WIKI_HOST = WIKI.split("://", 1)[-1].split("/", 1)[0].lower()


def _browse(path: str) -> list[dict]:
    """The header's Browse menu: classes, then each generated kind."""
    rows = [{"url": "/classes", "title": "Classes", "icon": "fa-shield-halved",
             "current": path == "/classes" or path.startswith("/class/")}]
    for kind, spec in entities.KINDS.items():
        rows.append({"url": f"/{spec['plural']}", "title": spec["title"], "icon": spec["icon"],
                     "current": path == f"/{spec['plural']}" or path.startswith(f"/{kind}/")})
    rows.append({"url": "/titles", "title": "Titles", "icon": "fa-signature", "current": path == "/titles"})
    return rows


def _context(request: Request) -> dict:
    return {"wiki_url": WIKI, "site_url": settings.app_url.rstrip("/"),
            "path": request.url.path, "browse": _browse(request.url.path),
            "generated": " ".join(["classes", *entities.KINDS, *data_pages.DATA_PAGES])}


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

# The site CSP, plus framing the main site's 3D/VFX viewer (/embed/viewer), which
# ally, mount and ability previews load inline when a reader asks for one.
WIKI_CSP = SITE_CSP.replace("frame-src ", f"frame-src {settings.app_url.rstrip('/')} ", 1)


@app.middleware("http")
async def _headers(request: Request, call_next):
    # Set before the outer security layer, which only fills what is missing.
    response = await call_next(request)
    response.headers.setdefault("Content-Security-Policy", WIKI_CSP)
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


def _delve_modifiers() -> dict:
    """app/trove/decode/delve_modifiers.py output."""
    return gamedata.load("delve_modifiers.json")


def _data_cards() -> list[dict]:
    """Search entries for the data pages; a modifier's name finds its page."""
    names = " ".join(m["name"] for m in _delve_modifiers()["modifiers"])
    return [{"name": data_pages.DATA_PAGES["delve-modifiers"], "url": "/delve-modifiers",
             "kind": "Game data", "terms": names},
            {"name": data_pages.DATA_PAGES["gems"], "url": "/gems", "kind": "Guide",
             "terms": "gems gem guide tiers radiant stellar crystal mystic lesser empowered cosmic "
                      "light power rank leveling focus boosters converters class gems builds"},
            {"name": data_pages.DATA_PAGES["stat-modifiers"], "url": "/stat-modifiers", "kind": "Guide",
             "terms": "stats stat modifiers bonus bonuses percent multiplier add multiplysum multiply set "
                      "nullify minimum maximum order character sheet critical damage health"},
            {"name": data_pages.DATA_PAGES["titles"], "url": "/titles", "kind": "Game data",
             "terms": "titles title prefix suffix name " + " ".join(t["name"] for t in entities.titles_page()["titles"])},
            {"name": data_pages.DATA_PAGES["pvp-stats"], "url": "/pvp-stats", "kind": "Game data",
             "terms": "pvp battle royale bloodstone arena stats stat curve cap soft cap diminishing returns "
                      + " ".join(name for _, name, _ in pvp_stats.SHEET)}]


def _entity_cards() -> list[dict]:
    """Search entries for every ally and mount page."""
    return [{"name": e["name"], "url": f"/{kind}/{entities.url_slug(e['slug'])}",
             "kind": kind.capitalize()}
            for kind in entities.KINDS for e in entities.entries(kind)]


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
    urls = [WIKI + "/", WIKI + "/classes"] + [f"{WIKI}/{spec['plural']}" for spec in entities.KINDS.values()]
    urls += [WIKI + c["url"] for c in _class_cards() + _data_cards() + _entity_cards()]
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
    kinds = [{"url": f"/{spec['plural']}", "title": spec["title"], "icon": spec["icon"],
              "count": len(entities.entries(kind))} for kind, spec in entities.KINDS.items()]
    return _render(request, "wiki/home.html", {"title": "Kiwi Wiki", "classes": _class_cards(),
                                               "data_pages": _data_cards(), "kinds": kinds})


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


async def _entity_index(request: Request, kind: str) -> HTMLResponse:
    spec = entities.KINDS[kind]
    found = await media.blueprints(kind)
    rows = sorted(({**entities.summary(kind, e), "thumb": media.thumb_url(e, media.lookup(kind, e, found), 64)}
                   for e in entities.entries(kind)), key=lambda r: r["name"].lower())
    with_abilities = sum(1 for r in rows if r["abilities"])
    return _render(request, "wiki/entity_index.html", {
        "title": spec["title"], "kind": kind, "rows": rows, "noun": spec.get("noun", spec["title"].lower()),
        "lead": spec["lead"].format(n=with_abilities), "with_abilities": with_abilities,
        "facets": entities.facets(kind, rows), "groups": entities.groups(kind, rows),
        "extra": entities.index_extra(kind),
        "description": spec["lead"].format(n=with_abilities),
    })


async def _entity_page(request: Request, kind: str, name: str) -> Response:
    e = entities.find(kind, name)
    if e is None and kind == "mount" and entities.find("dragon", name):
        return RedirectResponse(entities.url("dragon", entities.find("dragon", name) or {}), status_code=301)
    if e is None:
        raise HTTPException(status_code=404)
    canonical = entities.url_slug(e["slug"])
    if name != canonical:
        return RedirectResponse(f"/{kind}/{canonical}", status_code=301)
    slug = entities.storage_slug(kind, e)
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    d = entities.detail(kind, e)
    found = media.lookup(kind, e, await media.blueprints(kind))
    # A group page lists many models; none of them is the page's own.
    d["image"] = media.thumb_url(e, found, 256) if kind not in ("style", "npc", "placeable") else ""
    d["preview"] = media.preview_url(e, found) if kind not in ("style", "npc", "placeable") else ""
    for row in d.get("ranks") or []:
        row["image"] = media.render_url(row.get("blueprint", ""), 64)
    for row in d.get("sizes") or []:
        row["image"] = media.render_url(row.get("trophy_blueprint", ""), 64)
    for section in d.get("npc_sections") or []:
        for row in section["npcs"]:
            row["image"] = media.render_url(row.get("blueprint", ""), 64, row.get("prefab", ""))
    for group in d.get("style_groups") or []:
        for row in group["styles"]:
            row["image"] = media.render_url(row.get("blueprint", ""), 64) if row.get("blueprint") else ""
    if d.get("dressing_room"):
        d["dressing_room_url"] = (f"{settings.app_url.rstrip('/')}/dressing-room?"
                                  f"{urlencode(d['dressing_room'])}")
    return _render(request, "wiki/entity.html", {
        "title": e["name"], "kind": kind, "index": entities.KINDS[kind], "slug": slug, "d": d,
        "page": live, "rev": page["rev"] if page else 0,
        "description": _excerpt(live["body"]) if live and live.get("body")
        else (d["description"] or f"{e['name']} in Trove, read from the game files."),
    })


def _entity_routes(kind: str) -> None:
    """``/<plural>`` and ``/<kind>/<name>``, registered ahead of the article catch-all."""
    async def index(request: Request) -> HTMLResponse:
        return await _entity_index(request, kind)

    async def page(request: Request, name: str) -> Response:
        return await _entity_page(request, kind, name)

    opts = {"response_class": HTMLResponse, "dependencies": [Depends(_gate)], "methods": ["GET"]}
    app.add_api_route(f"/{entities.KINDS[kind]['plural']}", index, name=f"{kind}_index", **opts)
    app.add_api_route(f"/{kind}/{{name}}", page, name=f"{kind}_page", **opts)


for _kind in entities.KINDS:
    _entity_routes(_kind)


@app.get("/delve-modifiers", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def delve_modifiers(request: Request) -> HTMLResponse:
    slug = "data/delve-modifiers"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    data = _delve_modifiers()
    groups = {c: [m for m in data["modifiers"] if m["category"] == c]
              for c in ("creature", "lair", "path", "player", "tier", "new")}
    return _render(request, "wiki/delve_modifiers.html", {
        "title": data_pages.DATA_PAGES["delve-modifiers"],
        "slug": slug,
        "groups": groups,
        "data": data,
        "page": live,
        "rev": page["rev"] if page else 0,
        "description": "Every Trove delve modifier: what creature, lair and path modifiers do, "
                       "with numbers read from the game files.",
    })


@app.get("/gems", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def gems(request: Request) -> HTMLResponse:
    """The How Gems Work guide, client-rendered by /static/gems-guide.js."""
    if not (await web_flags._fetch()).get("gems_guide_enabled", True):
        raise HTTPException(status_code=404)
    slug = "data/gems"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    return _render(request, "wiki/gems.html", {
        "title": data_pages.DATA_PAGES["gems"],
        "slug": slug,
        "page": live,
        "rev": page["rev"] if page else 0,
        "description": "How gems work in Trove: tiers, elements, Lesser and Empowered, stat rolls, "
                       "leveling and Power Rank, focusing, boosters and build codes.",
    })


@app.get("/stat-modifiers", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def stat_modifiers(request: Request) -> HTMLResponse:
    slug = "data/stat-modifiers"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    return _render(request, "wiki/stat_modifiers.html", {
        "title": data_pages.DATA_PAGES["stat-modifiers"],
        "slug": slug,
        "page": live,
        "rev": page["rev"] if page else 0,
        "description": "Why two \"+30%\" bonuses can be worth wildly different amounts: how Trove "
                       "stacks every bonus on your character sheet, and in what order.",
    })


@app.get("/pvp-stats", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def pvp_stats_page(request: Request) -> HTMLResponse:
    slug = "data/pvp-stats"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    return _render(request, "wiki/pvp_stats.html", {
        "title": data_pages.DATA_PAGES["pvp-stats"],
        "slug": slug,
        "page": live,
        "rev": page["rev"] if page else 0,
        **pvp_stats.page(gamedata.load("pvp.json", {"modes": {}})),
        "description": "How Trove turns your character sheet into PvP, Battle Royale and Bloodstone "
                       "stats: the minimums, the slowdown and the caps for every stat.",
    })


@app.get("/titles", response_class=HTMLResponse, dependencies=[Depends(_gate)])
async def titles(request: Request) -> HTMLResponse:
    slug = "data/titles"
    page = await _page(slug)
    live = page if page and not page.get("deleted") else None
    return _render(request, "wiki/titles.html", {
        "title": data_pages.DATA_PAGES["titles"], "slug": slug, "page": live,
        "rev": page["rev"] if page else 0, **entities.titles_page(),
        "description": "Every Trove title the game files hold, with how each is earned.",
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
        "classes": [{"name": c["name"], "url": c["url"]} for c in _class_cards()] + _data_cards()
        + _entity_cards() if tool == "search" else None,
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
    if slug.startswith("data/"):
        title = data_pages.title_for(slug)
        if title is None:
            raise HTTPException(status_code=404)
        return title
    kind, _, name = slug.partition("/")
    if kind in entities.KINDS:
        e = entities.find(kind, name)
        if e is None:
            raise HTTPException(status_code=404)
        return e["name"]
    return None


def page_url(slug: str) -> str:
    """Where a stored slug is read: class/<tech> lives at /class/<display name>."""
    if slug.startswith("class/"):
        c = trove_stats.class_by_tech_name(slug[6:])
        return f"/class/{_class_slug(c)}" if c else "/classes"
    kind, _, name = slug.partition("/")
    if kind in entities.KINDS:
        return f"/{kind}/{entities.url_slug(name)}"
    return "/" + slug.removeprefix("data/")


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
