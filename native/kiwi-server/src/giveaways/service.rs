//! `app/giveaways/service.py`: the vault, the giveaway lifecycle, entries,
//! the draw and the winner email, call for call over [`Store`].
//!
//! In-memory values follow Beanie: `insert()` keeps microseconds, `save()`
//! reads the stored document back (millisecond datetimes).

use std::collections::{HashMap, HashSet};

use chrono::{DateTime, Duration, Utc};
use mongodb::bson::{Bson, Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    body::{GiveawayCreate, GiveawayUpdate, VaultCodeUpdate, VaultCodesAdd, VaultItemCreate, VaultItemUpdate},
    model::{
        CodeStatus, ENTRIES, Entry, GIVEAWAYS, Giveaway, GiveawayStatus, Invalid, PRIZE_CODES, PrizeCode, VAULT_ITEMS,
        VaultItem, entry_fields, outbox_fields,
    },
    store::{DbResult, Store, by_id, is_in},
    views::{self, Counts},
};
use crate::{
    auth::models::{SITE_USERS, SiteUser, USERS},
    error::ApiError,
};

pub type Res<T> = Result<T, ApiError>;

pub const OUTBOX: &str = "email_outbox";
const NO_CODES: &str = "That drawer has no available codes - add some first.";

fn db<T>(r: DbResult<T>) -> Res<T> {
    r.map_err(|e| {
        tracing::error!("giveaways: {e:#}");
        ApiError::internal()
    })
}

fn valid<T>(r: Result<T, Invalid>) -> Res<T> {
    r.map_err(|e| {
        tracing::error!("giveaways: {e}");
        ApiError::internal()
    })
}

/// `_oid`: `PydanticObjectId(value)`, a 400 when bson rejects it. A
/// 24-character value `bytes.fromhex` shortens by skipping whitespace is a
/// short ObjectId pymongo cannot encode: the unhandled 500.
pub fn parse_oid(value: &str) -> Res<ObjectId> {
    let invalid = || ApiError::bad_request("Invalid id");
    if value.chars().count() != 24 {
        return Err(invalid());
    }
    let mut bytes = Vec::with_capacity(12);
    let mut it = value.chars().peekable();
    while let Some(c) = it.next() {
        if matches!(c, ' ' | '\t' | '\n' | '\r' | '\x0b' | '\x0c') {
            continue;
        }
        let (Some(hi), Some(lo)) = (c.to_digit(16), it.next().and_then(|d| d.to_digit(16))) else {
            return Err(invalid());
        };
        bytes.push((hi * 16 + lo) as u8);
    }
    match <[u8; 12]>::try_from(bytes) {
        Ok(b) => Ok(ObjectId::from_bytes(b)),
        Err(_) => Err(ApiError::internal()),
    }
}

async fn get_doc<S: Store>(s: &S, coll: &'static str, id: ObjectId) -> Res<Option<Document>> {
    db(s.find_one(coll, by_id(id)).await)
}

pub async fn get_item<S: Store>(s: &S, id: ObjectId) -> Res<Option<VaultItem>> {
    match get_doc(s, VAULT_ITEMS, id).await? {
        Some(d) => valid(VaultItem::from_doc(&d, s.now())).map(Some),
        None => Ok(None),
    }
}

pub async fn get_code<S: Store>(s: &S, id: ObjectId) -> Res<Option<PrizeCode>> {
    match get_doc(s, PRIZE_CODES, id).await? {
        Some(d) => valid(PrizeCode::from_doc(&d, s.now())).map(Some),
        None => Ok(None),
    }
}

pub async fn get_giveaway<S: Store>(s: &S, id: ObjectId) -> Res<Option<Giveaway>> {
    match get_doc(s, GIVEAWAYS, id).await? {
        Some(d) => valid(Giveaway::from_doc(&d, s.now())).map(Some),
        None => Ok(None),
    }
}

async fn find_giveaways<S: Store>(
    s: &S,
    filter: Document,
    sort: Option<Document>,
    limit: Option<i64>,
) -> Res<Vec<Giveaway>> {
    let docs = db(s.find(GIVEAWAYS, filter, sort, limit).await)?;
    docs.iter().map(|d| valid(Giveaway::from_doc(d, s.now()))).collect()
}

async fn find_codes<S: Store>(s: &S, filter: Document, sort: Option<Document>) -> Res<Vec<PrizeCode>> {
    let docs = db(s.find(PRIZE_CODES, filter, sort, None).await)?;
    docs.iter().map(|d| valid(PrizeCode::from_doc(d, s.now()))).collect()
}

async fn find_entries<S: Store>(s: &S, filter: Document) -> Res<Vec<Entry>> {
    let docs = db(s.find(ENTRIES, filter, None, None).await)?;
    docs.iter().map(|d| valid(Entry::from_doc(d, s.now()))).collect()
}

async fn save_item<S: Store>(s: &S, item: &mut VaultItem) -> Res<()> {
    db(s.save(VAULT_ITEMS, item.id, item.fields()).await)?;
    item.round_trip();
    Ok(())
}

async fn save_code<S: Store>(s: &S, code: &mut PrizeCode) -> Res<()> {
    db(s.save(PRIZE_CODES, code.id, code.fields()).await)?;
    code.round_trip();
    Ok(())
}

async fn save_giveaway<S: Store>(s: &S, g: &mut Giveaway) -> Res<()> {
    db(s.save(GIVEAWAYS, g.id, g.fields()).await)?;
    g.round_trip();
    Ok(())
}

async fn entry_count<S: Store>(s: &S, gid: ObjectId) -> Res<i64> {
    Ok(db(s.count(ENTRIES, doc! {"giveaway_id": gid}).await)? as i64)
}

fn drawer_missing() -> ApiError {
    ApiError::not_found("Drawer not found")
}

fn giveaway_missing() -> ApiError {
    ApiError::not_found("Giveaway not found")
}

fn code_missing() -> ApiError {
    ApiError::not_found("Code not found")
}

// ---------------------------------------------------------------------------
// Vault

async fn code_counts<S: Store>(s: &S, item_id: ObjectId) -> Res<Counts> {
    let mut out = Counts::default();
    for c in find_codes(s, doc! {"vault_item_id": item_id}, None).await? {
        out.add(c.status);
    }
    Ok(out)
}

pub async fn list_items<S: Store>(s: &S) -> Res<Value> {
    let docs = db(s.find(VAULT_ITEMS, doc! {}, Some(doc! {"created_at": -1}), None).await)?;
    let mut out = Vec::with_capacity(docs.len());
    for d in &docs {
        let item = valid(VaultItem::from_doc(d, s.now()))?;
        out.push(views::item_view(&item, code_counts(s, item.id).await?));
    }
    Ok(Value::Array(out))
}

pub async fn create_item<S: Store>(s: &S, req: VaultItemCreate) -> Res<Value> {
    let now = s.now();
    let mut item = VaultItem {
        id: ObjectId::from_bytes([0; 12]),
        name: req.name,
        description: req.description,
        created_at: now,
        updated_at: s.now(),
    };
    item.id = db(s.insert(VAULT_ITEMS, item.fields()).await)?.map_err(|_| ApiError::internal())?;
    Ok(views::item_view(&item, Counts::default()))
}

pub async fn update_item<S: Store>(s: &S, item_id: &str, req: VaultItemUpdate) -> Res<Value> {
    let mut item = get_item(s, parse_oid(item_id)?).await?.ok_or_else(drawer_missing)?;
    if let Some(name) = req.name {
        item.name = name;
    }
    if let Some(d) = req.description {
        item.description = Some(d);
    }
    item.updated_at = s.now();
    save_item(s, &mut item).await?;
    Ok(views::item_view(&item, code_counts(s, item.id).await?))
}

pub async fn delete_item<S: Store>(s: &S, item_id: &str) -> Res<()> {
    let iid = parse_oid(item_id)?;
    let item = get_item(s, iid).await?.ok_or_else(drawer_missing)?;
    let in_use =
        db(s.count(PRIZE_CODES, doc! {"$and": [{"vault_item_id": iid}, {"status": {"$ne": "available"}}]}).await)?;
    if in_use > 0 {
        return Err(ApiError::bad_request(
            "This drawer has codes reserved by or awarded through giveaways - cancel those first.",
        ));
    }
    db(s.delete_many(PRIZE_CODES, doc! {"vault_item_id": iid}).await)?;
    db(s.delete_one(VAULT_ITEMS, item.id).await)
}

pub async fn list_codes<S: Store>(s: &S, item_id: &str) -> Res<Value> {
    let iid = parse_oid(item_id)?;
    let codes = find_codes(s, doc! {"vault_item_id": iid}, Some(doc! {"created_at": -1})).await?;
    Ok(Value::Array(codes.iter().map(views::code_view).collect()))
}

/// The cleaning step of `add_codes`: stripped, non-blank, at most 400
/// characters, first occurrence only.
pub fn clean_codes(raw: &[String]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for r in raw {
        let s = kiwi_core::pystr::strip(r);
        if s.is_empty() || s.chars().count() > 400 || seen.contains(s) {
            continue;
        }
        seen.insert(s.to_owned());
        out.push(s.to_owned());
    }
    out
}

pub async fn add_codes<S: Store>(s: &S, item_id: &str, req: VaultCodesAdd) -> Res<Value> {
    let iid = parse_oid(item_id)?;
    if get_item(s, iid).await?.is_none() {
        return Err(drawer_missing());
    }
    let cleaned = clean_codes(&req.codes);
    if cleaned.is_empty() {
        return Err(ApiError::bad_request("No valid codes to add."));
    }
    let existing: HashSet<String> =
        find_codes(s, doc! {"vault_item_id": iid}, None).await?.into_iter().map(|c| c.code).collect();
    let mut docs = Vec::new();
    for code in cleaned.iter().filter(|c| !existing.contains(*c)) {
        let c = PrizeCode {
            id: ObjectId::from_bytes([0; 12]),
            vault_item_id: iid,
            code: code.clone(),
            status: CodeStatus::Available,
            giveaway_id: None,
            awarded_to: None,
            awarded_at: None,
            created_at: s.now(),
        };
        docs.push(c.fields());
    }
    let added = docs.len();
    if !docs.is_empty() {
        db(s.insert_many(PRIZE_CODES, docs).await)?;
    }
    Ok(json!({"added": added, "skipped": cleaned.len() - added}))
}

pub async fn update_code<S: Store>(s: &S, code_id: &str, req: VaultCodeUpdate) -> Res<Value> {
    let mut code = get_code(s, parse_oid(code_id)?).await?.ok_or_else(code_missing)?;
    if code.status == CodeStatus::Awarded {
        return Err(ApiError::bad_request("An awarded code can't be edited."));
    }
    code.code = req.code;
    save_code(s, &mut code).await?;
    Ok(views::code_view(&code))
}

pub async fn delete_code<S: Store>(s: &S, code_id: &str) -> Res<()> {
    let code = get_code(s, parse_oid(code_id)?).await?.ok_or_else(code_missing)?;
    if code.status != CodeStatus::Available {
        return Err(ApiError::bad_request(
            "This code is reserved by or awarded through a giveaway - cancel that first.",
        ));
    }
    db(s.delete_one(PRIZE_CODES, code.id).await)
}

// ---------------------------------------------------------------------------
// Giveaways (admin)

async fn item_name<S: Store>(s: &S, item_id: Option<ObjectId>) -> Res<Option<String>> {
    match item_id {
        None => Ok(None),
        Some(id) => Ok(get_item(s, id).await?.map(|i| i.name)),
    }
}

pub async fn list_admin<S: Store>(s: &S) -> Res<Value> {
    let rows = find_giveaways(s, doc! {}, Some(doc! {"created_at": -1}), None).await?;
    let mut out = Vec::with_capacity(rows.len());
    for g in &rows {
        out.push(views::admin_view(g, item_name(s, g.vault_item_id).await?.as_deref()));
    }
    Ok(Value::Array(out))
}

async fn reserve_from_item<S: Store>(s: &S, item_id: ObjectId, giveaway_id: ObjectId) -> Res<PrizeCode> {
    let filter = doc! {"$and": [{"vault_item_id": item_id}, {"status": "available"}]};
    let Some(d) = db(s.find_one(PRIZE_CODES, filter).await)? else {
        return Err(ApiError::bad_request(NO_CODES));
    };
    let mut code = valid(PrizeCode::from_doc(&d, s.now()))?;
    code.status = CodeStatus::Reserved;
    code.giveaway_id = Some(giveaway_id);
    save_code(s, &mut code).await?;
    Ok(code)
}

async fn release_code<S: Store>(s: &S, code_id: Option<ObjectId>) -> Res<()> {
    let Some(id) = code_id else { return Ok(()) };
    if let Some(mut code) = get_code(s, id).await?
        && code.status == CodeStatus::Reserved
    {
        code.status = CodeStatus::Available;
        code.giveaway_id = None;
        save_code(s, &mut code).await?;
    }
    Ok(())
}

pub async fn create<S: Store>(s: &S, req: GiveawayCreate) -> Res<Value> {
    let now = s.now();
    if req.ends_at <= req.starts_at {
        return Err(ApiError::bad_request("End must be after start."));
    }
    if req.ends_at <= now {
        return Err(ApiError::bad_request("End date is in the past."));
    }
    let item = get_item(s, parse_oid(&req.vault_item_id)?).await?.ok_or_else(drawer_missing)?;
    let available =
        db(s.count(PRIZE_CODES, doc! {"$and": [{"vault_item_id": item.id}, {"status": "available"}]}).await)?;
    if available < 1 {
        return Err(ApiError::bad_request(NO_CODES));
    }
    let created_at = s.now();
    let mut g = Giveaway {
        id: ObjectId::from_bytes([0; 12]),
        title: req.title,
        description: req.description.or_else(|| item.description.clone()),
        prize_name: item.name.clone(),
        starts_at: req.starts_at,
        ends_at: req.ends_at,
        status: if req.starts_at <= now { GiveawayStatus::Open } else { GiveawayStatus::Scheduled },
        vault_item_id: Some(item.id),
        prize_code_id: None,
        winner_user_id: None,
        winner_username: None,
        drawn_at: None,
        entry_count: 0,
        created_at,
        updated_at: s.now(),
    };
    g.id = db(s.insert(GIVEAWAYS, g.fields()).await)?.map_err(|_| ApiError::internal())?;
    let code = reserve_from_item(s, item.id, g.id).await?;
    g.prize_code_id = Some(code.id);
    save_giveaway(s, &mut g).await?;
    Ok(views::admin_view(&g, Some(&item.name)))
}

/// `update`'s re-derivation of scheduled/open from the (possibly new) window.
pub fn rederive(
    status: GiveawayStatus,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    now: DateTime<Utc>,
) -> GiveawayStatus {
    match status {
        GiveawayStatus::Scheduled | GiveawayStatus::Open => {
            if starts_at <= now && now < ends_at {
                GiveawayStatus::Open
            } else if starts_at > now {
                GiveawayStatus::Scheduled
            } else {
                status
            }
        }
        other => other,
    }
}

pub async fn update<S: Store>(s: &S, giveaway_id: &str, req: GiveawayUpdate) -> Res<Value> {
    let mut g = get_giveaway(s, parse_oid(giveaway_id)?).await?.ok_or_else(giveaway_missing)?;
    if g.finished() {
        return Err(ApiError::bad_request("A finished giveaway can't be edited."));
    }
    if let Some(t) = req.title {
        g.title = t;
    }
    if let Some(d) = req.description {
        g.description = Some(d);
    }
    if let Some(t) = req.starts_at {
        g.starts_at = t;
    }
    if let Some(t) = req.ends_at {
        g.ends_at = t;
    }
    if g.ends_at <= g.starts_at {
        return Err(ApiError::bad_request("End must be after start."));
    }
    if let Some(vid) = req.vault_item_id
        && Some(vid.as_str()) != g.vault_item_id.map(|o| o.to_hex()).as_deref()
    {
        let item = get_item(s, parse_oid(&vid)?).await?.ok_or_else(drawer_missing)?;
        release_code(s, g.prize_code_id).await?;
        let code = reserve_from_item(s, item.id, g.id).await?;
        g.vault_item_id = Some(item.id);
        g.prize_code_id = Some(code.id);
        g.prize_name = item.name;
    }
    let now = s.now();
    g.status = rederive(g.status, g.starts_at, g.ends_at, now);
    g.updated_at = now;
    save_giveaway(s, &mut g).await?;
    let name = item_name(s, g.vault_item_id).await?;
    Ok(views::admin_view(&g, name.as_deref()))
}

pub async fn cancel<S: Store>(s: &S, giveaway_id: &str) -> Res<Value> {
    let mut g = get_giveaway(s, parse_oid(giveaway_id)?).await?.ok_or_else(giveaway_missing)?;
    if matches!(g.status, GiveawayStatus::Drawn | GiveawayStatus::Closed) {
        return Err(ApiError::bad_request("This giveaway has already finished."));
    }
    release_code(s, g.prize_code_id).await?;
    g.status = GiveawayStatus::Cancelled;
    g.updated_at = s.now();
    save_giveaway(s, &mut g).await?;
    let name = item_name(s, g.vault_item_id).await?;
    Ok(views::admin_view(&g, name.as_deref()))
}

pub async fn draw_now<S: Store>(s: &S, giveaway_id: &str) -> Res<Value> {
    let mut g = get_giveaway(s, parse_oid(giveaway_id)?).await?.ok_or_else(giveaway_missing)?;
    if !matches!(g.status, GiveawayStatus::Open | GiveawayStatus::Scheduled) {
        return Err(ApiError::bad_request("Only an open/scheduled giveaway can be drawn."));
    }
    draw(s, &mut g).await?;
    let name = item_name(s, g.vault_item_id).await?;
    Ok(views::admin_view(&g, name.as_deref()))
}

// ---------------------------------------------------------------------------
// Public + entries

fn public(rows: &[Giveaway]) -> Vec<Value> {
    rows.iter().map(views::public_view).collect()
}

pub async fn list_ongoing<S: Store>(s: &S) -> Res<Vec<Value>> {
    Ok(public(&find_giveaways(s, doc! {"status": "open"}, Some(doc! {"ends_at": 1}), None).await?))
}

pub async fn list_upcoming<S: Store>(s: &S) -> Res<Vec<Value>> {
    Ok(public(&find_giveaways(s, doc! {"status": "scheduled"}, Some(doc! {"starts_at": 1}), None).await?))
}

pub async fn list_ended<S: Store>(s: &S, days: i64) -> Res<Vec<Value>> {
    let cutoff = s.now() - Duration::days(days.max(1));
    let filter =
        doc! {"$and": [is_in("status", ["drawn", "closed"]), {"ends_at": {"$gte": crate::dt::to_bson(cutoff)}}]};
    Ok(public(&find_giveaways(s, filter, Some(doc! {"ends_at": -1}), None).await?))
}

/// `list_public` (the `/site/giveaways` proxy): open, upcoming, then the 50
/// most recent past draws.
pub async fn list_public<S: Store>(s: &S) -> Res<Vec<Value>> {
    let past = find_giveaways(s, is_in("status", ["drawn", "closed"]), Some(doc! {"ends_at": -1}), Some(50)).await?;
    let mut out = list_ongoing(s).await?;
    out.extend(list_upcoming(s).await?);
    out.extend(public(&past));
    Ok(out)
}

pub async fn my_entry_ids<S: Store>(s: &S, user: &SiteUser) -> Res<Value> {
    let rows = find_entries(s, doc! {"site_user_id": user.id}).await?;
    Ok(json!({"giveaway_ids": rows.iter().map(|r| r.giveaway_id.to_hex()).collect::<Vec<_>>()}))
}

pub async fn my_participations<S: Store>(s: &S, user: &SiteUser) -> Res<Value> {
    let entries = find_entries(s, doc! {"site_user_id": user.id}).await?;
    if entries.is_empty() {
        return Ok(json!([]));
    }
    let mut gids: Vec<ObjectId> = Vec::new();
    for e in &entries {
        if !gids.contains(&e.giveaway_id) {
            gids.push(e.giveaway_id);
        }
    }
    let giveaways: HashMap<ObjectId, Giveaway> =
        find_giveaways(s, is_in("_id", gids), None, None).await?.into_iter().map(|g| (g.id, g)).collect();
    let mut won_codes: HashMap<Option<ObjectId>, String> = HashMap::new();
    for c in find_codes(s, doc! {"awarded_to": user.id}, None).await? {
        won_codes.insert(c.giveaway_id, c.code);
    }
    let mut out = Vec::new();
    for e in &entries {
        let Some(g) = giveaways.get(&e.giveaway_id) else { continue };
        let won = g.status == GiveawayStatus::Drawn && g.winner_user_id == Some(user.id);
        let code = if won { won_codes.get(&Some(g.id)).map(String::as_str) } else { None };
        out.push(views::mine(g, e.entered_at, won, code));
    }
    views::sort_mine(&mut out);
    Ok(Value::Array(out.into_iter().map(|m| m.view).collect()))
}

/// `user.display_name or user.username`.
pub fn entrant_name(user: &SiteUser) -> &str {
    user.display_name.as_deref().filter(|d| !d.is_empty()).unwrap_or(&user.username)
}

pub async fn enter<S: Store>(s: &S, user: &SiteUser, giveaway_id: &str) -> Res<Value> {
    let gid = parse_oid(giveaway_id)?;
    let mut g = get_giveaway(s, gid).await?.ok_or_else(giveaway_missing)?;
    let now = s.now();
    if !(g.status == GiveawayStatus::Open && g.starts_at <= now && now < g.ends_at) {
        return Err(ApiError::bad_request("This giveaway isn't open for entries."));
    }
    let fields = entry_fields(gid, user.id, entrant_name(user), s.now());
    if db(s.insert(ENTRIES, fields).await)?.is_err() {
        return Ok(views::enter_view(giveaway_id, entry_count(s, gid).await?));
    }
    let count = entry_count(s, gid).await?;
    g.entry_count = count;
    g.updated_at = now;
    save_giveaway(s, &mut g).await?;
    Ok(views::enter_view(giveaway_id, count))
}

// ---------------------------------------------------------------------------
// Draw + winner email

/// The worker tick: open scheduled giveaways whose start passed, then draw
/// open ones whose end passed (a failed draw is logged and skipped).
pub async fn run_due<S: Store>(s: &S) -> Res<()> {
    let now = s.now();
    let at = Bson::DateTime(crate::dt::to_bson(now));
    let starting =
        find_giveaways(s, doc! {"$and": [{"status": "scheduled"}, {"starts_at": {"$lte": at.clone()}}]}, None, None)
            .await?;
    for mut g in starting {
        g.status = GiveawayStatus::Open;
        g.updated_at = now;
        save_giveaway(s, &mut g).await?;
    }
    let ending = find_giveaways(s, doc! {"$and": [{"status": "open"}, {"ends_at": {"$lte": at}}]}, None, None).await?;
    for mut g in ending {
        if let Err(e) = draw(s, &mut g).await {
            tracing::warn!("draw failed for giveaway {}: {e}", g.id.to_hex());
        }
    }
    Ok(())
}

/// The winner's opt-in address (`SiteUser.get(...).notify_email`).
async fn notify_email<S: Store>(s: &S, id: ObjectId) -> Res<Option<String>> {
    let doc = get_doc(s, SITE_USERS, id).await?;
    Ok(doc.and_then(|d| d.get_str("notify_email").ok().filter(|e| !e.is_empty()).map(str::to_owned)))
}

/// `_draw`: a uniformly random entrant wins the reserved code; no entrants
/// closes the giveaway and frees the code.
pub async fn draw<S: Store>(s: &S, g: &mut Giveaway) -> Res<()> {
    let entries = find_entries(s, doc! {"giveaway_id": g.id}).await?;
    let now = s.now();
    if entries.is_empty() {
        g.status = GiveawayStatus::Closed;
        g.updated_at = now;
        save_giveaway(s, g).await?;
        release_code(s, g.prize_code_id).await?;
        tracing::info!("Giveaway {} closed - no entrants", g.id.to_hex());
        return Ok(());
    }
    let winner = &entries[s.pick(entries.len())];
    let to = notify_email(s, winner.site_user_id).await?;
    let mut code = match g.prize_code_id {
        Some(id) => get_code(s, id).await?,
        None => None,
    };
    g.status = GiveawayStatus::Drawn;
    g.winner_user_id = Some(winner.site_user_id);
    g.winner_username = Some(winner.username.clone());
    g.drawn_at = Some(now);
    g.updated_at = now;
    save_giveaway(s, g).await?;
    if let Some(c) = code.as_mut()
        && c.status == CodeStatus::Reserved
    {
        c.status = CodeStatus::Awarded;
        c.awarded_to = Some(winner.site_user_id);
        c.awarded_at = Some(now);
        save_code(s, c).await?;
    }
    if let (Some(c), Some(to)) = (&code, to.as_deref())
        && let Err(e) = email_winner(s, to, g, &c.code).await
    {
        tracing::warn!("winner email failed for giveaway {}: {e}", g.id.to_hex());
    }
    tracing::info!("Giveaway {} drawn - winner={}", g.id.to_hex(), winner.username);
    Ok(())
}

async fn email_winner<S: Store>(s: &S, to: &str, g: &Giveaway, code: &str) -> Res<()> {
    let (subject, text, html) = views::winner_email(g, code);
    queue_email(s, to, &subject, &text, &html).await
}

/// `app/core/email_outbox.py` `queue_email`.
pub async fn queue_email<S: Store>(s: &S, to: &str, subject: &str, text: &str, html: &str) -> Res<()> {
    if !s.email_enabled() {
        tracing::warn!("SMTP not configured - not queueing email to {to} ({subject})");
        return Ok(());
    }
    let bounced = doc! {"$and": [{"email": to.to_lowercase()}, {"email_bounced": true}]};
    if db(s.find_one(USERS, bounced).await)?.is_some() {
        tracing::warn!("Skipping email to bounced address {to} ({subject})");
        return Ok(());
    }
    let fields = outbox_fields(to, subject, text, html, s.now(), s.now());
    db(s.insert(OUTBOX, fields).await)?.map_err(|_| ApiError::internal())?;
    Ok(())
}
