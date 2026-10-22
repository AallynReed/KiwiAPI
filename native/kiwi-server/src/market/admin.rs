//! `/admin/market/*` (`app/admin/router.py` + `market/service.py`): the
//! scraper's interest allow-list and the `/market` sidebar categories.
//! Category membership is item names, so allow-list edits never touch it.

use axum::{
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
};
use futures::TryStreamExt;
use kiwi_core::market as core;
use mongodb::{
    Collection,
    bson::{Bson, Document, doc, oid::ObjectId},
};
use serde_json::{Value, json};

use super::{
    Segment,
    store::{CATEGORIES, Category, INTEREST, InterestItem, internal},
};
use crate::{
    auth::Superuser,
    dt,
    error::{ApiError, ApiResult},
    pyjson::{PydanticJson, no_content},
    routes::misc::invalidate_interest_cache,
    state::AppState,
    validation::{DepsThenBody, Field, FromBody},
};

fn coll(state: &AppState, name: &str) -> Collection<Document> {
    state.mongo().collection(name)
}

async fn all(c: &Collection<Document>, sort: Option<Document>) -> ApiResult<Vec<Document>> {
    let find = c.find(doc! {});
    let find = match sort {
        Some(s) => find.sort(s),
        None => find,
    };
    find.await.map_err(internal)?.try_collect().await.map_err(internal)
}

fn strip(s: &str) -> &str {
    kiwi_core::pystr::strip(s)
}

// --- request bodies ------------------------------------------------------------------

/// `InterestItemAddRequest` / `MarketCategoryCreateRequest`.
pub struct NameBody {
    pub name: String,
}

impl FromBody for NameBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(NameBody { name: m.field("name").str().required() })).required()
    }
}

/// `InterestItemBulkReplaceRequest`.
pub struct ItemsBody {
    pub items: Vec<String>,
}

impl FromBody for ItemsBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(ItemsBody { items: m.field("items").list(|f| f.str().required()).required() })).required()
    }
}

/// `MarketCategoryUpdateRequest`.
pub struct UpdateBody {
    pub name: Option<String>,
    pub items: Option<Vec<String>>,
}

impl FromBody for UpdateBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(UpdateBody {
                name: m.field("name").str().optional(),
                items: m.field("items").list(|f| f.str().required()).optional(),
            })
        })
        .required()
    }
}

/// `MarketCategoryReorderRequest`.
pub struct IdsBody {
    pub ids: Vec<String>,
}

impl FromBody for IdsBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(IdsBody { ids: m.field("ids").list(|f| f.str().required()).required() })).required()
    }
}

// --- interest items ------------------------------------------------------------------

/// `InterestItemAdminView`.
pub fn interest_view(d: &InterestItem) -> Value {
    json!({"name": d.name, "added_by": d.added_by.map(|o| o.to_hex()), "added_at": dt::pydantic(&d.added_at)})
}

pub async fn list_interest(State(state): State<AppState>, _: Superuser) -> ApiResult<PydanticJson> {
    let docs = all(&coll(&state, INTEREST), Some(doc! {"name": 1})).await?;
    let items = docs.iter().map(InterestItem::from_doc).collect::<ApiResult<Vec<_>>>()?;
    let views: Vec<Value> = items.iter().map(interest_view).collect();
    Ok(PydanticJson(json!({"items": views, "count": items.len()})))
}

/// `admin_add_interest_item`: the name is checked before any store access.
pub async fn add_interest_core(state: &AppState, admin: ObjectId, raw: &str) -> ApiResult<Response> {
    let name = strip(raw);
    if name.is_empty() {
        return Err(ApiError::from_value_error("name is empty"));
    }
    let c = coll(state, INTEREST);
    if c.find_one(doc! {"name": name}).await.map_err(internal)?.is_some() {
        return Err(ApiError::from_value_error(format!("interest item '{name}' already exists")));
    }
    let item = InterestItem { name: name.to_owned(), added_by: Some(admin), added_at: dt::now() };
    c.insert_one(item.to_doc(ObjectId::new())).await.map_err(internal)?;
    invalidate_interest_cache();
    Ok((StatusCode::CREATED, PydanticJson(interest_view(&item))).into_response())
}

pub async fn add_interest(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, NameBody>,
) -> Response {
    add_interest_core(&state, admin.id, &body.name).await.unwrap_or_else(IntoResponse::into_response)
}

pub async fn remove_interest(State(state): State<AppState>, _: Superuser, Segment(name): Segment<3>) -> Response {
    let run = async {
        let c = coll(&state, INTEREST);
        let Some(found) = c.find_one(doc! {"name": &name}).await.map_err(internal)? else {
            return Err(ApiError::not_found(format!("No interest item named '{name}'")));
        };
        InterestItem::from_doc(&found)?;
        let id = found.get("_id").cloned().unwrap_or(Bson::Null);
        c.delete_one(doc! {"_id": id}).await.map_err(internal)?;
        invalidate_interest_cache();
        Ok(no_content())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}

pub const REPLACE_EMPTY: &str = "names list is empty (refusing to wipe the collection)";

/// `admin_replace_interest_items`.
pub async fn replace_interest_core(state: &AppState, admin: ObjectId, names: &[String]) -> ApiResult<Value> {
    let clean = core::clean_names(names);
    if clean.is_empty() {
        return Err(ApiError::bad_request(REPLACE_EMPTY));
    }
    let c = coll(state, INTEREST);
    let before = c.count_documents(doc! {}).await.map_err(internal)?;
    c.delete_many(doc! {}).await.map_err(internal)?;
    let docs: Vec<Document> = clean
        .iter()
        .map(|n| InterestItem { name: n.clone(), added_by: Some(admin), added_at: dt::now() }.to_doc(ObjectId::new()))
        .collect();
    c.insert_many(docs).await.map_err(internal)?;
    invalidate_interest_cache();
    Ok(json!({"removed": before, "added": clean.len()}))
}

pub async fn replace_interest(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, ItemsBody>,
) -> Response {
    match replace_interest_core(&state, admin.id, &body.items).await {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

// --- categories ----------------------------------------------------------------------

/// `MarketCategoryAdminView`.
pub fn category_view(d: &Category) -> Value {
    json!({
        "id": d.id.to_hex(),
        "name": d.name,
        "order": d.order,
        "items": core::sorted_lower(&d.items),
        "created_at": dt::pydantic(&d.created_at),
    })
}

async fn load_categories(state: &AppState) -> ApiResult<Vec<Category>> {
    all(&coll(state, CATEGORIES), None).await?.iter().map(Category::from_doc).collect()
}

/// Beanie's `save()`: `$set` of the whole document, upserting.
async fn save(state: &AppState, d: &Category) -> ApiResult<()> {
    coll(state, CATEGORIES)
        .update_one(doc! {"_id": d.id}, doc! {"$set": d.to_doc()})
        .upsert(true)
        .await
        .map_err(internal)?;
    Ok(())
}

/// `MarketItemCategory.get(to_oid(id))`.
async fn get_category(state: &AppState, id: &str) -> ApiResult<Option<Category>> {
    let Ok(oid) = ObjectId::parse_str(id) else { return Ok(None) };
    match coll(state, CATEGORIES).find_one(doc! {"_id": oid}).await.map_err(internal)? {
        Some(d) => Category::from_doc(&d).map(Some),
        None => Ok(None),
    }
}

fn no_such_category() -> ApiError {
    ApiError::not_found("No such category")
}

pub async fn list_categories(State(state): State<AppState>, _: Superuser) -> ApiResult<PydanticJson> {
    let docs = load_categories(&state).await?;
    let keys: Vec<(i64, &str)> = docs.iter().map(|d| (d.order, d.name.as_str())).collect();
    let views: Vec<Value> = core::category_order(&keys).into_iter().map(|i| category_view(&docs[i])).collect();
    Ok(PydanticJson(json!({"categories": views, "count": docs.len()})))
}

/// `admin_create_category`.
pub async fn create_category_core(state: &AppState, admin: ObjectId, raw: &str) -> ApiResult<Response> {
    let name = strip(raw);
    if name.is_empty() {
        return Err(ApiError::from_value_error("name is empty"));
    }
    let c = coll(state, CATEGORIES);
    if c.find_one(doc! {"name": name}).await.map_err(internal)?.is_some() {
        return Err(ApiError::from_value_error(format!("category '{name}' already exists")));
    }
    let existing = load_categories(state).await?;
    let order = existing.iter().map(|d| d.order).max().unwrap_or(-1) + 1;
    let cat = Category {
        id: ObjectId::new(),
        name: name.to_owned(),
        order,
        items: Vec::new(),
        created_by: Some(admin),
        created_at: dt::now(),
    };
    c.insert_one(cat.to_doc()).await.map_err(internal)?;
    Ok((StatusCode::CREATED, PydanticJson(category_view(&cat))).into_response())
}

pub async fn create_category(
    State(state): State<AppState>,
    DepsThenBody(Superuser(admin), body): DepsThenBody<Superuser, NameBody>,
) -> Response {
    create_category_core(&state, admin.id, &body.name).await.unwrap_or_else(IntoResponse::into_response)
}

/// `admin_update_category`.
pub async fn update_category_core(state: &AppState, id: &str, body: &UpdateBody) -> ApiResult<Value> {
    let Some(mut cat) = get_category(state, id).await? else { return Err(no_such_category()) };
    if let Some(raw) = &body.name {
        let name = strip(raw);
        if name.is_empty() {
            return Err(ApiError::from_value_error("name is empty"));
        }
        if name != cat.name && coll(state, CATEGORIES).find_one(doc! {"name": name}).await.map_err(internal)?.is_some()
        {
            return Err(ApiError::from_value_error(format!("category '{name}' already exists")));
        }
        cat.name = name.to_owned();
    }
    if let Some(items) = &body.items {
        cat.items = core::clean_items(items);
    }
    save(state, &cat).await?;
    Ok(category_view(&cat))
}

pub async fn update_category(
    State(state): State<AppState>,
    Segment(id): Segment<4>,
    DepsThenBody(_, body): DepsThenBody<Superuser, UpdateBody>,
) -> Response {
    match update_category_core(&state, &id, &body).await {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// `admin_delete_category`.
pub async fn delete_category_core(state: &AppState, id: &str) -> ApiResult<Response> {
    let Some(cat) = get_category(state, id).await? else { return Err(no_such_category()) };
    coll(state, CATEGORIES).delete_one(doc! {"_id": cat.id}).await.map_err(internal)?;
    Ok(no_content())
}

pub async fn delete_category(State(state): State<AppState>, _: Superuser, Segment(id): Segment<4>) -> Response {
    delete_category_core(&state, &id).await.unwrap_or_else(IntoResponse::into_response)
}

/// `admin_reorder_categories`: the number of saves.
pub async fn reorder_core(state: &AppState, ids: &[String]) -> ApiResult<usize> {
    let oids: Vec<Option<ObjectId>> = ids.iter().map(|i| ObjectId::parse_str(i).ok()).collect();
    let mut docs = load_categories(state).await?;
    let keys: Vec<(ObjectId, i64, &str)> = docs.iter().map(|d| (d.id, d.order, d.name.as_str())).collect();
    let saves = core::reorder(&keys, &oids);
    for &(i, pos) in &saves {
        docs[i].order = pos;
        save(state, &docs[i]).await?;
    }
    Ok(saves.len())
}

pub async fn reorder_categories(
    State(state): State<AppState>,
    DepsThenBody(_, body): DepsThenBody<Superuser, IdsBody>,
) -> Response {
    match reorder_core(&state, &body.ids).await {
        Ok(n) => PydanticJson(json!({"reordered": n})).into_response(),
        Err(e) => e.into_response(),
    }
}
