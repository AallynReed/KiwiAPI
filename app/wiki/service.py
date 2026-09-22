"""Wiki pages, revisions and suggestions.

Anyone may read. Only accounts the master has marked ``is_wiki_editor`` write;
every other signed-in account may *suggest* an edit, which an editor accepts
(it lands as a revision credited to the suggester) or rejects.

Every write is optimistic: the client sends the revision it started from
(``base_rev``) and the save is a compare-and-set on it, so two editors can't
silently overwrite each other - the second one gets a 409 and re-bases.

Slugs are either plain (``beginner-guide``) or namespaced to game data
(``class/<tech_name>``). A namespaced page's title is the entity's name and
can't be edited; its body is the write-up shown under the generated data.
"""
from __future__ import annotations

import re
import unicodedata

from beanie import PydanticObjectId
from pymongo import ReturnDocument
from pymongo.errors import DuplicateKeyError

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode
from app.core.ratelimit import check_rate_limit
from app.core.utils import iso, to_oid, utcnow
from app.site_auth.models import SiteUser
from app.trove import stats as trove_stats
from app.wiki.models import RevisionAction, WikiPage, WikiRevision, WikiSuggestion

SLUG_MAX = 100
TITLE_MAX = 120
BODY_MAX = 100_000
SUMMARY_MAX = 200
NOTE_MAX = 500
MAX_PENDING_PER_AUTHOR = 10

# Plain slugs the wiki host routes itself.
RESERVED = frozenset({"classes", "class", "static", "health"})
_PLAIN = re.compile(r"^[a-z0-9](?:[a-z0-9-]{0,98}[a-z0-9])?$")


def slugify(text: str) -> str:
    """"Beginner's Guide!" -> "beginners-guide". Same rule as wiki.js slugify."""
    text = unicodedata.normalize("NFKD", text or "").encode("ascii", "ignore").decode()
    text = re.sub(r"['’]", "", text.lower())
    return re.sub(r"[^a-z0-9]+", "-", text).strip("-")[:SLUG_MAX].strip("-")


def class_for(tech: str) -> dict | None:
    return trove_stats.class_by_tech_name(tech)


def _bad(message: str) -> APIError:
    return APIError(400, ErrorCode.bad_request, message)


def normalize_slug(raw: str) -> str:
    slug = (raw or "").strip().strip("/").lower()
    if slug.startswith("class/"):
        if class_for(slug[6:]) is None:
            raise _bad("There's no such class.")
        return slug
    if not _PLAIN.match(slug) or slug in RESERVED:
        raise _bad("That page address isn't allowed. Use letters, numbers and dashes.")
    return slug


def fixed_title(slug: str) -> str | None:
    """The title of a data-backed page, which editors can't change."""
    if slug.startswith("class/"):
        c = class_for(slug[6:])
        return (c or {}).get("name") or None
    return None


def _clean(text: str | None, *, limit: int, field: str, required: bool = False) -> str:
    value = (text or "").replace("\r\n", "\n").strip()
    if required and not value:
        raise _bad(f"{field} can't be empty.")
    if len(value) > limit:
        raise _bad(f"{field} is too long (max {limit:,} characters).")
    return value


def is_editor(user: SiteUser | None) -> bool:
    return bool(user and user.is_active and not user.is_deleted and user.is_wiki_editor)


def _require_editor(user: SiteUser | None) -> SiteUser:
    if not is_editor(user):
        raise APIError(403, ErrorCode.forbidden, "Only wiki editors can do that.")
    return user  # type: ignore[return-value]


async def _limit(user: SiteUser, bucket: str) -> None:
    max_, window = await runtime_config.get_rate_limit(f"wiki_{bucket}")
    await check_rate_limit(f"wiki_{bucket}:{user.id}", max_, window)


async def _names(ids) -> dict:
    wanted = list({i for i in ids if i})
    if not wanted:
        return {}
    users = await SiteUser.find({"_id": {"$in": wanted}}).to_list()
    return {u.id: u.username for u in users}


# ── reads ──────────────────────────────────────────────────────────────────

def _page_dto(p: WikiPage, names: dict) -> dict:
    return {
        "slug": p.slug, "title": p.title, "body": p.body, "rev": p.rev,
        "deleted": p.deleted,
        "created_at": iso(p.created_at), "updated_at": iso(p.updated_at),
        "updated_by": names.get(p.updated_by),
    }


def _rev_dto(r: WikiRevision, names: dict, *, body: bool = False) -> dict:
    out = {
        "slug": r.slug, "rev": r.rev, "title": r.title, "summary": r.summary,
        "action": r.action, "author": names.get(r.author_id),
        "approved_by": names.get(r.approved_by), "created_at": iso(r.created_at),
        "size": len(r.body),
    }
    if body:
        out["body"] = r.body
    return out


async def get_page(slug: str) -> dict | None:
    """The page, deleted ones included (the view shows them as gone)."""
    page = await WikiPage.find_one(WikiPage.slug == slug)
    if page is None:
        return None
    return _page_dto(page, await _names([page.updated_by]))


async def list_pages() -> list[dict]:
    pages = await WikiPage.find({"deleted": False}).sort("title").to_list()
    return [{"slug": p.slug, "title": p.title, "updated_at": iso(p.updated_at)} for p in pages]


async def history(slug: str, *, limit: int, offset: int) -> dict:
    q = WikiRevision.find(WikiRevision.slug == slug)
    total = await q.count()
    revs = await q.sort("-rev").skip(offset).limit(limit).to_list()
    names = await _names([r.author_id for r in revs] + [r.approved_by for r in revs])
    return {"slug": slug, "total": total, "items": [_rev_dto(r, names) for r in revs]}


async def revision(slug: str, rev: int) -> dict | None:
    r = await WikiRevision.find_one(WikiRevision.slug == slug, WikiRevision.rev == rev)
    if r is None:
        return None
    return _rev_dto(r, await _names([r.author_id, r.approved_by]), body=True)


async def recent(limit: int) -> list[dict]:
    revs = await WikiRevision.find_all().sort("-created_at").limit(limit).to_list()
    names = await _names([r.author_id for r in revs] + [r.approved_by for r in revs])
    return [_rev_dto(r, names) for r in revs]


async def search(q: str, limit: int) -> list[dict]:
    q = (q or "").strip()[:100]
    if not q:
        return []
    score = {"$meta": "textScore"}
    cursor = WikiPage.get_pymongo_collection().find(
        {"$text": {"$search": q}, "deleted": False},
        {"slug": 1, "title": 1, "body": 1, "score": score},
    ).sort([("score", score)]).limit(limit)
    found = await cursor.to_list(limit)
    seen = {d["slug"] for d in found}
    # Text search matches whole words only; a title prefix still finds "Knig…".
    rx = {"$regex": "^" + re.escape(q), "$options": "i"}
    for p in await WikiPage.find({"title": rx, "deleted": False}).limit(limit).to_list():
        if p.slug not in seen:
            found.append({"slug": p.slug, "title": p.title, "body": p.body})
    return [{"slug": d["slug"], "title": d["title"], "excerpt": _excerpt(d["body"], q)}
            for d in found[:limit]]


def _excerpt(body: str, q: str, width: int = 160) -> str:
    text = re.sub(r"[#>*_`\[\]|~]+", "", body)
    text = re.sub(r"\s+", " ", text).strip()
    i = text.lower().find(q.lower().split()[0]) if q.split() else -1
    start = max(0, i - width // 3) if i >= 0 else 0
    snippet = text[start:start + width]
    return ("…" if start else "") + snippet + ("…" if start + width < len(text) else "")


# ── writes ─────────────────────────────────────────────────────────────────

def _conflict() -> APIError:
    return APIError(409, ErrorCode.conflict,
                    "Someone changed this page while you were editing. Reload to see their version.")


async def _commit(slug: str, *, title: str, body: str, summary: str, base_rev: int,
                  author_id: PydanticObjectId | None, approved_by: PydanticObjectId | None = None,
                  action: RevisionAction = "edit", deleted: bool = False) -> WikiPage:
    """Compare-and-set the page on ``base_rev`` and append the revision."""
    now = utcnow()
    if base_rev == 0:
        page = WikiPage(slug=slug, title=title, body=body, rev=1, deleted=deleted,
                        created_at=now, updated_at=now, updated_by=author_id)
        try:
            await page.insert()
        except DuplicateKeyError:
            raise APIError(409, ErrorCode.conflict, "A page with that address already exists.") from None
    else:
        raw = await WikiPage.get_pymongo_collection().find_one_and_update(
            {"slug": slug, "rev": base_rev},
            {"$set": {"title": title, "body": body, "deleted": deleted,
                      "updated_at": now, "updated_by": author_id},
             "$inc": {"rev": 1}},
            return_document=ReturnDocument.AFTER,
        )
        if raw is None:
            raise _conflict()
        page = WikiPage.model_validate(raw)
    await WikiRevision(slug=slug, rev=page.rev, title=title, body=body, summary=summary,
                       action=action, author_id=author_id, approved_by=approved_by,
                       created_at=now).insert()
    return page


def _validated(slug: str, title: str | None, body: str | None, summary: str | None):
    slug = normalize_slug(slug)
    title = fixed_title(slug) or _clean(title, limit=TITLE_MAX, field="Title", required=True)
    body = _clean(body, limit=BODY_MAX, field="Page text")
    summary = _clean(summary, limit=SUMMARY_MAX, field="Summary")
    return slug, title, body, summary


async def _current(slug: str) -> WikiPage | None:
    return await WikiPage.find_one(WikiPage.slug == slug)


async def save(user: SiteUser | None, *, slug: str, title: str | None, body: str | None,
               summary: str | None, base_rev: int, suggestion_id: str | None = None) -> dict:
    """An editor's save. With ``suggestion_id`` it also closes that suggestion as
    accepted and credits its author - the editor may have adjusted it first."""
    editor = _require_editor(user)
    await _limit(editor, "edit")
    slug, title, body, summary = _validated(slug, title, body, summary)
    page = await _current(slug)
    if (page.rev if page else 0) != base_rev:
        raise _conflict()
    if page and not page.deleted and page.title == title and page.body == body:
        raise _bad("Nothing changed.")
    if not body and not fixed_title(slug):
        raise _bad("Page text can't be empty.")

    author, approved_by, sug = editor.id, None, None
    if suggestion_id:
        sug = await _pending(suggestion_id)
        if sug.slug != slug:
            raise _bad("That suggestion is for a different page.")
        author, approved_by = sug.author_id, editor.id
        summary = summary or sug.summary

    saved = await _commit(slug, title=title, body=body, summary=summary, base_rev=base_rev,
                          author_id=author, approved_by=approved_by)
    if sug is not None:
        await _resolve(sug, "accepted", editor)
    return _page_dto(saved, await _names([saved.updated_by]))


async def revert(user: SiteUser | None, *, slug: str, rev: int, base_rev: int) -> dict:
    editor = _require_editor(user)
    await _limit(editor, "edit")
    slug = normalize_slug(slug)
    old = await WikiRevision.find_one(WikiRevision.slug == slug, WikiRevision.rev == rev)
    if old is None:
        raise APIError(404, ErrorCode.not_found, "No such revision.")
    saved = await _commit(slug, title=fixed_title(slug) or old.title, body=old.body,
                          summary=f"Restored revision {rev}", base_rev=base_rev,
                          author_id=editor.id, action="revert",
                          deleted=old.action == "delete")
    return _page_dto(saved, await _names([saved.updated_by]))


async def delete(user: SiteUser | None, *, slug: str, reason: str | None, base_rev: int) -> dict:
    editor = _require_editor(user)
    await _limit(editor, "edit")
    slug = normalize_slug(slug)
    page = await _current(slug)
    if page is None or page.deleted:
        raise APIError(404, ErrorCode.not_found, "No such page.")
    saved = await _commit(slug, title=page.title, body=page.body, base_rev=base_rev,
                          summary=_clean(reason, limit=SUMMARY_MAX, field="Reason"),
                          author_id=editor.id, action="delete", deleted=True)
    return _page_dto(saved, {})


# ── suggestions ────────────────────────────────────────────────────────────

def _sug_dto(s: WikiSuggestion, names: dict, *, body: bool = False) -> dict:
    out = {
        "id": str(s.id), "slug": s.slug, "base_rev": s.base_rev, "title": s.title,
        "summary": s.summary, "status": s.status, "note": s.note,
        "author": names.get(s.author_id), "resolved_by": names.get(s.resolved_by),
        "created_at": iso(s.created_at), "resolved_at": iso(s.resolved_at),
    }
    if body:
        out["body"] = s.body
    return out


async def suggest(user: SiteUser, *, slug: str, title: str | None, body: str | None,
                  summary: str | None, base_rev: int) -> dict:
    await _limit(user, "suggest")
    slug, title, body, summary = _validated(slug, title, body, summary)
    page = await _current(slug)
    if (page.rev if page else 0) != base_rev:
        raise _conflict()
    if page and not page.deleted and page.title == title and page.body == body:
        raise _bad("Nothing changed.")
    if not body:
        raise _bad("Page text can't be empty.")
    pending = await WikiSuggestion.find({"author_id": user.id, "status": "pending"}).count()
    if pending >= MAX_PENDING_PER_AUTHOR:
        raise _bad(f"You already have {pending} suggestions waiting for review. "
                   "Wait for those, or withdraw one, before sending more.")
    assert user.id is not None
    sug = WikiSuggestion(slug=slug, base_rev=base_rev, title=title, body=body,
                         summary=summary, author_id=user.id)
    await sug.insert()
    return _sug_dto(sug, {user.id: user.username})


async def list_suggestions(user: SiteUser, *, mine: bool, status: str | None,
                           limit: int, offset: int) -> dict:
    query: dict = {}
    if mine or not is_editor(user):
        query["author_id"] = user.id
    if status:
        query["status"] = status
    q = WikiSuggestion.find(query)
    total = await q.count()
    items = await q.sort("-created_at").skip(offset).limit(limit).to_list()
    names = await _names([s.author_id for s in items] + [s.resolved_by for s in items])
    return {"total": total, "items": [_sug_dto(s, names) for s in items]}


async def get_suggestion(user: SiteUser, sid: str) -> dict:
    s = await _load(sid)
    if not is_editor(user) and s.author_id != user.id:
        raise APIError(404, ErrorCode.not_found, "No such suggestion.")
    page = await _current(s.slug)
    names = await _names([s.author_id, s.resolved_by])
    out = _sug_dto(s, names, body=True)
    # What it would replace, for the reviewer's diff.
    out["current"] = {"rev": page.rev, "title": page.title, "body": page.body,
                      "deleted": page.deleted} if page else None
    return out


async def pending_count() -> int:
    return await WikiSuggestion.find({"status": "pending"}).count()


async def _load(sid: str) -> WikiSuggestion:
    oid = to_oid(sid)
    s = await WikiSuggestion.get(oid) if oid else None
    if s is None:
        raise APIError(404, ErrorCode.not_found, "No such suggestion.")
    return s


async def _pending(sid: str) -> WikiSuggestion:
    s = await _load(sid)
    if s.status != "pending":
        raise _bad(f"That suggestion was already {s.status}.")
    return s


async def _resolve(s: WikiSuggestion, status: str, by: SiteUser | None, note: str = "") -> None:
    s.status = status  # type: ignore[assignment]
    s.note = note
    s.resolved_at = utcnow()
    s.resolved_by = by.id if by else None
    await s.save()


async def accept(user: SiteUser | None, sid: str) -> dict:
    """Apply as-is. If the page moved on since it was written the editor has to
    merge it by hand (open it in the editor), so nothing is lost silently."""
    editor = _require_editor(user)
    await _limit(editor, "edit")
    s = await _pending(sid)
    page = await _current(s.slug)
    if (page.rev if page else 0) != s.base_rev:
        raise APIError(409, ErrorCode.conflict,
                       "The page changed after this was suggested. Open it in the editor to merge.")
    saved = await _commit(s.slug, title=fixed_title(s.slug) or s.title, body=s.body,
                          summary=s.summary, base_rev=s.base_rev,
                          author_id=s.author_id, approved_by=editor.id)
    await _resolve(s, "accepted", editor)
    return _page_dto(saved, await _names([saved.updated_by]))


async def reject(user: SiteUser | None, sid: str, note: str | None) -> dict:
    editor = _require_editor(user)
    s = await _pending(sid)
    await _resolve(s, "rejected", editor, _clean(note, limit=NOTE_MAX, field="Reason"))
    return _sug_dto(s, await _names([s.author_id, s.resolved_by]))


async def withdraw(user: SiteUser, sid: str) -> dict:
    s = await _pending(sid)
    if s.author_id != user.id:
        raise APIError(404, ErrorCode.not_found, "No such suggestion.")
    await _resolve(s, "withdrawn", user)
    return _sug_dto(s, {user.id: user.username})
