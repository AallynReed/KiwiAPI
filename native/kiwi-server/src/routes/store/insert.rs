//! `POST /v1/store/insert` (`app/trove/router.py`, `store/service.insert_dump`):
//! the store bot's StoreLog.cfg dump, saved product by product the way
//! Beanie's `save()` writes them, then the categories and the state doc, the
//! ingest log and a `store` event.

use axum::{
    extract::{Request, State},
    response::{IntoResponse, Response},
};
use kiwi_core::store::{
    self as core, Category, IngestError, NO_STORE_DATA, Price, Product, State as StoreState, Summary,
    parser::{ParsedStoreDump, parse_dump},
};
use mongodb::{
    Collection, Database,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::json;

use super::{model, service};
use crate::{
    auth::{IngestAuth, pyutil::bson_int},
    error::ApiError,
    features,
    lb_ingest::signals,
    market::insert::{decode, read_upload},
    pyjson::PydanticJson,
    state::AppState,
};

pub const PATH: &str = "/v1/store/insert";

fn opt_int(v: Option<i64>) -> Bson {
    v.map_or(Bson::Null, bson_int)
}

fn prices_bson(prices: &[Price]) -> Bson {
    Bson::Array(
        prices
            .iter()
            .map(|p| {
                Bson::Document(doc! {
                    "currency": &p.currency,
                    "cost": p.cost,
                    "can_purchase": p.can_purchase,
                    "monthly": bson_int(p.monthly),
                    "sale": &p.sale,
                })
            })
            .collect(),
    )
}

fn ints(v: &[i64]) -> Bson {
    Bson::Array(v.iter().map(|i| bson_int(*i)).collect())
}

/// `StoreProductDoc` as Beanie encodes it, in model order (no `_id`).
pub fn product_doc(p: &Product) -> Document {
    let textures: Vec<Bson> = p
        .textures
        .iter()
        .map(|t| {
            Bson::Document(doc! {
                "texture": &t.texture, "x": bson_int(t.x), "y": bson_int(t.y), "text": &t.text, "overlay": t.overlay,
            })
        })
        .collect();
    let history: Vec<Bson> = p
        .price_history
        .iter()
        .map(|h| {
            Bson::Document(
                doc! { "ts": bson_int(h.ts), "prices": prices_bson(&h.prices), "price_string": &h.price_string },
            )
        })
        .collect();
    doc! {
        "code": &p.code,
        "kind": &p.kind,
        "name": &p.name,
        "image": &p.image,
        "info": &p.info,
        "informational": p.informational,
        "tradable": p.tradable,
        "prices": prices_bson(&p.prices),
        "price_string": &p.price_string,
        "price_string_currency": &p.price_string_currency,
        "price_string_sale": &p.price_string_sale,
        "promo": &p.promo,
        "deal_expires_at": opt_int(p.deal_expires_at),
        "interact_label": &p.interact_label,
        "interact_enabled": p.interact_enabled,
        "trial_limits": &p.trial_limits,
        "class_level": opt_int(p.class_level),
        "class_power_rank": opt_int(p.class_power_rank),
        "class_shield_frame": opt_int(p.class_shield_frame),
        "class_sub_name": &p.class_sub_name,
        "class_icon": &p.class_icon,
        "textures": textures,
        "loot_title": &p.loot_title,
        "loot_body": &p.loot_body,
        "categories": ints(&p.categories),
        "first_seen": bson_int(p.first_seen),
        "last_seen": bson_int(p.last_seen),
        "price_history": history,
        "availability": Bson::Array(p.availability.iter().map(|iv| ints(iv)).collect()),
    }
}

pub fn category_doc(c: &Category) -> Document {
    let codes: Vec<Bson> = c.codes.iter().map(|s| Bson::String(s.clone())).collect();
    doc! {
        "index": bson_int(c.index),
        "label": &c.label,
        "icon": &c.icon,
        "codes": codes,
        "last_seen": bson_int(c.last_seen),
    }
}

pub fn state_doc(s: &StoreState) -> Document {
    doc! {
        "key": &s.key,
        "last_anchor": opt_int(s.last_anchor),
        "title": &s.title,
        "product_count": bson_int(s.product_count),
        "category_count": bson_int(s.category_count),
    }
}

/// Beanie `save()`: `$set` of every field (and the loaded `_id`), upserting;
/// a document built in memory has no `_id` and lands on a fresh one.
async fn save(c: &Collection<Document>, id: Option<Bson>, fields: Document) -> mongodb::error::Result<()> {
    let (filter_id, mut set) = match id {
        Some(id) => (id.clone(), doc! { "_id": id }),
        None => (Bson::ObjectId(ObjectId::new()), Document::new()),
    };
    set.extend(fields);
    c.update_one(doc! { "_id": filter_id }, doc! { "$set": set }).upsert(true).await?;
    Ok(())
}

/// Why `insert_dump` stopped.
#[derive(Debug)]
pub enum InsertError {
    /// A `ValueError`: the 400 with a failed log row.
    Value(String),
    /// Anything else: the unhandled 500.
    Internal(String),
}

fn internal(e: impl std::fmt::Display) -> InsertError {
    InsertError::Internal(e.to_string())
}

/// `_get_state()`.
async fn load_state(db: &Database) -> Result<(Option<Bson>, StoreState), InsertError> {
    match db.collection::<Document>(service::STATE).find_one(doc! { "key": "state" }).await.map_err(internal)? {
        None => Ok((None, StoreState::default())),
        Some(d) => Ok((d.get("_id").cloned(), model::state(&d).map_err(|e| internal(e.0))?)),
    }
}

/// An ingest rule that raised: `int(float("nan"))` is the one ValueError.
fn ingest_error(e: IngestError, deal: Option<f64>) -> InsertError {
    if deal.is_some_and(f64::is_nan) {
        InsertError::Value("cannot convert float NaN to integer".into())
    } else {
        InsertError::Internal(e.0)
    }
}

/// `service.insert_dump(text, timestamp)`.
pub async fn insert_dump(db: &Database, parsed: &ParsedStoreDump, anchor: i64) -> Result<Summary, InsertError> {
    if parsed.products.is_empty() && parsed.categories.is_empty() {
        return Err(InsertError::Value(NO_STORE_DATA.into()));
    }
    let prev_anchor = load_state(db).await?.1.last_anchor;
    let products = db.collection::<Document>(service::PRODUCTS);
    let (mut created, mut price_changes) = (0, 0);
    for p in &parsed.products {
        let found = products.find_one(doc! { "code": &p.code }).await.map_err(internal)?;
        let (id, existing) = match found {
            None => (None, None),
            Some(d) => (d.get("_id").cloned(), Some(model::product(&d).map_err(|e| internal(e.0))?)),
        };
        let (doc, is_new, changed) =
            core::ingest_product(existing, p, anchor, prev_anchor).map_err(|e| ingest_error(e, p.deal_seconds))?;
        created += usize::from(is_new);
        price_changes += usize::from(changed);
        save(&products, id, product_doc(&doc)).await.map_err(internal)?;
    }
    let categories = db.collection::<Document>(service::CATEGORIES);
    for c in &parsed.categories {
        let found = categories.find_one(doc! { "index": bson_int(c.index) }).await.map_err(internal)?;
        let (id, existing) = match found {
            None => (None, None),
            Some(d) => (d.get("_id").cloned(), Some(model::category(&d).map_err(|e| internal(e.0))?)),
        };
        let cat = core::ingest_category(existing, c, anchor);
        save(&categories, id, category_doc(&cat)).await.map_err(internal)?;
    }
    let (id, state) = load_state(db).await?;
    let state = core::ingest_state(state, parsed, anchor);
    save(&db.collection::<Document>(service::STATE), id, state_doc(&state)).await.map_err(internal)?;
    let summary = Summary {
        products: parsed.products.len(),
        categories: parsed.categories.len(),
        created,
        price_changes,
        anchor,
        done_marker: parsed.done,
    };
    tracing::info!(
        "store ingest: {{'products': {}, 'categories': {}, 'created': {created}, 'price_changes': {price_changes}, \
         'anchor': {anchor}, 'done_marker': {}}}",
        summary.products,
        summary.categories,
        if parsed.done { "True" } else { "False" }
    );
    Ok(summary)
}

fn n(v: usize) -> Bson {
    bson_int(v as i64)
}

pub fn log_summary(s: &Summary, nbytes: usize) -> Document {
    doc! {
        "products": n(s.products),
        "categories": n(s.categories),
        "created": n(s.created),
        "price_changes": n(s.price_changes),
        "anchor": bson_int(s.anchor),
        "done_marker": s.done_marker,
        "bytes": n(nbytes),
    }
}

/// `StoreInsertResponse`.
pub fn response_body(s: &Summary) -> serde_json::Value {
    json!({
        "products": s.products,
        "categories": s.categories,
        "created": s.created,
        "price_changes": s.price_changes,
        "anchor": s.anchor,
        "done_marker": s.done_marker,
    })
}

/// `str(exc)[:300]`.
fn clip(msg: &str) -> &str {
    kiwi_core::pystr::prefix(msg, 300)
}

async fn failed(state: &AppState, auth: &IngestAuth, msg: &str, nbytes: usize) -> Response {
    let summary = doc! { "bytes": n(nbytes) };
    signals::record(state, PATH, &auth.user, auth.token.as_ref(), summary, Some(clip(msg))).await;
    ApiError::bad_request(msg).into_response()
}

pub async fn insert(State(state): State<AppState>, req: Request) -> Response {
    let (auth, raw, timestamp) = match read_upload::<features::Store>(&state, req, false).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    let parsed = parse_dump(&decode(&raw));
    let anchor = timestamp.unwrap_or_else(|| chrono::Utc::now().timestamp());
    let Some(db) = state.inner.db.mongo.as_ref() else {
        tracing::error!("store insert: MONGO_URI not configured");
        return ApiError::internal().into_response();
    };
    let summary = match insert_dump(db, &parsed, anchor).await {
        Ok(s) => s,
        Err(InsertError::Value(msg)) => return failed(&state, &auth, &msg, raw.len()).await,
        Err(InsertError::Internal(e)) => {
            tracing::error!("store insert: {e}");
            return ApiError::internal().into_response();
        }
    };
    signals::record(&state, PATH, &auth.user, auth.token.as_ref(), log_summary(&summary, raw.len()), None).await;
    let data = json!({"anchor": summary.anchor, "products": summary.products, "price_changes": summary.price_changes});
    crate::events::publish(&state, "store", &summary.anchor.to_string(), data).await;
    (auth.rate_limit, PydanticJson(response_body(&summary))).into_response()
}
