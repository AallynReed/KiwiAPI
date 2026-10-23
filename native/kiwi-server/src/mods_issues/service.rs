//! `app/trove/mods_hub/issues.py`: issues and requests on a mod, and the
//! notification feed derived from them.

use chrono::{DateTime, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Map, Value, json};

use crate::{
    auth::{
        Rejection,
        models::{SITE_USERS, SiteUser},
    },
    error::{ApiError, ErrorCode},
    mods_write_auth::{
        docs::{date, int, iso, opt_date, opt_oid, read_date, read_ids, read_int, read_oid, read_opt_date, read_str},
        project::{self, PROJECTS, Project},
        store::{Res, Store},
    },
};

pub const ISSUES: &str = "mod_issues";
pub const EVENTS: &str = "mod_issue_events";

pub const TITLE_MAX: usize = 140;
pub const BODY_MAX: usize = 8000;
pub const COMMENT_MAX: usize = 8000;
pub const MAX_OPEN_PER_AUTHOR: u64 = 10;
pub const NOTIFICATION_LIMIT: i64 = 20;

type Out<T> = Result<T, Rejection>;

#[derive(Debug, Clone)]
pub struct Issue {
    pub id: ObjectId,
    pub project_id: ObjectId,
    pub project_slug: String,
    pub project_handle: String,
    pub project_title: String,
    pub number: i64,
    pub kind: String,
    pub title: String,
    pub body: String,
    pub author_id: ObjectId,
    pub author_username: String,
    pub status: String,
    pub closed_at: Option<DateTime<Utc>>,
    pub closed_by_id: Option<ObjectId>,
    pub comment_count: i64,
    pub participant_ids: Vec<ObjectId>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_activity_at: DateTime<Utc>,
    pub last_activity_by: Option<ObjectId>,
}

fn zero() -> ObjectId {
    ObjectId::from_bytes([0; 12])
}

impl Issue {
    pub fn from_doc(d: &Document) -> Self {
        Issue {
            id: read_oid(d, "_id").unwrap_or_else(zero),
            project_id: read_oid(d, "project_id").unwrap_or_else(zero),
            project_slug: read_str(d, "project_slug"),
            project_handle: read_str(d, "project_handle"),
            project_title: read_str(d, "project_title"),
            number: read_int(d, "number", 0),
            kind: d.get_str("kind").unwrap_or("issue").to_owned(),
            title: read_str(d, "title"),
            body: read_str(d, "body"),
            author_id: read_oid(d, "author_id").unwrap_or_else(zero),
            author_username: read_str(d, "author_username"),
            status: d.get_str("status").unwrap_or("open").to_owned(),
            closed_at: read_opt_date(d, "closed_at"),
            closed_by_id: read_oid(d, "closed_by_id"),
            comment_count: read_int(d, "comment_count", 0),
            participant_ids: read_ids(d, "participant_ids"),
            created_at: read_date(d, "created_at"),
            updated_at: read_date(d, "updated_at"),
            last_activity_at: read_date(d, "last_activity_at"),
            last_activity_by: read_oid(d, "last_activity_by"),
        }
    }

    fn fields(&self) -> Document {
        doc! {
            "project_id": self.project_id,
            "project_slug": &self.project_slug,
            "project_handle": &self.project_handle,
            "project_title": &self.project_title,
            "number": int(self.number),
            "kind": &self.kind,
            "title": &self.title,
            "body": &self.body,
            "author_id": self.author_id,
            "author_username": &self.author_username,
            "status": &self.status,
            "closed_at": opt_date(self.closed_at),
            "closed_by_id": opt_oid(self.closed_by_id),
            "comment_count": int(self.comment_count),
            "participant_ids": self.participant_ids.iter().map(|i| Bson::ObjectId(*i)).collect::<Vec<_>>(),
            "created_at": date(self.created_at),
            "updated_at": date(self.updated_at),
            "last_activity_at": date(self.last_activity_at),
            "last_activity_by": opt_oid(self.last_activity_by),
        }
    }
}

#[derive(Debug, Clone)]
pub struct Event {
    pub id: ObjectId,
    pub issue_id: ObjectId,
    pub project_id: ObjectId,
    pub kind: String,
    pub body: String,
    pub author_id: ObjectId,
    pub author_username: String,
    pub created_at: DateTime<Utc>,
}

impl Event {
    fn new(issue: &Issue, kind: &str, body: String, actor: &SiteUser, now: DateTime<Utc>) -> Self {
        Event {
            id: zero(),
            issue_id: issue.id,
            project_id: issue.project_id,
            kind: kind.into(),
            body,
            author_id: actor.id,
            author_username: display(actor),
            created_at: now,
        }
    }

    pub fn from_doc(d: &Document) -> Self {
        Event {
            id: read_oid(d, "_id").unwrap_or_else(zero),
            issue_id: read_oid(d, "issue_id").unwrap_or_else(zero),
            project_id: read_oid(d, "project_id").unwrap_or_else(zero),
            kind: d.get_str("kind").unwrap_or("comment").to_owned(),
            body: read_str(d, "body"),
            author_id: read_oid(d, "author_id").unwrap_or_else(zero),
            author_username: read_str(d, "author_username"),
            created_at: read_date(d, "created_at"),
        }
    }

    fn fields(&self) -> Document {
        doc! {
            "issue_id": self.issue_id,
            "project_id": self.project_id,
            "kind": &self.kind,
            "body": &self.body,
            "author_id": self.author_id,
            "author_username": &self.author_username,
            "created_at": date(self.created_at),
        }
    }

    async fn insert(&mut self, store: &Store) -> Res<()> {
        self.id = store.insert(EVENTS, self.fields()).await?;
        Ok(())
    }
}

/// `actor.display_name or actor.username`.
fn display(actor: &SiteUser) -> String {
    actor.display_name.clone().filter(|d| !d.is_empty()).unwrap_or_else(|| actor.username.clone())
}

fn disabled() -> ApiError {
    ApiError::not_found("This mod isn't taking issues or requests.")
}

pub fn ensure_issues_open(project: &Project) -> Res<()> {
    if project.issues_enabled { Ok(()) } else { Err(disabled()) }
}

/// `_clean(text, limit=, field=, required=)`.
fn clean(text: &str, limit: usize, field: &str, required: bool) -> Res<String> {
    let value = kiwi_core::pystr::strip(text);
    if required && value.is_empty() {
        return Err(ApiError::bad_request(format!("{field} can't be empty.")));
    }
    if value.chars().count() > limit {
        return Err(ApiError::bad_request(format!("{field} is too long (max {limit} characters).")));
    }
    Ok(value.to_owned())
}

/// `_limit(actor, bucket)`.
async fn limit(store: &Store, actor: &SiteUser, bucket: &str) -> Out<()> {
    store.rate_limit(&format!("mod_issue_{bucket}"), &format!("mod_issue_{bucket}:{}", actor.id.to_hex())).await
}

/// `_viewer_ids(project)`: collaborators, then the owner.
fn viewer_ids(project: &Project) -> Vec<ObjectId> {
    let mut ids = project.collaborators.clone();
    if let Some(o) = project.owner_id {
        ids.push(o);
    }
    ids
}

pub fn can_moderate(project: &Project, actor: Option<&SiteUser>) -> bool {
    project::can_edit(project, actor)
}

pub fn issue_dto(issue: &Issue, viewer: Option<&SiteUser>, project: Option<&Project>) -> Value {
    json!({
        "number": issue.number,
        "kind": issue.kind,
        "title": issue.title,
        "body": issue.body,
        "status": issue.status,
        "author": issue.author_username,
        "author_id": issue.author_id.to_hex(),
        "is_author": viewer.is_some_and(|v| v.id == issue.author_id),
        "can_moderate": project.is_some_and(|p| can_moderate(p, viewer)),
        "comment_count": issue.comment_count,
        "created_at": iso(Some(issue.created_at)),
        "updated_at": iso(Some(issue.updated_at)),
        "last_activity_at": iso(Some(issue.last_activity_at)),
        "closed_at": iso(issue.closed_at),
    })
}

fn event_dto(event: &Event, project: Option<&Project>) -> Value {
    let by_owner =
        project.is_some_and(|p| p.owner_id == Some(event.author_id) || p.collaborators.contains(&event.author_id));
    json!({
        "id": event.id.to_hex(),
        "kind": event.kind,
        "body": event.body,
        "author": event.author_username,
        "author_id": event.author_id.to_hex(),
        "by_owner": by_owner,
        "created_at": iso(Some(event.created_at)),
    })
}

/// `list_issues(project, viewer, status=, limit=, offset=)`.
pub async fn list_issues(
    store: &Store,
    project: &Project,
    viewer: Option<&SiteUser>,
    status: &str,
    limit: i64,
    offset: u64,
) -> Res<Value> {
    ensure_issues_open(project)?;
    let mut query = doc! {"project_id": project.id};
    if matches!(status, "open" | "closed") {
        query.insert("status", status);
    }
    let total = store.count(ISSUES, query.clone()).await?;
    let rows = store.find(ISSUES, query, Some(doc! {"last_activity_at": -1}), offset, limit).await?;
    let open_count = store.count(ISSUES, doc! {"project_id": project.id, "status": "open"}).await?;
    let items: Vec<Value> = rows.iter().map(|d| issue_dto(&Issue::from_doc(d), viewer, Some(project))).collect();
    let count = items.len();
    let closed = if status == "all" { json!(total as i64 - open_count as i64) } else { Value::Null };
    Ok(json!({
        "items": items,
        "count": count,
        "total": total,
        "open_count": open_count,
        "closed_count": closed,
        "can_moderate": can_moderate(project, viewer),
    }))
}

/// `_get(project, number)`.
async fn get(store: &Store, project: &Project, number: i64) -> Res<Issue> {
    store
        .find_one(ISSUES, doc! {"project_id": project.id, "number": number})
        .await?
        .as_ref()
        .map(Issue::from_doc)
        .ok_or_else(|| ApiError::not_found("No such issue on this mod."))
}

/// `get_issue(project, number, viewer)`.
pub async fn get_issue(store: &Store, project: &Project, number: i64, viewer: Option<&SiteUser>) -> Res<Value> {
    ensure_issues_open(project)?;
    let issue = get(store, project, number).await?;
    let events = store.find(EVENTS, doc! {"issue_id": issue.id}, Some(doc! {"created_at": 1}), 0, 0).await?;
    let mut out = match issue_dto(&issue, viewer, Some(project)) {
        Value::Object(m) => m,
        _ => Map::new(),
    };
    out.insert("events".into(), events.iter().map(|d| event_dto(&Event::from_doc(d), Some(project))).collect());
    Ok(Value::Object(out))
}

/// `_next_number(project)`: an atomic `$inc` of `issue_seq`.
async fn next_number(store: &Store, project: &Project) -> Res<i64> {
    let doc =
        store.find_one_and_update(PROJECTS, doc! {"_id": project.id}, doc! {"$inc": {"issue_seq": 1_i32}}).await?;
    let n = doc.map(|d| read_int(&d, "issue_seq", 0)).unwrap_or(0);
    Ok(if n == 0 { 1 } else { n })
}

/// `create_issue(project, actor, kind=, title=, body=)`.
pub async fn create_issue(
    store: &Store,
    project: &Project,
    actor: &SiteUser,
    kind: &str,
    title: &str,
    body: &str,
) -> Out<Value> {
    ensure_issues_open(project)?;
    let title = clean(title, TITLE_MAX, "Title", true)?;
    let body = clean(body, BODY_MAX, "Description", false)?;
    if !matches!(kind, "issue" | "request") {
        return Err(ApiError::bad_request("kind must be 'issue' or 'request'.").into());
    }
    let open_by_author =
        store.count(ISSUES, doc! {"project_id": project.id, "author_id": actor.id, "status": "open"}).await?;
    if open_by_author >= MAX_OPEN_PER_AUTHOR {
        return Err(ApiError::new(
            axum::http::StatusCode::TOO_MANY_REQUESTS,
            ErrorCode::RateLimited,
            format!(
                "You already have {MAX_OPEN_PER_AUTHOR} open threads on this mod. Close one before opening another."
            ),
        )
        .into());
    }
    limit(store, actor, "create").await?;
    let now = store.now();
    let number = next_number(store, project).await?;
    let mut participants = viewer_ids(project);
    if !participants.contains(&actor.id) {
        participants.push(actor.id);
    }
    let mut issue = Issue {
        id: zero(),
        project_id: project.id,
        project_slug: project.slug.clone(),
        project_handle: project.owner_handle.clone(),
        project_title: project.title.clone(),
        number,
        kind: kind.into(),
        title,
        body,
        author_id: actor.id,
        author_username: display(actor),
        status: "open".into(),
        closed_at: None,
        closed_by_id: None,
        comment_count: 0,
        participant_ids: dedup(participants),
        created_at: now,
        updated_at: now,
        last_activity_at: now,
        last_activity_by: Some(actor.id),
    };
    issue.id = store.insert(ISSUES, issue.fields()).await?;
    sync_open_count(store, project).await?;
    Ok(issue_dto(&issue, Some(actor), Some(project)))
}

fn dedup(ids: Vec<ObjectId>) -> Vec<ObjectId> {
    let mut out: Vec<ObjectId> = Vec::with_capacity(ids.len());
    for id in ids {
        if !out.contains(&id) {
            out.push(id);
        }
    }
    out
}

/// `add_comment(project, number, actor, body)`.
pub async fn add_comment(store: &Store, project: &Project, number: i64, actor: &SiteUser, body: &str) -> Out<Value> {
    ensure_issues_open(project)?;
    let issue = get(store, project, number).await?;
    let body = clean(body, COMMENT_MAX, "Reply", true)?;
    limit(store, actor, "comment").await?;
    let mut event = Event::new(&issue, "comment", body, actor, store.now());
    event.insert(store).await?;
    touch(store, &issue, actor, doc! {"$inc": {"comment_count": 1_i32}}).await?;
    Ok(event_dto(&event, Some(project)))
}

/// `_touch(issue, actor, extra)`: activity stamp plus `$addToSet` of the actor.
async fn touch(store: &Store, issue: &Issue, actor: &SiteUser, extra: Document) -> Res<DateTime<Utc>> {
    let now = store.now();
    let mut update = doc! {
        "$set": {"updated_at": date(now), "last_activity_at": date(now), "last_activity_by": actor.id},
        "$addToSet": {"participant_ids": actor.id},
    };
    for (op, fields) in extra {
        let Bson::Document(fields) = fields else { continue };
        match update.get_mut(&op) {
            Some(Bson::Document(existing)) => existing.extend(fields),
            _ => {
                update.insert(op, fields);
            }
        }
    }
    store.update_one(ISSUES, doc! {"_id": issue.id}, update).await?;
    Ok(now)
}

/// `set_status(project, number, actor, status, comment)`.
pub async fn set_status(
    store: &Store,
    project: &Project,
    number: i64,
    actor: &SiteUser,
    status: &str,
    comment: &str,
) -> Out<Value> {
    ensure_issues_open(project)?;
    if !matches!(status, "open" | "closed") {
        return Err(ApiError::bad_request("status must be 'open' or 'closed'.").into());
    }
    let mut issue = get(store, project, number).await?;
    if !can_moderate(project, Some(actor)) && actor.id != issue.author_id {
        return Err(ApiError::forbidden("Only the mod's creator or the person who opened this can close it.").into());
    }
    if issue.status == status {
        return Ok(issue_dto(&issue, Some(actor), Some(project)));
    }
    let comment = clean(comment, COMMENT_MAX, "Reply", false)?;
    limit(store, actor, "comment").await?;
    if !comment.is_empty() {
        Event::new(&issue, "comment", comment.clone(), actor, store.now()).insert(store).await?;
        issue.comment_count += 1;
    }
    let kind = if status == "closed" { "closed" } else { "reopened" };
    Event::new(&issue, kind, String::new(), actor, store.now()).insert(store).await?;
    issue.status = status.into();
    issue.closed_at = (status == "closed").then(|| store.now());
    issue.closed_by_id = (status == "closed").then_some(actor.id);
    let mut extra = doc! {"$set": {
        "status": &issue.status,
        "closed_at": opt_date(issue.closed_at),
        "closed_by_id": opt_oid(issue.closed_by_id),
    }};
    if !comment.is_empty() {
        extra.insert("$inc", doc! {"comment_count": 1_i32});
    }
    let now = touch(store, &issue, actor, extra).await?;
    issue.updated_at = now;
    issue.last_activity_at = now;
    issue.last_activity_by = Some(actor.id);
    sync_open_count(store, project).await?;
    Ok(issue_dto(&issue, Some(actor), Some(project)))
}

/// `delete_issue(project, number, actor)`.
pub async fn delete_issue(store: &Store, project: &Project, number: i64, actor: &SiteUser) -> Res<()> {
    let issue = get(store, project, number).await?;
    if !can_moderate(project, Some(actor)) && actor.id != issue.author_id {
        return Err(ApiError::forbidden("You can't delete this thread."));
    }
    store.delete_many(EVENTS, doc! {"issue_id": issue.id}).await?;
    store.delete_one(ISSUES, doc! {"_id": issue.id}).await?;
    sync_open_count(store, project).await
}

/// `delete_comment(project, number, event_id, actor)`, with the id parsed the
/// way the rest of the module does (Python's extra `to_oid` argument raises).
pub async fn delete_comment(
    store: &Store,
    project: &Project,
    number: i64,
    event_id: &str,
    actor: &SiteUser,
) -> Res<()> {
    let issue = get(store, project, number).await?;
    let event = match crate::mods_write_auth::docs::to_oid(event_id) {
        Some(oid) => store.get(EVENTS, oid).await?.as_ref().map(Event::from_doc),
        None => None,
    };
    let Some(event) = event.filter(|e| e.issue_id == issue.id && e.kind == "comment") else {
        return Err(ApiError::not_found("No such reply on this issue."));
    };
    if !can_moderate(project, Some(actor)) && actor.id != event.author_id {
        return Err(ApiError::forbidden("You can't delete this reply."));
    }
    store.delete_one(EVENTS, doc! {"_id": event.id}).await?;
    let count = (issue.comment_count - 1).max(0);
    store.update_one(ISSUES, doc! {"_id": issue.id}, doc! {"$set": {"comment_count": int(count)}}).await
}

/// `_sync_open_count(project)`: recount and `$set` by key.
async fn sync_open_count(store: &Store, project: &Project) -> Res<()> {
    let n = store.count(ISSUES, doc! {"project_id": project.id, "status": "open"}).await?;
    store.update_one(PROJECTS, doc! {"_id": project.id}, doc! {"$set": {"open_issue_count": int(n as i64)}}).await
}

/// `notifications(user)`.
pub async fn notifications(store: &Store, user: &SiteUser) -> Res<Value> {
    let seen = user.mod_issues_seen_at.map(crate::dt::from_bson);
    let rows = store
        .find(
            ISSUES,
            doc! {"participant_ids": user.id, "last_activity_by": {"$ne": user.id}},
            Some(doc! {"last_activity_at": -1}),
            0,
            NOTIFICATION_LIMIT,
        )
        .await?;
    let mut items = Vec::with_capacity(rows.len());
    let mut unread = 0;
    for d in &rows {
        let issue = Issue::from_doc(d);
        let is_new = seen.is_none_or(|s| issue.last_activity_at > s);
        unread += i64::from(is_new);
        items.push(json!({
            "number": issue.number,
            "kind": issue.kind,
            "title": issue.title,
            "status": issue.status,
            "mod_title": issue.project_title,
            "handle": issue.project_handle,
            "slug": issue.project_slug,
            "url": format!("/mods/{}/{}#issue-{}", issue.project_handle, issue.project_slug, issue.number),
            "comment_count": issue.comment_count,
            "last_activity_at": iso(Some(issue.last_activity_at)),
            "unread": is_new,
        }));
    }
    Ok(json!({"items": items, "unread": unread, "seen_at": iso(seen)}))
}

/// `mark_seen(user)`.
pub async fn mark_seen(store: &Store, user: &SiteUser) -> Res<Value> {
    let now = store.now();
    store.update_one(SITE_USERS, doc! {"_id": user.id}, doc! {"$set": {"mod_issues_seen_at": date(now)}}).await?;
    Ok(json!({"seen_at": iso(Some(now))}))
}

/// `purge_project(project_id)`, for the project delete.
pub async fn purge_project(store: &Store, project_id: ObjectId) -> Res<()> {
    store.delete_many(EVENTS, doc! {"project_id": project_id}).await?;
    store.delete_many(ISSUES, doc! {"project_id": project_id}).await
}
