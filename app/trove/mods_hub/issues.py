"""Issues & requests on a mod, and the notifications they raise.

Players file a bug or a request on the mod page; the creator replies and closes
it. The thread is one ordered timeline (``ModIssueEvent``) mixing replies with
the record of each close/reopen, so the conversation reads the way GitHub's does.

Two rules shape everything here:

* **Per-mod consent.** Issues are the creator's to accept - ``issues_enabled``
  on the project. Off means the section is gone and every write 404s, so a
  disabled mod is indistinguishable from one that never had the feature.
* **Notifications are derived, not stored.** There is no per-user notification
  collection: an issue carries ``participant_ids`` (owner + collaborators +
  author + everyone who replied) and ``last_activity_at``, and the navbar feed is
  one indexed query over those against the reader's ``mod_issues_seen_at``
  watermark. Nothing is written per recipient, and nothing has to be cleaned up.
"""

from __future__ import annotations

import logging

from beanie import PydanticObjectId
from pymongo import ReturnDocument

from app.admin import runtime_config
from app.core.errors import APIError, ErrorCode
from app.core.ratelimit import check_rate_limit
from app.core.utils import iso as _iso
from app.core.utils import to_oid, utcnow
from app.site_auth.models import SiteUser
from app.trove.mods_hub import service
from app.trove.mods_hub.models import (
    IssueKind,
    ModIssue,
    ModIssueEvent,
    ModProject,
)

logger = logging.getLogger("kiwi.mods_hub")

TITLE_MAX = 140
BODY_MAX = 8000
COMMENT_MAX = 8000
# How many issues one account may have open on one mod at a time. A person with a
# real list of bugs files them; a person with 30 open threads is flooding.
MAX_OPEN_PER_AUTHOR = 10
NOTIFICATION_LIMIT = 20


def _disabled() -> APIError:
    # 404, not 403: a mod with issues off should look like a mod that never had
    # them, the same way a disabled site feature does.
    return APIError(404, ErrorCode.not_found, "This mod isn't taking issues or requests.")


def ensure_issues_open(project: ModProject) -> None:
    if not project.issues_enabled:
        raise _disabled()


def _clean(text: str, *, limit: int, field: str, required: bool = True) -> str:
    value = (text or "").strip()
    if required and not value:
        raise APIError(400, ErrorCode.bad_request, f"{field} can't be empty.")
    if len(value) > limit:
        raise APIError(400, ErrorCode.bad_request,
                       f"{field} is too long (max {limit} characters).")
    return value


async def _limit(actor: SiteUser, bucket: str) -> None:
    """Per-account write budget, master-tunable. Issues are the one place on the
    hub where a stranger writes onto someone else's page, so the flood guard is
    per-user rather than per-IP: an account is what the mod's owner can block by
    turning the feature off, and it survives a changing address."""
    max_, window = await runtime_config.get_rate_limit(f"mod_issue_{bucket}")
    await check_rate_limit(f"mod_issue_{bucket}:{actor.id}", max_, window)


def _viewer_ids(project: ModProject) -> list[PydanticObjectId]:
    """Everyone with edit rights on the mod - they hear about every issue on it."""
    ids = [c.user_id for c in (project.collaborators or [])]
    if project.owner_id:
        ids.append(project.owner_id)
    return ids


def can_moderate(project: ModProject, actor: SiteUser | None) -> bool:
    return service.can_edit(project, actor)


def issue_dto(issue: ModIssue, viewer: SiteUser | None = None,
              project: ModProject | None = None) -> dict:
    return {
        "number": issue.number,
        "kind": issue.kind,
        "title": issue.title,
        "body": issue.body,
        "status": issue.status,
        "author": issue.author_username,
        "author_id": str(issue.author_id),
        "is_author": viewer is not None and viewer.id == issue.author_id,
        "can_moderate": project is not None and can_moderate(project, viewer),
        "comment_count": issue.comment_count,
        "created_at": _iso(issue.created_at),
        "updated_at": _iso(issue.updated_at),
        "last_activity_at": _iso(issue.last_activity_at),
        "closed_at": _iso(issue.closed_at),
    }


def _event_dto(event: ModIssueEvent, project: ModProject | None = None) -> dict:
    return {
        "id": str(event.id),
        "kind": event.kind,
        "body": event.body,
        "author": event.author_username,
        "author_id": str(event.author_id),
        # Marks the creator's own replies in the thread, which is the answer
        # everyone opening an issue is actually looking for.
        "by_owner": project is not None and (
            event.author_id == project.owner_id
            or event.author_id in {c.user_id for c in (project.collaborators or [])}),
        "created_at": _iso(event.created_at),
    }


# --- reads -----------------------------------------------------------------

async def list_issues(
    project: ModProject, viewer: SiteUser | None, *,
    status: str = "open", limit: int = 30, offset: int = 0,
) -> dict:
    """A page of the mod's issues, newest activity first. ``status`` is
    ``open``/``closed``/``all``."""
    ensure_issues_open(project)
    query: dict = {"project_id": project.id}
    if status in ("open", "closed"):
        query["status"] = status
    total = await ModIssue.find(query).count()
    rows = await (ModIssue.find(query)
                  .sort("-last_activity_at")
                  .skip(offset).limit(limit).to_list())
    open_count = await ModIssue.find(
        {"project_id": project.id, "status": "open"}).count()
    return {
        "items": [issue_dto(i, viewer, project) for i in rows],
        "count": len(rows),
        "total": total,
        "open_count": open_count,
        "closed_count": total - open_count if status == "all" else None,
        "can_moderate": can_moderate(project, viewer),
    }


async def _get(project: ModProject, number: int) -> ModIssue:
    issue = await ModIssue.find_one({"project_id": project.id, "number": number})
    if issue is None:
        raise APIError(404, ErrorCode.not_found, "No such issue on this mod.")
    return issue


async def get_issue(project: ModProject, number: int, viewer: SiteUser | None) -> dict:
    ensure_issues_open(project)
    issue = await _get(project, number)
    events = await (ModIssueEvent.find({"issue_id": issue.id})
                    .sort("created_at").to_list())
    return {
        **issue_dto(issue, viewer, project),
        "events": [_event_dto(e, project) for e in events],
    }


# --- writes ----------------------------------------------------------------

async def _next_number(project: ModProject) -> int:
    """Hand out the next per-mod issue number atomically. Two people filing at the
    same moment must not both get #7 - the unique (project, number) index would
    reject the loser, so the counter is incremented in the database, not here."""
    doc = await ModProject.get_pymongo_collection().find_one_and_update(
        {"_id": project.id}, {"$inc": {"issue_seq": 1}},
        projection={"issue_seq": 1}, return_document=ReturnDocument.AFTER,
    )
    number = int((doc or {}).get("issue_seq") or 1)
    project.issue_seq = number      # keep the in-memory copy off the stale value
    return number


async def create_issue(
    project: ModProject, actor: SiteUser, *,
    kind: IssueKind = "issue", title: str, body: str = "",
) -> dict:
    ensure_issues_open(project)
    title = _clean(title, limit=TITLE_MAX, field="Title")
    body = _clean(body, limit=BODY_MAX, field="Description", required=False)
    if kind not in ("issue", "request"):
        raise APIError(400, ErrorCode.bad_request, "kind must be 'issue' or 'request'.")
    open_by_author = await ModIssue.find({
        "project_id": project.id, "author_id": actor.id, "status": "open",
    }).count()
    if open_by_author >= MAX_OPEN_PER_AUTHOR:
        raise APIError(429, ErrorCode.rate_limited,
                       f"You already have {MAX_OPEN_PER_AUTHOR} open threads on this mod. "
                       "Close one before opening another.")
    await _limit(actor, "create")
    now = utcnow()
    issue = ModIssue(
        project_id=project.id,
        project_slug=project.slug,
        project_handle=project.owner_handle,
        project_title=project.title,
        number=await _next_number(project),
        kind=kind, title=title, body=body,
        author_id=actor.id,
        author_username=actor.display_name or actor.username,
        participant_ids=list({*_viewer_ids(project), actor.id}),
        created_at=now, updated_at=now,
        last_activity_at=now, last_activity_by=actor.id,
    )
    await issue.insert()
    await _sync_open_count(project)
    return issue_dto(issue, actor, project)


async def add_comment(
    project: ModProject, number: int, actor: SiteUser, body: str,
) -> dict:
    """Reply on an issue. Closed issues still take replies - a fix that turns out
    not to work belongs on the thread that described it."""
    ensure_issues_open(project)
    issue = await _get(project, number)
    body = _clean(body, limit=COMMENT_MAX, field="Reply")
    await _limit(actor, "comment")
    event = ModIssueEvent(
        issue_id=issue.id, project_id=project.id, kind="comment", body=body,
        author_id=actor.id, author_username=actor.display_name or actor.username,
    )
    await event.insert()
    issue.comment_count += 1
    await _touch(issue, actor, {"$inc": {"comment_count": 1}})
    return _event_dto(event, project)


async def _touch(issue: ModIssue, actor: SiteUser, extra: dict | None = None) -> None:
    """Stamp the activity that drives the notification feed, and enrol the actor
    as a participant so they hear the rest of the conversation.

    A field update, not ``issue.save()``: two people replying to the same thread
    at once would otherwise write back each other's document and one reply's
    count - or worse, one participant - would simply vanish."""
    now = utcnow()
    issue.updated_at = now
    issue.last_activity_at = now
    issue.last_activity_by = actor.id
    if actor.id not in issue.participant_ids:
        issue.participant_ids.append(actor.id)
    update = {
        "$set": {"updated_at": now, "last_activity_at": now,
                 "last_activity_by": actor.id},
        "$addToSet": {"participant_ids": actor.id},
    }
    for op, fields in (extra or {}).items():
        update.setdefault(op, {}).update(fields)
    await ModIssue.get_pymongo_collection().update_one({"_id": issue.id}, update)


async def set_status(
    project: ModProject, number: int, actor: SiteUser, status: str,
    comment: str = "",
) -> dict:
    """Close or reopen. The creator can do either; the author can close and reopen
    their own thread, which is how "never mind, fixed itself" resolves without
    waiting on anyone."""
    ensure_issues_open(project)
    if status not in ("open", "closed"):
        raise APIError(400, ErrorCode.bad_request, "status must be 'open' or 'closed'.")
    issue = await _get(project, number)
    if not can_moderate(project, actor) and actor.id != issue.author_id:
        raise APIError(403, ErrorCode.forbidden,
                       "Only the mod's creator or the person who opened this can close it.")
    if issue.status == status:
        return issue_dto(issue, actor, project)
    comment = _clean(comment, limit=COMMENT_MAX, field="Reply", required=False)
    await _limit(actor, "comment")
    if comment:
        reply = ModIssueEvent(
            issue_id=issue.id, project_id=project.id, kind="comment", body=comment,
            author_id=actor.id, author_username=actor.display_name or actor.username,
        )
        await reply.insert()
        issue.comment_count += 1
    await ModIssueEvent(
        issue_id=issue.id, project_id=project.id,
        kind="closed" if status == "closed" else "reopened",
        author_id=actor.id, author_username=actor.display_name or actor.username,
    ).insert()
    issue.status = status  # type: ignore[assignment]
    issue.closed_at = utcnow() if status == "closed" else None
    issue.closed_by_id = actor.id if status == "closed" else None
    await _touch(issue, actor, {"$set": {
        "status": issue.status, "closed_at": issue.closed_at,
        "closed_by_id": issue.closed_by_id,
    }, **({"$inc": {"comment_count": 1}} if comment else {})})
    await _sync_open_count(project)
    return issue_dto(issue, actor, project)


async def delete_issue(project: ModProject, number: int, actor: SiteUser) -> None:
    """Remove a thread entirely - the creator's spam broom, and the author's way to
    take back something they shouldn't have posted."""
    issue = await _get(project, number)
    if not can_moderate(project, actor) and actor.id != issue.author_id:
        raise APIError(403, ErrorCode.forbidden, "You can't delete this thread.")
    await ModIssueEvent.find({"issue_id": issue.id}).delete()
    await issue.delete()
    await _sync_open_count(project)


async def delete_comment(
    project: ModProject, number: int, event_id: str, actor: SiteUser,
) -> None:
    issue = await _get(project, number)
    event = await ModIssueEvent.get(to_oid(event_id, "event id"))
    if event is None or event.issue_id != issue.id or event.kind != "comment":
        raise APIError(404, ErrorCode.not_found, "No such reply on this issue.")
    if not can_moderate(project, actor) and actor.id != event.author_id:
        raise APIError(403, ErrorCode.forbidden, "You can't delete this reply.")
    await event.delete()
    issue.comment_count = max(0, issue.comment_count - 1)
    await ModIssue.get_pymongo_collection().update_one(
        {"_id": issue.id}, {"$set": {"comment_count": issue.comment_count}})


async def _sync_open_count(project: ModProject) -> None:
    """Keep the denormalized badge count honest. Recounted rather than ±1'd: the
    count is read on every mod-page load and drifting it is worse than the query."""
    project.open_issue_count = await ModIssue.find(
        {"project_id": project.id, "status": "open"}).count()
    # A keyed $set, NOT project.save(): the project document also carries
    # `issue_seq`, which was just incremented in the database, and saving a copy
    # read before that would hand the next filing a number already taken.
    await ModProject.get_pymongo_collection().update_one(
        {"_id": project.id}, {"$set": {"open_issue_count": project.open_issue_count}})


# --- notifications ---------------------------------------------------------

async def notifications(user: SiteUser, *, limit: int = NOTIFICATION_LIMIT) -> dict:
    """Everything the reader takes part in that has moved since they last looked.

    "Takes part in" = they own the mod, collaborate on it, opened the thread, or
    replied to it. Activity the reader caused themselves is skipped - you don't
    need telling about your own reply."""
    seen = user.mod_issues_seen_at
    rows = await (ModIssue.find({
        "participant_ids": user.id,
        "last_activity_by": {"$ne": user.id},
    }).sort("-last_activity_at").limit(limit).to_list())
    items = []
    unread = 0
    for issue in rows:
        is_new = seen is None or issue.last_activity_at > seen
        unread += 1 if is_new else 0
        items.append({
            "number": issue.number,
            "kind": issue.kind,
            "title": issue.title,
            "status": issue.status,
            "mod_title": issue.project_title,
            "handle": issue.project_handle,
            "slug": issue.project_slug,
            "url": f"/mods/{issue.project_handle}/{issue.project_slug}#issue-{issue.number}",
            "comment_count": issue.comment_count,
            "last_activity_at": _iso(issue.last_activity_at),
            "unread": is_new,
        })
    return {"items": items, "unread": unread, "seen_at": _iso(seen)}


async def mark_seen(user: SiteUser) -> dict:
    """Move the read watermark to now. One timestamp for the whole feed: opening
    the panel is the act of reading it, which is all the state this needs."""
    user.mod_issues_seen_at = utcnow()
    await SiteUser.get_pymongo_collection().update_one(
        {"_id": user.id}, {"$set": {"mod_issues_seen_at": user.mod_issues_seen_at}})
    return {"seen_at": _iso(user.mod_issues_seen_at)}


async def purge_project(project_id: PydanticObjectId) -> None:
    """Drop a deleted mod's threads (called from the project purge). Events carry
    their own ``project_id``, so neither pass has to walk the issues first."""
    await ModIssueEvent.find({"project_id": project_id}).delete()
    await ModIssue.find({"project_id": project_id}).delete()
