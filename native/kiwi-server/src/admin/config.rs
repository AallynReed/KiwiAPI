//! `/admin/config*`: every registered tunable with its effective value, and
//! the override writes (`set_setting` / `reset_setting`).

use std::collections::HashMap;

use axum::extract::State;
use futures::TryStreamExt;
use mongodb::bson::{Bson, DateTime, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{KeySeg, Res, Seg, coll, docs::pyd, internal};
use crate::{
    auth::Superuser,
    error::ApiError,
    pyjson::PydanticJson,
    runtime_config::{self, COLLECTION, REGISTRY, Spec},
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

/// `RuntimeConfigUpdate`.
pub struct UpdateBody {
    pub value: Value,
}

impl FromBody for UpdateBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(UpdateBody { value: m.field("value").any().required() })).required()
    }
}

/// One stored override, as Beanie loads a `RuntimeConfig` document.
#[derive(Debug, Clone)]
pub struct Override {
    pub value: String,
    pub updated_at: DateTime,
    pub updated_by_user_id: Option<ObjectId>,
}

impl Override {
    pub fn from_doc(d: &Document) -> Result<(String, Override), ApiError> {
        let key = d.get_str("key").map_err(internal)?.to_owned();
        let value = d.get_str("value").map_err(internal)?.to_owned();
        let updated_at = match d.get("updated_at") {
            Some(Bson::DateTime(t)) => *t,
            None => super::docs::now(),
            Some(other) => return Err(internal(format!("runtime_config {key}: updated_at {other}"))),
        };
        let updated_by_user_id = match d.get("updated_by_user_id") {
            None | Some(Bson::Null) => None,
            Some(Bson::ObjectId(o)) => Some(*o),
            Some(Bson::String(s)) => Some(ObjectId::parse_str(s).map_err(internal)?),
            Some(other) => return Err(internal(format!("runtime_config {key}: updated_by_user_id {other}"))),
        };
        Ok((key, Override { value, updated_at, updated_by_user_id }))
    }
}

/// One `list_all()` item (`RuntimeConfigItem` field order).
pub fn item(spec: &Spec, default: Value, over: Option<&Override>) -> Value {
    let (value, is_default, updated_at, updated_by) = match over {
        Some(o) => (
            runtime_config::decode_stored(&o.value).unwrap_or_else(|| default.clone()),
            false,
            Value::from(pyd(&o.updated_at)),
            o.updated_by_user_id.map(|id| id.to_hex()),
        ),
        None => (default.clone(), true, Value::Null, None),
    };
    json!({
        "key": spec.key,
        "category": spec.category,
        "type": spec.ty.as_str(),
        "description": spec.description,
        "secret": spec.secret,
        "default": default,
        "value": value,
        "is_default": is_default,
        "min_value": spec.min_value.map(|n| n.to_json()),
        "max_value": spec.max_value.map(|n| n.to_json()),
        "choices": spec.choices.filter(|c| !c.is_empty()),
        "updated_at": updated_at,
        "updated_by_user_id": updated_by,
    })
}

/// `list_all()` over the loaded overrides.
pub fn items(state: &AppState, overrides: &HashMap<String, Override>) -> Vec<Value> {
    REGISTRY.iter().map(|s| item(s, s.default_value(state.settings()), overrides.get(s.key))).collect()
}

async fn load_overrides(state: &AppState) -> Result<HashMap<String, Override>, ApiError> {
    let docs: Vec<Document> =
        coll(state, COLLECTION).find(doc! {}).await.map_err(internal)?.try_collect().await.map_err(internal)?;
    let mut out = HashMap::new();
    for d in &docs {
        let (k, o) = Override::from_doc(d)?;
        out.insert(k, o);
    }
    Ok(out)
}

pub async fn list_runtime_config(State(state): State<AppState>, _: Superuser) -> Res {
    let items = items(&state, &load_overrides(&state).await?);
    let count = items.len();
    Ok(PydanticJson(json!({"items": items, "count": count})))
}

fn unknown(key: &str) -> ApiError {
    ApiError::not_found(format!("Unknown setting key '{key}'."))
}

/// `set_setting`'s checks, before any store access: 404 for an unknown key,
/// 400 with the validation message, else the value to persist.
pub fn validate(key: &str, value: &Value) -> Result<(&'static Spec, Value), ApiError> {
    let spec = runtime_config::spec(key).ok_or_else(|| unknown(key))?;
    let v = spec.validate(value).map_err(|e| ApiError::bad_request(e.0))?;
    Ok((spec, v))
}

async fn readback(state: &AppState, spec: &Spec) -> Res {
    let overrides = load_overrides(state).await?;
    Ok(PydanticJson(item(spec, spec.default_value(state.settings()), overrides.get(spec.key))))
}

pub async fn update_runtime_config(
    State(state): State<AppState>,
    Seg(key): KeySeg,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, UpdateBody>,
) -> Res {
    let (spec, validated) = validate(&key, &body.value)?;
    let encoded = runtime_config::encode_value(&validated);
    let c = coll(&state, COLLECTION);
    let now = super::docs::now();
    match c.find_one(doc! {"key": spec.key}).await.map_err(internal)? {
        None => {
            let d = doc! {
                "_id": ObjectId::new(),
                "key": spec.key,
                "value": &encoded,
                "updated_at": now,
                "updated_by_user_id": admin.id,
            };
            c.insert_one(d).await.map_err(internal)?;
        }
        Some(found) => {
            Override::from_doc(&found)?;
            let id = found.get("_id").cloned().unwrap_or(Bson::Null);
            let set = doc! {
                "_id": id.clone(),
                "key": spec.key,
                "value": &encoded,
                "updated_at": now,
                "updated_by_user_id": admin.id,
            };
            c.update_one(doc! {"_id": id}, doc! {"$set": set}).upsert(true).await.map_err(internal)?;
        }
    }
    runtime_config::invalidate(spec.key);
    readback(&state, spec).await
}

pub async fn reset_runtime_config(State(state): State<AppState>, _: Superuser, Seg(key): KeySeg) -> Res {
    let spec = runtime_config::spec(&key).ok_or_else(|| unknown(&key))?;
    let c = coll(&state, COLLECTION);
    if let Some(found) = c.find_one(doc! {"key": spec.key}).await.map_err(internal)? {
        Override::from_doc(&found)?;
        let id = found.get("_id").cloned().unwrap_or(Bson::Null);
        c.delete_one(doc! {"_id": id}).await.map_err(internal)?;
    }
    runtime_config::invalidate(spec.key);
    readback(&state, spec).await
}
