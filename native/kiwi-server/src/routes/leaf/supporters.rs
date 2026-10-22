//! `app/supporters`: the public credits list (`GET /v1/misc/supporters`,
//! tokenless `misc:read`) and the master CRUD under `/admin/supporters`.

use axum::{
    Router,
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, Uri, header},
    response::{IntoResponse, Response},
    routing::{delete, get},
};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use kiwi_core::leaf::{clean_supporter_names, py_strip};
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
    error::{ErrorKind, WriteFailure},
};
use serde_json::{Value, json};

use super::{body::DepsThenBody, internal};
use crate::{
    auth::{Public, Superuser, scopes},
    dt,
    error::{self, ApiError},
    middleware::pyurl::scope_path,
    pyjson::PydanticJson,
    ratelimit::RateLimitInfo,
    state::AppState,
    validation::{Field, FromBody},
};

pub const COLLECTION: &str = "supporters";
const DELETE_PREFIX: &str = "/admin/supporters/";

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/misc/supporters", get(list_public))
        .route(
            "/admin/supporters",
            get(admin_list).post(admin_add).put(admin_replace).fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/admin/supporters/{name}", delete(admin_remove))
}

fn coll(state: &AppState) -> Collection<Document> {
    state.mongo().collection(COLLECTION)
}

/// One `Supporter` as Beanie loads it (a document it can't validate is a 500).
#[derive(Debug, Clone, PartialEq)]
pub struct Supporter {
    pub name: String,
    pub added_by: Option<ObjectId>,
    pub created_at: DateTime<Utc>,
}

impl Supporter {
    pub fn from_doc(d: &Document) -> Result<Self, ApiError> {
        let name = match d.get("name") {
            Some(Bson::String(s)) => s.clone(),
            other => return Err(internal(format!("supporter name {other:?}"))),
        };
        let added_by = match d.get("added_by") {
            None | Some(Bson::Null) => None,
            Some(Bson::ObjectId(o)) => Some(*o),
            Some(Bson::String(s)) => Some(ObjectId::parse_str(s).map_err(internal)?),
            Some(other) => return Err(internal(format!("supporter added_by {other:?}"))),
        };
        let created_at = match d.get("created_at") {
            None => dt::now(),
            Some(Bson::DateTime(b)) => dt::from_bson(*b),
            Some(other) => return Err(internal(format!("supporter created_at {other:?}"))),
        };
        Ok(Supporter { name, added_by, created_at })
    }

    /// `SupporterAdminView`.
    pub fn admin_view(&self) -> Value {
        json!({
            "name": self.name,
            "added_by": self.added_by.map(|o| o.to_hex()),
            "created_at": dt::pydantic(&self.created_at),
        })
    }

    /// The row Beanie inserts (`_id` first, as pymongo sends it).
    pub fn to_doc(&self, id: ObjectId) -> Document {
        doc! {
            "_id": id,
            "name": &self.name,
            "added_by": self.added_by,
            "created_at": dt::to_bson(self.created_at),
        }
    }
}

async fn load_all(state: &AppState) -> Result<Vec<Supporter>, ApiError> {
    let docs: Vec<Document> = coll(state)
        .find(doc! {})
        .sort(doc! {"created_at": 1})
        .await
        .map_err(internal)?
        .try_collect()
        .await
        .map_err(internal)?;
    docs.iter().map(Supporter::from_doc).collect()
}

/// `SupporterList`.
pub fn public_body(names: &[String]) -> Value {
    json!({"supporters": names, "count": names.len()})
}

/// The public list with the caller's `X-RateLimit-*` and a 5-minute cache.
pub fn public_response(names: &[String], rate_limit: RateLimitInfo) -> Response {
    let cache = [(header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=300"))];
    (rate_limit, cache, PydanticJson(public_body(names))).into_response()
}

async fn list_public(
    State(state): State<AppState>,
    Public(ctx): Public<{ scopes::MISC_READ }>,
) -> Result<Response, ApiError> {
    let names: Vec<String> = load_all(&state).await?.into_iter().map(|s| s.name).collect();
    Ok(public_response(&names, ctx.rate_limit))
}

/// `SupporterAdminList`.
pub fn admin_list_body(items: &[Supporter]) -> Value {
    let views: Vec<Value> = items.iter().map(Supporter::admin_view).collect();
    json!({"items": views, "count": items.len()})
}

async fn admin_list(State(state): State<AppState>, _: Superuser) -> Result<PydanticJson, ApiError> {
    Ok(PydanticJson(admin_list_body(&load_all(&state).await?)))
}

pub struct AddRequest {
    pub name: String,
}

impl FromBody for AddRequest {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(AddRequest { name: m.field("name").str().required() })).required()
    }
}

fn is_duplicate_key(e: &mongodb::error::Error) -> bool {
    matches!(&*e.kind, ErrorKind::Write(WriteFailure::WriteError(w)) if w.code == 11000)
}

async fn admin_add(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), req): DepsThenBody<Superuser, AddRequest>,
) -> Result<Response, ApiError> {
    let name = py_strip(&req.name);
    if name.is_empty() {
        return Err(ApiError::from_value_error("Supporter name cannot be empty"));
    }
    let exists = || ApiError::from_value_error(format!("Supporter '{name}' already exists"));
    let coll = coll(&state);
    if coll.find_one(doc! {"name": name}).await.map_err(internal)?.is_some() {
        return Err(exists());
    }
    let row = Supporter { name: name.to_owned(), added_by: Some(admin.id), created_at: dt::now() };
    match coll.insert_one(row.to_doc(ObjectId::new())).await {
        Ok(_) => Ok((StatusCode::CREATED, PydanticJson(row.admin_view())).into_response()),
        Err(e) if is_duplicate_key(&e) => Err(exists()),
        Err(e) => Err(internal(e)),
    }
}

pub struct ReplaceRequest {
    pub names: Vec<String>,
}

impl FromBody for ReplaceRequest {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ReplaceRequest { names: m.field("names").list(|f| f.str().required()).required() }))
            .required()
    }
}

pub const REPLACE_EMPTY: &str = "Provide at least one name (use DELETE to remove individually)";

async fn admin_replace(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), req): DepsThenBody<Superuser, ReplaceRequest>,
) -> Result<PydanticJson, ApiError> {
    let cleaned = clean_supporter_names(&req.names);
    if cleaned.is_empty() {
        return Err(ApiError::bad_request(REPLACE_EMPTY));
    }
    let coll = coll(&state);
    let removed = coll.delete_many(doc! {}).await.map_err(internal)?.deleted_count;
    for name in &cleaned {
        let row = Supporter { name: name.clone(), added_by: Some(admin.id), created_at: dt::now() };
        coll.insert_one(row.to_doc(ObjectId::new())).await.map_err(internal)?;
    }
    Ok(PydanticJson(json!({"removed": removed, "added": cleaned.len()})))
}

/// The 204 FastAPI sends for a `-> None` route: JSON content type, no body.
pub fn no_content() -> Response {
    (StatusCode::NO_CONTENT, [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))]).into_response()
}

/// The `{name}` Python's router sees, or `None` when its decoded path
/// doesn't match `/admin/supporters/{name}` at all.
pub fn name_from_path(uri: &Uri) -> Option<String> {
    let path = scope_path(uri);
    let name = path.strip_prefix(DELETE_PREFIX)?;
    (!name.is_empty() && !name.contains('/')).then(|| name.to_owned())
}

/// Python routes on the decoded path, so a `%2F` in the name never reaches
/// this route there: that is the framework 404, before any auth.
async fn admin_remove(State(state): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    let Some(name) = name_from_path(&parts.uri) else {
        return error::not_found(&parts.method, &scope_path(&parts.uri), &parts.headers);
    };
    if let Err(r) = Superuser::from_request_parts(&mut parts, &state).await {
        return r.into_response();
    }
    remove(&state, &name).await.unwrap_or_else(IntoResponse::into_response)
}

async fn remove(state: &AppState, name: &str) -> Result<Response, ApiError> {
    let coll = coll(state);
    let Some(found) = coll.find_one(doc! {"name": py_strip(name)}).await.map_err(internal)? else {
        return Err(ApiError::not_found(format!("No supporter named '{name}'")));
    };
    Supporter::from_doc(&found)?;
    let id = found.get("_id").cloned().unwrap_or(Bson::Null);
    coll.delete_one(doc! {"_id": id}).await.map_err(internal)?;
    Ok(no_content())
}
