//! The BTT mod blocklist: `/v1/btt/mod-blocklist` and the master's
//! `/admin/btt/mod-blocklist` editing routes (`app/btt_blocklist`).

use axum::{
    extract::{FromRequest, FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use kiwi_core::btt::{BLOCK_KINDS, check_block_values, py_strip};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::Value;

use super::{Mongo, Store, dt_field, internal, obj, str_field};
use crate::{
    auth::{Public, Superuser, scopes::BTT_READ},
    dt,
    error::{ApiError, ApiResult},
    pyjson::PydanticJson,
    state::AppState,
    validation::{Field, FromBody, FromParams, Params, Path, Validator, read_body},
};

pub const BLOCKLIST: &str = "btt_mod_blocklist";
pub const TAKEDOWNS: &str = "mod_taken_down_artifacts";

/// `served()`: every value per kind, oldest first.
#[derive(Default)]
pub struct Served {
    pub author: Vec<String>,
    pub mod_: Vec<String>,
    pub hash: Vec<String>,
    pub workshop: Vec<String>,
    pub workshop_author: Vec<String>,
}

impl Served {
    pub fn from_entries(entries: &[Document]) -> ApiResult<Served> {
        let mut out = Served::default();
        for e in entries {
            let value = str_field(e, "value", None)?;
            match str_field(e, "kind", None)?.as_str() {
                "author" => out.author.push(value),
                "mod" => out.mod_.push(value),
                "hash" => out.hash.push(value),
                "workshop" => out.workshop.push(value),
                "workshop_author" => out.workshop_author.push(value),
                other => return Err(internal(format!("bad blocklist kind {other:?}"))),
            }
        }
        Ok(out)
    }
}

pub(crate) async fn served<S: Store>(s: &S) -> ApiResult<Served> {
    Served::from_entries(&s.blocklist(false).await?)
}

/// `taken_down_md5s()`: sorted.
async fn taken_down_md5s<S: Store>(s: &S) -> ApiResult<Vec<String>> {
    let mut out = s
        .takedown_md5s()
        .await?
        .into_iter()
        .map(|b| match b {
            Bson::String(s) => Ok(s),
            other => Err(internal(format!("bad takedown md5 {other:?}"))),
        })
        .collect::<ApiResult<Vec<_>>>()?;
    out.sort();
    Ok(out)
}

/// `BttModBlocklist`; `hashes` adds the takedown md5s, first occurrence kept.
pub fn public_body(s: &Served, takedowns: &[String]) -> Value {
    let mut seen = std::collections::HashSet::new();
    let hashes: Vec<String> = s.hash.iter().chain(takedowns).filter(|h| seen.insert(*h)).cloned().collect();
    obj(vec![
        ("authors", s.author.clone().into()),
        ("mods", s.mod_.clone().into()),
        ("hashes", hashes.into()),
        ("workshop_items", s.workshop.clone().into()),
        ("workshop_authors", s.workshop_author.clone().into()),
    ])
}

pub(crate) async fn public_core<S: Store>(s: &S) -> ApiResult<Value> {
    let entries = served(s).await?;
    Ok(public_body(&entries, &taken_down_md5s(s).await?))
}

pub async fn public(State(state): State<AppState>, Public(ctx): Public<{ BTT_READ }>) -> ApiResult<impl IntoResponse> {
    Ok((ctx.rate_limit, PydanticJson(public_core(&Mongo(&state)).await?)))
}

// --- admin ------------------------------------------------------------------------

/// `entry_dto` of a stored entry.
pub fn entry_dto(e: &Document) -> ApiResult<Value> {
    let id = e.get_object_id("_id").map_err(internal)?;
    let created = dt_field(e, "created_at", true)?;
    Ok(dto(id, &str_field(e, "kind", None)?, &str_field(e, "value", None)?, &str_field(e, "note", Some(""))?, created))
}

fn dto(id: ObjectId, kind: &str, value: &str, note: &str, created_at: chrono::DateTime<chrono::Utc>) -> Value {
    obj(vec![
        ("id", id.to_hex().into()),
        ("kind", kind.into()),
        ("value", value.into()),
        ("note", note.into()),
        ("created_at", dt::isoformat(&created_at).into()),
    ])
}

pub(crate) async fn admin_list_core<S: Store>(s: &S) -> ApiResult<Value> {
    let items = s.blocklist(true).await?.iter().map(entry_dto).collect::<ApiResult<Vec<_>>>()?;
    Ok(obj(vec![("items", Value::Array(items))]))
}

pub async fn admin_list(State(state): State<AppState>, _admin: Superuser) -> ApiResult<impl IntoResponse> {
    Ok(PydanticJson(admin_list_core(&Mongo(&state)).await?))
}

/// `BlockAdd`.
pub struct BlockAdd {
    pub kind: String,
    pub values: String,
    pub note: String,
}

impl FromBody for BlockAdd {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(BlockAdd {
                kind: m.field("kind").literal(&BLOCK_KINDS).required(),
                values: m.field("values").str().max_len(20000).required(),
                note: m.field("note").str().max_len(500).default(String::new()),
            })
        })
        .required()
    }
}

/// A body on a superuser route, in FastAPI's order: malformed JSON (422),
/// then the router's `get_current_superuser`, then body validation.
pub struct AdminBody<T>(pub T);

impl<T: FromBody> FromRequest<AppState> for AdminBody<T> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        Superuser::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        let mut v = Validator::default();
        let value = T::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
        match v.finish(value) {
            Ok(Some(t)) => Ok(AdminBody(t)),
            Ok(None) => Err(ApiError::internal().into_response()),
            Err(e) => Err(e.into_response()),
        }
    }
}

/// `add`: every new value inserted (duplicates skipped), as dtos.
pub(crate) async fn admin_add_core<S: Store>(s: &S, req: &BlockAdd) -> ApiResult<Value> {
    let values = check_block_values(&req.kind, &req.values).map_err(ApiError::bad_request)?;
    let note = py_strip(&req.note);
    let mut added = Vec::new();
    for value in values {
        let (id, now) = (ObjectId::new(), dt::now());
        let entry = doc! {
            "_id": id,
            "kind": &req.kind,
            "value": &value,
            "note": note,
            "created_at": dt::to_bson(now),
        };
        if s.insert_block(entry).await? {
            added.push(dto(id, &req.kind, &value, note, now));
        }
    }
    Ok(obj(vec![("added", Value::Array(added))]))
}

pub async fn admin_add(State(state): State<AppState>, AdminBody(req): AdminBody<BlockAdd>) -> ApiResult<Response> {
    Ok((StatusCode::CREATED, PydanticJson(admin_add_core(&Mongo(&state), &req).await?)).into_response())
}

pub struct EntryPath(pub String);

impl FromParams for EntryPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        EntryPath(p.path("entry_id").str().required())
    }
}

pub(crate) async fn admin_remove_core<S: Store>(s: &S, entry_id: &str) -> ApiResult<Response> {
    let not_found = || ApiError::not_found("No such entry.");
    let id = ObjectId::parse_str(entry_id).map_err(|_| not_found())?;
    if !s.delete_block(id).await? {
        return Err(not_found());
    }
    Ok((StatusCode::NO_CONTENT, [(header::CONTENT_TYPE, HeaderValue::from_static("application/json"))]).into_response())
}

pub async fn admin_remove(
    State(state): State<AppState>,
    _admin: Superuser,
    Path(EntryPath(entry_id)): Path<EntryPath>,
) -> ApiResult<Response> {
    admin_remove_core(&Mongo(&state), &entry_id).await
}
