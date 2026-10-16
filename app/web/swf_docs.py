"""The decompiled Trove Flash UI reference: one crawlable URL per doc.

The viewer (``site/static/swf-docs.js``) renders the same markdown client-side;
this gives each doc its own ``/swf-docs/<slug>`` page with the content in the
HTML, so search engines see 110 pages instead of one shell behind ``#hash``
links. Docs are static files under ``site/static/swf-docs/``; only slugs listed
in its ``index.json`` (plus the README overview) are served.
"""
import json
import re
from functools import lru_cache
from pathlib import Path

from markdown_it import MarkdownIt

from app.core.config import settings

DOCS_DIR = Path(__file__).resolve().parents[2] / "site" / "static" / "swf-docs"
OVERVIEW = "README"
_TITLE = "Trove UI (Flash) Reference"
_DESC = ("Decompiled reference for Trove's Flash UI (.swf) files: document classes, "
         "game-logic classes, and timeline symbols for every screen.")

_MD = MarkdownIt("commonmark", {"html": False}).enable("table")
# Sibling-doc links in the markdown (`./atlas.md`, `atlas.md#x`) -> real page paths.
_DOC_LINK = re.compile(r'href="(?:\./)?([A-Za-z0-9_.-]+)\.md(#[^"]*)?"')


def _path(slug: str) -> str:
    return "/swf-docs" if slug == OVERVIEW else f"/swf-docs/{slug}"


@lru_cache(maxsize=1)
def _manifest(mtime: float) -> tuple[list[dict], dict[str, dict]]:
    data = json.loads((DOCS_DIR / "index.json").read_text(encoding="utf-8"))
    groups, by_slug = [], {}
    for g in data.get("groups") or []:
        items = []
        for it in g.get("items") or []:
            file = it.get("file") or ""
            if not file.endswith(".md"):
                continue
            slug = file[:-3]
            entry = {**it, "slug": slug, "url": _path(slug),
                     "label": (it.get("swf") or file).removesuffix(".swf")}
            items.append(entry)
            by_slug[slug.lower()] = entry
        groups.append({"title": g.get("title") or "", "shared": g.get("kind") == "shared", "docs": items})
    return groups, by_slug


def manifest() -> tuple[list[dict], dict[str, dict]]:
    """(groups for the sidebar, slug.lower() -> doc entry). Re-read when index.json changes."""
    return _manifest((DOCS_DIR / "index.json").stat().st_mtime)


@lru_cache(maxsize=256)
def _render(slug: str, mtime: float) -> str:
    html = _MD.render((DOCS_DIR / f"{slug}.md").read_text(encoding="utf-8"))
    return _DOC_LINK.sub(lambda m: f'href="{_path(m.group(1))}{m.group(2) or ""}"', html)


def doc_paths() -> list[str]:
    """Every doc page path for the sitemap (the overview is listed on its own)."""
    return [e["url"] for g in manifest()[0] for e in g["docs"]]


def page_context(slug: str | None) -> dict | None:
    """Template context for ``/swf-docs`` (``slug`` None) or ``/swf-docs/<slug>``;
    None when the slug isn't a published doc."""
    groups, by_slug = manifest()
    if slug is None:
        entry, real = None, OVERVIEW
    else:
        entry = by_slug.get(slug.lower())
        if entry is None:
            return None
        real = entry["slug"]
    md = DOCS_DIR / f"{real}.md"
    if not md.is_file():
        return None
    base = settings.app_url.rstrip("/")
    url = base + _path(real)
    if entry:
        cls = entry.get("mainClass")
        name = entry.get("swf") or real
        title = f"{name} ({cls}) - {_TITLE}" if cls and cls != "—" else f"{name} - {_TITLE}"
        desc = (entry.get("summary") or _DESC)[:300]
    else:
        title, desc = f"{_TITLE} - every Trove .swf screen decompiled", _DESC
    crumbs = [{"@type": "ListItem", "position": 1, "name": _TITLE, "item": f"{base}/swf-docs"}]
    if entry:
        crumbs.append({"@type": "ListItem", "position": 2, "name": entry.get("swf") or real, "item": url})
    ld = {"@context": "https://schema.org", "@graph": [
        {"@type": "TechArticle", "headline": title, "description": desc, "url": url,
         "inLanguage": "en", "isPartOf": {"@id": f"{base}/#website"}},
        {"@type": "BreadcrumbList", "itemListElement": crumbs},
    ]}
    return {
        "doc_slug": real, "doc_file": f"{real}.md", "doc_title": title, "doc_desc": desc,
        "doc_url": url, "doc_html": _render(real, md.stat().st_mtime), "doc_groups": groups,
        "doc_ld": json.dumps(ld, ensure_ascii=False).replace("<", "\\u003c"),
        "doc_count": sum(len(g["docs"]) for g in groups),
    }
