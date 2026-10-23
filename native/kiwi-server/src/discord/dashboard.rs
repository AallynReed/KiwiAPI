//! `app/bot/router.py`: the site user's "Discord Bot" dashboard under
//! `/v1/site-auth/discord`. Every route goes through [`Deps`] (Discord REST,
//! the stores, the clock), so tests run the real logic against fakes.

use std::collections::HashSet;

use axum::{
    Router,
    extract::{FromRequest, FromRequestParts, Request, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post, put},
};
use chrono::{DateTime, Utc};
use futures::TryStreamExt;
use indexmap::IndexMap;
use mongodb::bson::{Document, doc, oid::ObjectId};
use serde_json::{Value, json};

use super::{
    announcements, contexts,
    models::{
        CLUB_RANKS, CLUBS, Club, GUILD_CONFIGS, GuildConfig, MAX_CLUBS_PER_GUILD, MarketWatchItem, demote_rank,
        promote_rank, rank_label, reconcile, reconcile_club,
    },
    rest::{self, MemberCtx, PERM_ADMINISTRATOR, PERM_MANAGE_GUILD, Rest, RestError, Snapshot},
};
use crate::{
    admin::path_params,
    auth::{CurrentSiteUser, models::SiteUser},
    embed_templates::EmbedTemplate,
    error::{ApiError, ErrorCode, method_not_allowed},
    i18n,
    pyjson::PydanticJson,
    site_auth::oauth::{DISCORD_TOK_PREFIX, clear_discord_token},
    state::AppState,
    validation::{BodyInput, Field, FromBody, FromParams, Params, read_body},
};

const PREFIX: &str = "/v1/site-auth/discord";

/// `CAPABILITIES`.
pub const CAPABILITIES: [(&str, &str); 3] = [
    ("manage_announcements", "Configure announcements (enable types + set channels)"),
    ("manage_ping_roles", "Choose which roles announcements ping"),
    ("manage_clubs", "Create and edit the server's Trove clubs"),
];

const INVITE_PERMS: u64 = 5084151364439105;

type Res = Result<Value, ApiError>;

/// What the routes need besides the request.
pub trait Deps: Rest {
    fn now(&self) -> DateTime<Utc>;
    fn invite_url(&self) -> Option<String>;
    fn reconnect_url(&self) -> Option<String>;
    fn discord_token(&self, user: ObjectId) -> impl Future<Output = Result<Option<String>, ApiError>> + Send;
    fn clear_token(&self, user: ObjectId) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn find_config(&self, guild_id: i64) -> impl Future<Output = Result<Option<Document>, ApiError>> + Send;
    fn find_configs(&self, ids: &[i64]) -> impl Future<Output = Result<Vec<Document>, ApiError>> + Send;
    /// Beanie `save()`: `$set` of the whole document, upserting.
    fn save_config(&self, d: Document) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn insert_config(&self, d: Document) -> impl Future<Output = Result<ObjectId, ApiError>> + Send;
    fn clubs(&self, guild_id: i64) -> impl Future<Output = Result<Vec<Document>, ApiError>> + Send;
    fn count_clubs(&self, guild_id: i64) -> impl Future<Output = Result<u64, ApiError>> + Send;
    fn get_club(&self, id: ObjectId) -> impl Future<Output = Result<Option<Document>, ApiError>> + Send;
    fn insert_club(&self, d: Document) -> impl Future<Output = Result<ObjectId, ApiError>> + Send;
    fn save_club(&self, d: Document) -> impl Future<Output = Result<(), ApiError>> + Send;
    fn delete_club(&self, id: ObjectId) -> impl Future<Output = Result<(), ApiError>> + Send;
    /// `atype.current_anchor()`; `Err` is any exception.
    fn anchor(&self, key: &str) -> impl Future<Output = Result<Option<String>, ()>> + Send;
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("discord dashboard: {e}");
    ApiError::internal()
}

/// A REST failure as the route reports it: `DiscordRestError` -> 502.
fn discord_err(e: RestError) -> ApiError {
    match e {
        RestError::Rest(m) => {
            ApiError::new(StatusCode::BAD_GATEWAY, ErrorCode::InternalError, format!("Couldn't reach Discord: {m}"))
        }
        RestError::Other(m) => internal(m),
    }
}

/// An unwrapped REST failure (no `_discord`): any exception is a 500.
fn raw_err(e: RestError) -> ApiError {
    match e {
        RestError::Rest(m) | RestError::Other(m) => internal(m),
    }
}

fn load_config(d: Option<Document>, now: DateTime<Utc>) -> Result<Option<GuildConfig>, ApiError> {
    d.map(|d| GuildConfig::from_doc(&d, now)).transpose().map_err(internal)
}

async fn save_cfg<D: Deps>(deps: &D, existing: bool, cfg: &mut GuildConfig, user: &SiteUser) -> Result<(), ApiError> {
    cfg.updated_by = user.discord_id;
    cfg.updated_at = deps.now();
    if existing {
        deps.save_config(cfg.to_doc()).await
    } else {
        cfg.id = Some(deps.insert_config(cfg.to_doc()).await?);
        Ok(())
    }
}

/// `str(value).isdigit()` then `int(value)`.
fn digit_id(value: &str) -> Option<i64> {
    if !kiwi_core::pynum::is_decimal(value) {
        return None;
    }
    kiwi_core::pynum::int_of_digits(value).ok()
}

/// `_valid_id`.
fn valid_id(value: Option<&str>, valid: &HashSet<i64>) -> Option<i64> {
    value.filter(|v| !v.is_empty()).and_then(digit_id).filter(|i| valid.contains(i))
}

/// `_valid_ids`.
fn valid_ids(values: &[String], valid: &HashSet<i64>) -> Vec<i64> {
    values.iter().filter_map(|v| digit_id(v)).filter(|i| valid.contains(i)).collect()
}

fn id_set(items: &[Value]) -> Result<HashSet<i64>, ApiError> {
    items.iter().map(|x| rest::py_i64(x.get("id").unwrap_or(&Value::Null)).map_err(raw_err)).collect()
}

/// `_live_ids`: (text channel ids, assignable role ids).
fn live_ids(snap: &Snapshot, guild_id: i64) -> Result<(HashSet<i64>, HashSet<i64>), ApiError> {
    let channels = id_set(&rest::text_channels(&snap.channels).map_err(raw_err)?)?;
    let roles = id_set(&rest::assignable_roles(&snap.roles, guild_id).map_err(raw_err)?)?;
    Ok((channels, roles))
}

async fn member_ctx<D: Deps>(deps: &D, guild_id: i64, user: &SiteUser) -> Result<MemberCtx, ApiError> {
    let Some(uid) = user.discord_id.filter(|i| *i != 0) else {
        return Err(ApiError::forbidden("Link your Discord account (sign in with Discord) to manage the bot."));
    };
    rest::guild_member_context(deps, guild_id, uid)
        .await
        .map_err(discord_err)?
        .ok_or_else(|| ApiError::forbidden("You're not a member of this server (or the bot isn't in it)."))
}

fn has_capability(ctx: &MemberCtx, perms: Option<&IndexMap<String, Vec<i64>>>, cap: &str) -> bool {
    ctx.is_admin || perms.and_then(|p| p.get(cap)).is_some_and(|ids| ids.iter().any(|r| ctx.role_ids.contains(r)))
}

async fn snapshot<D: Deps>(deps: &D, guild_id: i64) -> Result<Snapshot, ApiError> {
    rest::guild_snapshot(deps, guild_id)
        .await
        .map_err(discord_err)?
        .ok_or_else(|| ApiError::forbidden("The bot isn't in this server (or Discord is unreachable)."))
}

fn str_id(v: Option<i64>) -> Value {
    match v {
        Some(i) if i != 0 => Value::String(i.to_string()),
        _ => Value::Null,
    }
}

fn str_ids(v: &[i64]) -> Vec<String> {
    v.iter().map(i64::to_string).collect()
}

/// `_detail_payload`.
pub fn detail_payload(guild_id: i64, ctx: &MemberCtx, cfg: Option<&GuildConfig>, snap: &Snapshot) -> Res {
    let perms = cfg.map(|c| &c.config_perms);
    let can_manage = has_capability(ctx, perms, "manage_announcements");
    let can_ping = has_capability(ctx, perms, "manage_ping_roles");
    let can_clubs = has_capability(ctx, perms, "manage_clubs");
    let preflight = |enabled: bool, channel: Option<i64>| -> Result<Value, ApiError> {
        match channel.filter(|c| enabled && *c != 0) {
            Some(c) => rest::preflight_for(&snap.guild, &snap.me, &snap.channels, c).map_err(raw_err),
            None => Ok(Value::Null),
        }
    };
    let mut anns = Vec::new();
    let mut announcing = false;
    for a in &announcements::TYPES {
        let s = cfg.and_then(|c| c.announcements.get(a.key));
        let enabled = s.is_some_and(|s| s.enabled);
        announcing |= enabled;
        let channel = s.and_then(|s| s.channel_id);
        anns.push(json!({
            "key": a.key,
            "label": a.label,
            "description": a.description,
            "category": a.category,
            "enabled": enabled,
            "channel_id": str_id(channel),
            "ping_role_ids": s.map(|s| str_ids(&s.ping_role_ids)).unwrap_or_default(),
            "channel_missing": s.is_some_and(|s| s.channel_missing),
            "customizable": contexts::has_customization(a.key),
            "template": s.and_then(|s| s.template.as_ref()).map_or(Value::Null, EmbedTemplate::to_value),
            "preflight": preflight(enabled, channel)?,
        }));
    }
    let board = cfg.map(|c| &c.live_board);
    let mw = cfg.map(|c| &c.market_watch);
    let language = cfg.and_then(|c| i18n::supported(&c.language)).unwrap_or("en");
    let languages: Vec<Value> = i18n::LANGS.iter().map(|(c, l)| json!({"code": c, "label": l})).collect();
    let mut out = json!({
        "guild_id": guild_id.to_string(),
        "is_admin": ctx.is_admin,
        "can_manage_announcements": can_manage,
        "can_manage_ping_roles": can_ping,
        "can_manage_clubs": can_clubs,
        "language": language,
        "languages": languages,
        "channels": rest::text_channels(&snap.channels).map_err(raw_err)?,
        "roles": rest::assignable_roles(&snap.roles, guild_id).map_err(raw_err)?,
        "announcements": anns,
        "announcing": announcing,
        "live_board": {
            "enabled": board.is_some_and(|b| b.enabled),
            "channel_id": str_id(board.and_then(|b| b.channel_id)),
            "channel_missing": board.is_some_and(|b| b.channel_missing),
            "preflight": preflight(board.is_some_and(|b| b.enabled), board.and_then(|b| b.channel_id))?,
        },
        "market_watch": {
            "enabled": mw.is_some_and(|m| m.enabled),
            "channel_id": str_id(mw.and_then(|m| m.channel_id)),
            "ping_role_ids": mw.map(|m| str_ids(&m.ping_role_ids)).unwrap_or_default(),
            "channel_missing": mw.is_some_and(|m| m.channel_missing),
            "items": mw.map(|m| m.items.iter().map(|it| json!({"name": it.name, "max_price_each": it.max_price_each})).collect::<Vec<_>>()).unwrap_or_default(),
        },
    });
    if ctx.is_admin
        && let Value::Object(m) = &mut out
    {
        m.insert(
            "capabilities".into(),
            Value::Array(CAPABILITIES.iter().map(|(k, l)| json!({"key": k, "label": l})).collect()),
        );
        let mut p = serde_json::Map::new();
        for (k, _) in CAPABILITIES {
            p.insert(k.into(), json!(perms.and_then(|x| x.get(k)).map(|v| str_ids(v)).unwrap_or_default()));
        }
        m.insert("permissions".into(), Value::Object(p));
    }
    Ok(out)
}

// --- request bodies --------------------------------------------------------------------

#[derive(Default)]
pub struct AnnouncementUpdate {
    pub enabled: bool,
    pub channel_id: Option<String>,
    pub ping_role_ids: Vec<String>,
}

pub struct AnnouncementsUpdate(pub Vec<(String, AnnouncementUpdate)>);

impl FromBody for AnnouncementsUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(AnnouncementsUpdate(
                m.field("announcements")
                    .dict(|f| {
                        f.model(|m| AnnouncementUpdate {
                            enabled: m.field("enabled").bool().default(false),
                            channel_id: m.field("channel_id").str().optional(),
                            ping_role_ids: m.field("ping_role_ids").list(|f| f.str().required()).default(Vec::new()),
                        })
                        .required()
                    })
                    .required(),
            ))
        })
        .required()
    }
}

pub struct TemplateUpdate(pub Option<EmbedTemplate>);

impl FromBody for TemplateUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            let f = m.field("template");
            let t = match f.input() {
                crate::validation::Input::Missing(_) | crate::validation::Input::Json(Value::Null) => None,
                _ => Some(EmbedTemplate::validate(f)),
            };
            Some(TemplateUpdate(t))
        })
        .required()
    }
}

pub struct LanguageUpdate(pub String);

impl FromBody for LanguageUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(LanguageUpdate(m.field("language").str().required()))).required()
    }
}

pub struct LiveBoardUpdate {
    pub enabled: bool,
    pub channel_id: Option<String>,
}

impl FromBody for LiveBoardUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(LiveBoardUpdate {
                enabled: m.field("enabled").bool().default(false),
                channel_id: m.field("channel_id").str().optional(),
            })
        })
        .required()
    }
}

#[derive(Default)]
pub struct WatchItemUpdate {
    pub name: String,
    pub max_price_each: Option<f64>,
}

pub struct MarketWatchUpdate {
    pub enabled: bool,
    pub channel_id: Option<String>,
    pub ping_role_ids: Vec<String>,
    pub items: Vec<WatchItemUpdate>,
}

impl FromBody for MarketWatchUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(MarketWatchUpdate {
                enabled: m.field("enabled").bool().default(false),
                channel_id: m.field("channel_id").str().optional(),
                ping_role_ids: m.field("ping_role_ids").list(|f| f.str().required()).default(Vec::new()),
                items: m
                    .field("items")
                    .list(|f| {
                        f.model(|m| WatchItemUpdate {
                            name: m.field("name").str().required(),
                            max_price_each: m.field("max_price_each").float().optional(),
                        })
                        .required()
                    })
                    .default(Vec::new()),
            })
        })
        .required()
    }
}

pub struct ClubUpdate {
    pub name: String,
    pub public: bool,
    pub description: Option<String>,
    pub banner_url: Option<String>,
    pub avatar_url: Option<String>,
    pub discord_url: Option<String>,
    pub website_url: Option<String>,
    pub role_links: Vec<(String, String)>,
}

impl FromBody for ClubUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(ClubUpdate {
                name: m.field("name").str().required(),
                public: m.field("public").bool().default(false),
                description: m.field("description").str().optional(),
                banner_url: m.field("banner_url").str().optional(),
                avatar_url: m.field("avatar_url").str().optional(),
                discord_url: m.field("discord_url").str().optional(),
                website_url: m.field("website_url").str().optional(),
                role_links: m.field("role_links").dict(|f| f.str().required()).default(Vec::new()),
            })
        })
        .required()
    }
}

pub struct PermissionsUpdate(pub Vec<(String, Vec<String>)>);

impl FromBody for PermissionsUpdate {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| {
            Some(PermissionsUpdate(
                m.field("permissions").dict(|f| f.list(|f| f.str().required()).required()).required(),
            ))
        })
        .required()
    }
}

// --- path parameters -------------------------------------------------------------------

pub struct GuildPath(pub i64);

impl FromParams for GuildPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        GuildPath(p.path("guild_id").int().required())
    }
}

pub struct KeyPath(pub i64, pub String);

impl FromParams for KeyPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        let g = p.path("guild_id").int().required();
        KeyPath(g, p.path("key").str().required())
    }
}

pub struct ClubPath(pub i64, pub String);

impl FromParams for ClubPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        let g = p.path("guild_id").int().required();
        ClubPath(g, p.path("club_id").str().required())
    }
}

pub struct ActionPath {
    pub guild_id: i64,
    pub club_id: String,
    pub member_id: i64,
    pub action: String,
}

impl FromParams for ActionPath {
    fn from_params(p: &mut Params<'_>) -> Self {
        ActionPath {
            guild_id: p.path("guild_id").int().required(),
            club_id: p.path("club_id").str().required(),
            member_id: p.path("member_id").int().required(),
            action: p.path("action").str().required(),
        }
    }
}

/// Path parameters (Starlette-decoded) as FastAPI validates them.
pub fn validate_path<P: FromParams>(path: Vec<(String, String)>, headers: &HeaderMap) -> Result<P, ApiError> {
    let mut p = Params::new(path, None, headers);
    let value = P::from_params(&mut p);
    Ok(p.v.finish(value)?)
}

/// Path parameters and the body together, path errors first.
pub fn validate_parts<P: FromParams, B: FromBody>(
    path: Vec<(String, String)>,
    headers: &HeaderMap,
    body: &BodyInput,
) -> Result<(P, B), ApiError> {
    let mut p = Params::new(path, None, headers);
    let params = P::from_params(&mut p);
    let mut v = p.v;
    let b = B::from_body(Field::new(&mut v, vec!["body".into()], body.input()));
    match v.finish(b)? {
        Some(b) => Ok((params, b)),
        None => Err(ApiError::internal()),
    }
}

/// The site user, then path parameters, FastAPI's order.
pub struct Authed<P>(pub SiteUser, pub P);

impl<P: FromParams + Send> FromRequestParts<AppState> for Authed<P> {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, state: &AppState) -> Result<Self, Response> {
        let CurrentSiteUser(user) =
            CurrentSiteUser::from_request_parts(parts, state).await.map_err(IntoResponse::into_response)?;
        let value = validate_path(path_params(parts), &parts.headers).map_err(IntoResponse::into_response)?;
        Ok(Authed(user, value))
    }
}

/// A JSON body route: body decoded first, then the site user, then path and
/// body validated together.
pub struct AuthedBody<P, B>(pub SiteUser, pub P, pub B);

impl<P: FromParams + Send, B: FromBody + Send> FromRequest<AppState> for AuthedBody<P, B> {
    type Rejection = Response;

    async fn from_request(req: Request, state: &AppState) -> Result<Self, Response> {
        let (mut parts, body) = req.into_parts();
        let body = read_body(&parts.headers, body).await.map_err(IntoResponse::into_response)?;
        let CurrentSiteUser(user) =
            CurrentSiteUser::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
        let (p, b) = validate_parts(path_params(&parts), &parts.headers, &body).map_err(IntoResponse::into_response)?;
        Ok(AuthedBody(user, p, b))
    }
}

// --- routes ------------------------------------------------------------------------------

pub fn router() -> Router<AppState> {
    let nm = |allow: &'static str| move || async move { method_not_allowed(allow) };
    Router::new()
        .route("/v1/site-auth/discord/guilds", get(list_guilds_h).fallback(nm("GET")))
        .route("/v1/site-auth/discord/guilds/{guild_id}", get(guild_detail_h).fallback(nm("GET")))
        .route("/v1/site-auth/discord/guilds/{guild_id}/announcements", put(set_announcements_h).fallback(nm("PUT")))
        .route("/v1/site-auth/discord/announcement-meta", get(announcement_meta_h).fallback(nm("GET")))
        .route(
            "/v1/site-auth/discord/guilds/{guild_id}/announcements/{key}/template",
            put(set_template_h).fallback(nm("PUT")),
        )
        .route("/v1/site-auth/discord/guilds/{guild_id}/language", put(set_language_h).fallback(nm("PUT")))
        .route("/v1/site-auth/discord/guilds/{guild_id}/live-board", put(set_live_board_h).fallback(nm("PUT")))
        .route("/v1/site-auth/discord/guilds/{guild_id}/market-watch", put(set_market_watch_h).fallback(nm("PUT")))
        .route(
            "/v1/site-auth/discord/guilds/{guild_id}/clubs",
            get(list_clubs_h).post(create_club_h).fallback(nm("GET")),
        )
        .route(
            "/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}",
            put(update_club_h).delete(delete_club_h).fallback(nm("PUT")),
        )
        .route("/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}/roster", get(roster_h).fallback(nm("GET")))
        .route(
            "/v1/site-auth/discord/guilds/{guild_id}/clubs/{club_id}/roster/{member_id}/{action}",
            post(roster_action_h).fallback(nm("POST")),
        )
        .route("/v1/site-auth/discord/guilds/{guild_id}/permissions", put(set_permissions_h).fallback(nm("PUT")))
}

/// For `routes::defer_to_python`: an int path value pydantic takes but `i64`
/// can't hold goes to Python.
pub fn defers(matched: &str, path: &str) -> bool {
    if !matched.starts_with(PREFIX) {
        return false;
    }
    let decoded = percent_encoding::percent_decode_str(path).decode_utf8_lossy();
    matched.split('/').zip(decoded.split('/')).any(|(t, v)| {
        matches!(t, "{guild_id}" | "{member_id}")
            && matches!(crate::validation::str_as_int(v), Ok(i) if i64::try_from(i).is_err())
    })
}

fn reply(r: Res) -> Response {
    match r {
        Ok(v) => PydanticJson(v).into_response(),
        Err(e) => e.into_response(),
    }
}

/// The real dependencies.
pub struct Live<'a> {
    pub state: &'a AppState,
    pub rest: rest::Live,
}

impl<'a> Live<'a> {
    pub fn new(state: &'a AppState) -> Self {
        Live { state, rest: rest::Live::new(state.settings()) }
    }

    fn coll(&self, name: &str) -> mongodb::Collection<Document> {
        self.state.mongo().collection(name)
    }
}

impl Rest for Live<'_> {
    async fn get(&self, path: &str, timeout: u64) -> rest::RResult<Option<Value>> {
        self.rest.get(path, timeout).await
    }
    async fn send(&self, method: &str, path: &str, reason: Option<&str>) -> rest::RResult<()> {
        self.rest.send(method, path, reason).await
    }
    async fn bot_user_id(&self) -> rest::RResult<Option<i64>> {
        self.rest.bot_user_id().await
    }
    async fn user_guilds(&self, token: &str) -> Option<Vec<Value>> {
        self.rest.user_guilds(token).await
    }
}

async fn all(c: mongodb::Cursor<Document>) -> Result<Vec<Document>, ApiError> {
    c.try_collect().await.map_err(internal)
}

impl Deps for Live<'_> {
    fn now(&self) -> DateTime<Utc> {
        crate::dt::now()
    }

    fn invite_url(&self) -> Option<String> {
        let id = self.state.settings().discord_client_id.as_deref().filter(|s| !s.is_empty())?;
        Some(format!("https://discord.com/oauth2/authorize?client_id={id}&scope=bot&permissions={INVITE_PERMS}"))
    }

    fn reconnect_url(&self) -> Option<String> {
        let s = self.state.settings();
        s.discord_oauth_enabled().then(|| format!("{}/v1/site-auth/oauth/discord/start", s.api_url))
    }

    async fn discord_token(&self, user: ObjectId) -> Result<Option<String>, ApiError> {
        let Some(mut conn) = self.state.inner.db.redis.clone() else { return Ok(None) };
        let raw: Option<Vec<u8>> = redis::cmd("GET")
            .arg(format!("{DISCORD_TOK_PREFIX}{}", user.to_hex()))
            .query_async(&mut conn)
            .await
            .map_err(internal)?;
        raw.map(|b| String::from_utf8(b).map_err(internal)).transpose()
    }

    async fn clear_token(&self, user: ObjectId) -> Result<(), ApiError> {
        clear_discord_token(self.state, user).await
    }

    async fn find_config(&self, guild_id: i64) -> Result<Option<Document>, ApiError> {
        self.coll(GUILD_CONFIGS).find_one(doc! {"guild_id": guild_id}).await.map_err(internal)
    }

    async fn find_configs(&self, ids: &[i64]) -> Result<Vec<Document>, ApiError> {
        all(self.coll(GUILD_CONFIGS).find(doc! {"guild_id": {"$in": ids.to_vec()}}).await.map_err(internal)?).await
    }

    async fn save_config(&self, d: Document) -> Result<(), ApiError> {
        let id = d.get_object_id("_id").map_err(internal)?;
        self.coll(GUILD_CONFIGS).update_one(doc! {"_id": id}, doc! {"$set": d}).upsert(true).await.map_err(internal)?;
        Ok(())
    }

    async fn insert_config(&self, d: Document) -> Result<ObjectId, ApiError> {
        let r = self.coll(GUILD_CONFIGS).insert_one(d).await.map_err(internal)?;
        r.inserted_id.as_object_id().ok_or_else(|| internal("inserted id"))
    }

    async fn clubs(&self, guild_id: i64) -> Result<Vec<Document>, ApiError> {
        all(self.coll(CLUBS).find(doc! {"guild_id": guild_id}).sort(doc! {"created_at": 1}).await.map_err(internal)?)
            .await
    }

    async fn count_clubs(&self, guild_id: i64) -> Result<u64, ApiError> {
        self.coll(CLUBS).count_documents(doc! {"guild_id": guild_id}).await.map_err(internal)
    }

    async fn get_club(&self, id: ObjectId) -> Result<Option<Document>, ApiError> {
        self.coll(CLUBS).find_one(doc! {"_id": id}).await.map_err(internal)
    }

    async fn insert_club(&self, d: Document) -> Result<ObjectId, ApiError> {
        let r = self.coll(CLUBS).insert_one(d).await.map_err(internal)?;
        r.inserted_id.as_object_id().ok_or_else(|| internal("inserted id"))
    }

    async fn save_club(&self, d: Document) -> Result<(), ApiError> {
        let id = d.get_object_id("_id").map_err(internal)?;
        self.coll(CLUBS).update_one(doc! {"_id": id}, doc! {"$set": d}).upsert(true).await.map_err(internal)?;
        Ok(())
    }

    async fn delete_club(&self, id: ObjectId) -> Result<(), ApiError> {
        self.coll(CLUBS).delete_one(doc! {"_id": id}).await.map_err(internal)?;
        Ok(())
    }

    async fn anchor(&self, key: &str) -> Result<Option<String>, ()> {
        announcements::current_anchor(self.state, key, crate::dt::now().timestamp_micros()).await.map_err(|e| {
            tracing::debug!("anchor {key}: {e:#}");
        })
    }
}

async fn list_guilds_h(State(state): State<AppState>, CurrentSiteUser(user): CurrentSiteUser) -> Response {
    reply(list_guilds(&Live::new(&state), &user).await)
}

async fn guild_detail_h(State(state): State<AppState>, Authed(user, GuildPath(g)): Authed<GuildPath>) -> Response {
    reply(guild_detail(&Live::new(&state), &user, g).await)
}

async fn set_announcements_h(
    State(state): State<AppState>,
    AuthedBody(user, GuildPath(g), body): AuthedBody<GuildPath, AnnouncementsUpdate>,
) -> Response {
    reply(set_announcements(&Live::new(&state), &user, g, body).await)
}

async fn announcement_meta_h(CurrentSiteUser(_): CurrentSiteUser) -> Response {
    reply(Ok(contexts::announcement_meta(&announcements::keys())))
}

async fn set_template_h(
    State(state): State<AppState>,
    AuthedBody(user, KeyPath(g, key), body): AuthedBody<KeyPath, TemplateUpdate>,
) -> Response {
    reply(set_template(&Live::new(&state), &user, g, &key, body.0).await)
}

async fn set_language_h(
    State(state): State<AppState>,
    AuthedBody(user, GuildPath(g), body): AuthedBody<GuildPath, LanguageUpdate>,
) -> Response {
    reply(set_language(&Live::new(&state), &user, g, &body.0).await)
}

async fn set_live_board_h(
    State(state): State<AppState>,
    AuthedBody(user, GuildPath(g), body): AuthedBody<GuildPath, LiveBoardUpdate>,
) -> Response {
    reply(set_live_board(&Live::new(&state), &user, g, body).await)
}

async fn set_market_watch_h(
    State(state): State<AppState>,
    AuthedBody(user, GuildPath(g), body): AuthedBody<GuildPath, MarketWatchUpdate>,
) -> Response {
    reply(set_market_watch(&Live::new(&state), &user, g, body).await)
}

async fn list_clubs_h(State(state): State<AppState>, Authed(user, GuildPath(g)): Authed<GuildPath>) -> Response {
    reply(list_clubs(&Live::new(&state), &user, g).await)
}

async fn create_club_h(State(state): State<AppState>, Authed(user, GuildPath(g)): Authed<GuildPath>) -> Response {
    reply(create_club(&Live::new(&state), &user, g).await)
}

async fn update_club_h(
    State(state): State<AppState>,
    AuthedBody(user, ClubPath(g, club), body): AuthedBody<ClubPath, ClubUpdate>,
) -> Response {
    reply(update_club(&Live::new(&state), &user, g, &club, body).await)
}

async fn delete_club_h(State(state): State<AppState>, Authed(user, ClubPath(g, club)): Authed<ClubPath>) -> Response {
    reply(delete_club(&Live::new(&state), &user, g, &club).await)
}

async fn roster_h(State(state): State<AppState>, Authed(user, ClubPath(g, club)): Authed<ClubPath>) -> Response {
    reply(roster(&Live::new(&state), &user, g, &club).await)
}

async fn roster_action_h(State(state): State<AppState>, Authed(user, p): Authed<ActionPath>) -> Response {
    reply(roster_action(&Live::new(&state), &user, &p).await)
}

async fn set_permissions_h(
    State(state): State<AppState>,
    AuthedBody(user, GuildPath(g), body): AuthedBody<GuildPath, PermissionsUpdate>,
) -> Response {
    reply(set_permissions(&Live::new(&state), &user, g, body).await)
}

// --- the route logic ----------------------------------------------------------------------

fn perms_int(v: Option<&Value>) -> i128 {
    match v {
        Some(Value::String(s)) => kiwi_core::pynum::int(s).map_or(0, i128::from),
        Some(v) => crate::feeds::pyval::py_int(v).unwrap_or(0),
        None => 0,
    }
}

/// `list_my_guilds`.
pub async fn list_guilds<D: Deps>(deps: &D, user: &SiteUser) -> Res {
    let invite = deps.invite_url();
    let reconnect = deps.reconnect_url();
    let linked = user.discord_id.is_some_and(|i| i != 0);
    let token = if linked { deps.discord_token(user.id).await? } else { None };
    let token = token.filter(|t| !t.is_empty());
    let user_guilds = match &token {
        Some(t) => deps.user_guilds(t).await,
        None => None,
    };
    let Some(user_guilds) = user_guilds else {
        if token.is_some() {
            deps.clear_token(user.id).await?;
        }
        return Ok(json!({
            "linked": linked, "guilds_synced": false, "invite_url": invite, "reconnect_url": reconnect, "guilds": [],
        }));
    };
    let bot_ids: HashSet<i64> = match rest::bot_guilds(deps).await {
        Ok(gs) => id_set(&gs)?,
        Err(RestError::Rest(m)) => {
            tracing::warn!("bot_guilds failed: {m}");
            return Ok(json!({
                "linked": true, "guilds_synced": true, "invite_url": invite, "reconnect_url": reconnect,
                "guilds": [], "error": "Couldn't reach Discord.",
            }));
        }
        Err(RestError::Other(m)) => return Err(internal(m)),
    };
    let uid = user.discord_id.unwrap_or(0);
    let mut ids = Vec::new();
    for g in &user_guilds {
        let gid = rest::py_i64(g.get("id").unwrap_or(&Value::Null)).map_err(raw_err)?;
        ids.push(gid);
    }
    let present: Vec<i64> = ids.iter().copied().filter(|g| bot_ids.contains(g)).collect();
    let ctxs = futures::future::join_all(present.iter().map(|g| rest::guild_member_context(deps, *g, uid))).await;
    let ctx_by_id: Vec<(i64, Option<MemberCtx>)> =
        present.iter().copied().zip(ctxs.into_iter().map(|c| c.ok().flatten())).collect();
    let mut configs: Vec<GuildConfig> = Vec::new();
    if !present.is_empty() {
        let now = deps.now();
        for d in deps.find_configs(&present).await? {
            configs.push(GuildConfig::from_doc(&d, now).map_err(internal)?);
        }
    }
    let mut guilds: Vec<Value> = Vec::new();
    for (g, gid) in user_guilds.iter().zip(&ids) {
        let gid = *gid;
        let oauth_admin = g.get("owner").is_some_and(crate::feeds::pyval::truthy)
            || perms_int(g.get("permissions")) & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD) != 0;
        let name = g.get("name").cloned().ok_or_else(|| internal("guild without name"))?;
        let icon = g.get("icon").cloned().unwrap_or(Value::Null);
        if bot_ids.contains(&gid) {
            let Some(ctx) = ctx_by_id.iter().rev().find(|(i, _)| *i == gid).and_then(|(_, c)| c.clone()) else {
                continue;
            };
            let cfg = configs.iter_mut().rev().find(|c| c.guild_id == gid);
            let (accessible, announcing) = match cfg {
                Some(cfg) => {
                    if cfg.migrate_legacy() {
                        deps.save_config(cfg.to_doc()).await?;
                    }
                    let accessible = ctx.is_admin
                        || cfg.config_perms.values().any(|ids| ids.iter().any(|r| ctx.role_ids.contains(r)));
                    let announcing = cfg.announcements.values().any(|s| s.enabled && !s.channel_missing);
                    (accessible, announcing)
                }
                None => (ctx.is_admin, false),
            };
            if !accessible {
                continue;
            }
            guilds.push(json!({
                "id": gid.to_string(), "name": name, "icon": icon, "bot_present": true,
                "is_admin": ctx.is_admin, "announcing": announcing,
            }));
        } else if oauth_admin {
            guilds.push(json!({
                "id": gid.to_string(), "name": name, "icon": icon, "bot_present": false, "is_admin": true,
                "invite_url": invite.as_ref().map(|i| format!("{i}&guild_id={gid}")),
            }));
        }
    }
    let mut keyed = Vec::with_capacity(guilds.len());
    for g in guilds {
        let lower = g["name"].as_str().ok_or_else(|| internal("guild name is not a str"))?.to_lowercase();
        keyed.push((!g["bot_present"].as_bool().unwrap_or(false), lower, g));
    }
    keyed.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    Ok(json!({
        "linked": true, "guilds_synced": true, "invite_url": invite, "reconnect_url": reconnect,
        "guilds": keyed.into_iter().map(|k| k.2).collect::<Vec<_>>(),
    }))
}

const NO_CONFIG: &str = "You don't have permission to configure the bot in this server.";

/// `guild_detail`.
pub async fn guild_detail<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let mut cfg = load_config(deps.find_config(guild_id).await?, deps.now())?;
    let perms = cfg.as_ref().map(|c| &c.config_perms);
    if !(has_capability(&ctx, perms, "manage_announcements") || has_capability(&ctx, perms, "manage_ping_roles")) {
        return Err(ApiError::forbidden(NO_CONFIG));
    }
    let snap = snapshot(deps, guild_id).await?;
    if let Some(c) = cfg.as_mut() {
        let mut dirty = c.migrate_legacy();
        let channels = id_set(&snap.channels)?;
        let roles = id_set(&snap.roles)?;
        dirty = reconcile(c, &channels, &roles) || dirty;
        if dirty {
            c.updated_at = deps.now();
            deps.save_config(c.to_doc()).await?;
        }
    }
    detail_payload(guild_id, &ctx, cfg.as_ref(), &snap)
}

/// `_load_or_new_config`: (existed, migrated working copy).
async fn load_or_new<D: Deps>(
    deps: &D,
    guild_id: i64,
) -> Result<(Option<IndexMap<String, Vec<i64>>>, GuildConfig), ApiError> {
    let now = deps.now();
    let existing = load_config(deps.find_config(guild_id).await?, now)?;
    let perms = existing.as_ref().map(|c| c.config_perms.clone());
    let mut cfg = existing.unwrap_or_else(|| GuildConfig::new(guild_id, now));
    cfg.migrate_legacy();
    Ok((perms, cfg))
}

/// `set_announcements`.
pub async fn set_announcements<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, body: AnnouncementsUpdate) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let (perms, mut cfg) = load_or_new(deps, guild_id).await?;
    let can_manage = has_capability(&ctx, perms.as_ref(), "manage_announcements");
    let can_ping = has_capability(&ctx, perms.as_ref(), "manage_ping_roles");
    if !(can_manage || can_ping) {
        return Err(ApiError::forbidden(NO_CONFIG));
    }
    let snap = snapshot(deps, guild_id).await?;
    let (live_channels, live_roles) = live_ids(&snap, guild_id)?;
    for (key, upd) in body.0 {
        let Some(atype) = announcements::by_key(&key) else { continue };
        let mut cur = cfg.announcements.get(&key).cloned().unwrap_or_default();
        let new_channel = valid_id(upd.channel_id.as_deref(), &live_channels);
        let mut new_pings = valid_ids(&upd.ping_role_ids, &live_roles);
        new_pings.sort_unstable();
        new_pings.dedup();
        if (upd.enabled != cur.enabled || new_channel != cur.channel_id) && !can_manage {
            return Err(ApiError::forbidden("You can't change announcement channels or toggles in this server."));
        }
        let mut cur_sorted = cur.ping_role_ids.clone();
        cur_sorted.sort_unstable();
        if new_pings != cur_sorted && !can_ping {
            return Err(ApiError::forbidden("You can't change which roles get pinged in this server."));
        }
        if upd.enabled && new_channel.is_none_or(|c| c == 0) {
            return Err(ApiError::bad_request(format!("Pick a channel before enabling {}.", atype.label)));
        }
        if can_manage {
            let (was_enabled, old_channel) = (cur.enabled, cur.channel_id);
            cur.enabled = upd.enabled;
            cur.channel_id = new_channel;
            cur.channel_missing = false;
            if cur.enabled && new_channel.is_some_and(|c| c != 0) && (!was_enabled || old_channel != new_channel) {
                cur.last_anchor = deps.anchor(&key).await.unwrap_or(None);
            }
        }
        if can_ping {
            cur.ping_role_ids = new_pings;
        }
        cfg.announcements.insert(key, cur);
    }
    save_cfg(deps, perms.is_some(), &mut cfg, user).await?;
    detail_payload(guild_id, &ctx, Some(&cfg), &snap)
}

/// `set_announcement_template`.
pub async fn set_template<D: Deps>(
    deps: &D,
    user: &SiteUser,
    guild_id: i64,
    key: &str,
    template: Option<EmbedTemplate>,
) -> Res {
    if announcements::by_key(key).is_none() || !contexts::has_customization(key) {
        return Err(ApiError::not_found("Unknown announcement type."));
    }
    let ctx = member_ctx(deps, guild_id, user).await?;
    let (perms, mut cfg) = load_or_new(deps, guild_id).await?;
    if !has_capability(&ctx, perms.as_ref(), "manage_announcements") {
        return Err(ApiError::forbidden("You can't configure announcements in this server."));
    }
    let mut cur = cfg.announcements.get(key).cloned().unwrap_or_default();
    cur.template = template;
    cfg.announcements.insert(key.to_owned(), cur);
    save_cfg(deps, perms.is_some(), &mut cfg, user).await?;
    let snap = snapshot(deps, guild_id).await?;
    detail_payload(guild_id, &ctx, Some(&cfg), &snap)
}

/// `set_language`.
pub async fn set_language<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, language: &str) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let (perms, mut cfg) = load_or_new(deps, guild_id).await?;
    if !has_capability(&ctx, perms.as_ref(), "manage_announcements") {
        return Err(ApiError::forbidden(NO_CONFIG));
    }
    if i18n::supported(language).is_none() {
        return Err(ApiError::bad_request("Unsupported language."));
    }
    cfg.language = language.to_owned();
    save_cfg(deps, perms.is_some(), &mut cfg, user).await?;
    let snap = snapshot(deps, guild_id).await?;
    detail_payload(guild_id, &ctx, Some(&cfg), &snap)
}

/// `set_live_board`.
pub async fn set_live_board<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, body: LiveBoardUpdate) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let (perms, mut cfg) = load_or_new(deps, guild_id).await?;
    if !has_capability(&ctx, perms.as_ref(), "manage_announcements") {
        return Err(ApiError::forbidden(NO_CONFIG));
    }
    let snap = snapshot(deps, guild_id).await?;
    let (live_channels, _) = live_ids(&snap, guild_id)?;
    let channel = valid_id(body.channel_id.as_deref(), &live_channels);
    if body.enabled && channel.is_none_or(|c| c == 0) {
        return Err(ApiError::bad_request("Pick a channel before enabling the live board."));
    }
    let board = &mut cfg.live_board;
    if channel != board.channel_id {
        board.message_id = None;
    }
    board.enabled = body.enabled;
    board.channel_id = channel;
    board.channel_missing = false;
    save_cfg(deps, perms.is_some(), &mut cfg, user).await?;
    detail_payload(guild_id, &ctx, Some(&cfg), &snap)
}

/// `set_market_watch`.
pub async fn set_market_watch<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, body: MarketWatchUpdate) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let (perms, mut cfg) = load_or_new(deps, guild_id).await?;
    if !has_capability(&ctx, perms.as_ref(), "manage_announcements") {
        return Err(ApiError::forbidden(NO_CONFIG));
    }
    let snap = snapshot(deps, guild_id).await?;
    let (live_channels, live_roles) = live_ids(&snap, guild_id)?;
    let channel = valid_id(body.channel_id.as_deref(), &live_channels);
    if body.enabled && channel.is_none_or(|c| c == 0) {
        return Err(ApiError::bad_request("Pick a channel before enabling market watch."));
    }
    let mw = &mut cfg.market_watch;
    let pings = if has_capability(&ctx, perms.as_ref(), "manage_ping_roles") {
        valid_ids(&body.ping_role_ids, &live_roles)
    } else {
        mw.ping_role_ids.clone()
    };
    let mut prior: IndexMap<String, MarketWatchItem> = IndexMap::new();
    for it in &mw.items {
        prior.insert(it.name.to_lowercase(), it.clone());
    }
    let mut items = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for raw in body.items.iter().take(50) {
        let nm = kiwi_core::pystr::strip(&raw.name).to_owned();
        let low = nm.to_lowercase();
        if nm.is_empty() || seen.contains(&low) {
            continue;
        }
        seen.insert(low.clone());
        let cap = raw.max_price_each.filter(|p| *p > 0.0);
        items.push(MarketWatchItem {
            name: nm,
            max_price_each: cap,
            last_alert_sig: prior.get(&low).and_then(|p| p.last_alert_sig.clone()),
        });
    }
    mw.enabled = body.enabled;
    mw.channel_id = channel;
    mw.ping_role_ids = pings;
    mw.channel_missing = false;
    mw.items = items;
    save_cfg(deps, perms.is_some(), &mut cfg, user).await?;
    detail_payload(guild_id, &ctx, Some(&cfg), &snap)
}

// --- clubs -----------------------------------------------------------------------------

fn club_view(c: &Club) -> Value {
    let links: serde_json::Map<String, Value> =
        c.role_links.iter().map(|(k, v)| (k.clone(), Value::String(v.to_string()))).collect();
    json!({
        "id": c.id.map_or_else(|| "None".to_owned(), |i| i.to_hex()),
        "name": c.name,
        "public": c.public,
        "description": c.description,
        "banner_url": c.banner_url,
        "avatar_url": c.avatar_url,
        "discord_url": c.discord_url,
        "website_url": c.website_url,
        "role_links": links,
    })
}

/// `_club_url`.
fn club_url(value: Option<&str>, label: &str) -> Result<Option<String>, ApiError> {
    let Some(v) = value else { return Ok(None) };
    let v = kiwi_core::pystr::strip(v);
    if v.is_empty() {
        return Ok(None);
    }
    if !(v.starts_with("http://") || v.starts_with("https://")) {
        return Err(ApiError::bad_request(format!("{label} must start with http:// or https://.")));
    }
    if v.chars().count() > 500 {
        return Err(ApiError::bad_request(format!("{label} is too long.")));
    }
    Ok(Some(v.to_owned()))
}

async fn require_clubs<D: Deps>(deps: &D, guild_id: i64, user: &SiteUser) -> Result<MemberCtx, ApiError> {
    let ctx = member_ctx(deps, guild_id, user).await?;
    let cfg = load_config(deps.find_config(guild_id).await?, deps.now())?;
    if !has_capability(&ctx, cfg.as_ref().map(|c| &c.config_perms), "manage_clubs") {
        return Err(ApiError::forbidden("You don't have permission to manage clubs in this server."));
    }
    Ok(ctx)
}

async fn get_club<D: Deps>(deps: &D, guild_id: i64, club_id: &str) -> Result<Club, ApiError> {
    let missing = || ApiError::not_found("Club not found.");
    let Ok(id) = ObjectId::parse_str(club_id) else { return Err(missing()) };
    let club = match deps.get_club(id).await {
        Ok(Some(d)) => Club::from_doc(&d, deps.now()).ok(),
        _ => None,
    };
    club.filter(|c| c.guild_id == guild_id).ok_or_else(missing)
}

fn ranks_json() -> Value {
    Value::Array(CLUB_RANKS.iter().map(|r| json!({"key": r, "label": rank_label(r)})).collect())
}

/// `list_clubs`.
pub async fn list_clubs<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64) -> Res {
    require_clubs(deps, guild_id, user).await?;
    let snap = snapshot(deps, guild_id).await?;
    let live_roles = id_set(&snap.roles)?;
    let now = deps.now();
    let mut clubs = Vec::new();
    for d in deps.clubs(guild_id).await? {
        clubs.push(Club::from_doc(&d, now).map_err(internal)?);
    }
    for c in &mut clubs {
        if reconcile_club(c, &live_roles) {
            c.updated_at = deps.now();
            deps.save_club(c.to_doc()).await?;
        }
    }
    Ok(json!({
        "guild_id": guild_id.to_string(),
        "max_clubs": MAX_CLUBS_PER_GUILD,
        "ranks": ranks_json(),
        "roles": rest::assignable_roles(&snap.roles, guild_id).map_err(raw_err)?,
        "clubs": clubs.iter().map(club_view).collect::<Vec<_>>(),
    }))
}

/// `create_club`.
pub async fn create_club<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64) -> Res {
    require_clubs(deps, guild_id, user).await?;
    if deps.count_clubs(guild_id).await? >= MAX_CLUBS_PER_GUILD {
        return Err(ApiError::bad_request(format!("A server can have at most {MAX_CLUBS_PER_GUILD} clubs.")));
    }
    let mut club = Club::new(guild_id, "New Club", user.discord_id, deps.now());
    club.id = Some(deps.insert_club(club.to_doc()).await?);
    Ok(club_view(&club))
}

/// `update_club`.
pub async fn update_club<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, club_id: &str, body: ClubUpdate) -> Res {
    require_clubs(deps, guild_id, user).await?;
    let mut club = get_club(deps, guild_id, club_id).await?;
    let name = kiwi_core::pystr::strip(&body.name).to_owned();
    if name.is_empty() {
        return Err(ApiError::bad_request("Club name is required."));
    }
    let snap = snapshot(deps, guild_id).await?;
    let (_, live_roles) = live_ids(&snap, guild_id)?;
    let mut links = IndexMap::new();
    for (rank, rid) in &body.role_links {
        if CLUB_RANKS.contains(&rank.as_str())
            && let Some(id) = digit_id(rid).filter(|i| live_roles.contains(i))
        {
            links.insert(rank.clone(), id);
        }
    }
    club.name = kiwi_core::pystr::prefix(&name, 100).to_owned();
    club.public = body.public;
    let desc = kiwi_core::pystr::prefix(kiwi_core::pystr::strip(body.description.as_deref().unwrap_or("")), 1000);
    club.description = (!desc.is_empty()).then(|| desc.to_owned());
    club.banner_url = club_url(body.banner_url.as_deref(), "Banner URL")?;
    club.avatar_url = club_url(body.avatar_url.as_deref(), "Profile picture URL")?;
    club.discord_url = club_url(body.discord_url.as_deref(), "Discord link")?;
    club.website_url = club_url(body.website_url.as_deref(), "Website link")?;
    club.role_links = links;
    club.updated_by = user.discord_id;
    club.updated_at = deps.now();
    deps.save_club(club.to_doc()).await?;
    Ok(club_view(&club))
}

/// `delete_club`.
pub async fn delete_club<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, club_id: &str) -> Res {
    require_clubs(deps, guild_id, user).await?;
    let club = get_club(deps, guild_id, club_id).await?;
    if let Some(id) = club.id {
        deps.delete_club(id).await?;
    }
    Ok(json!({"deleted": true, "id": club_id}))
}

/// `_member_view`.
fn member_view(m: &Value) -> Value {
    let user = match m.get("user") {
        Some(u) if crate::feeds::pyval::truthy(u) => u.clone(),
        _ => json!({}),
    };
    let truthy = |v: Option<&Value>| v.filter(|x| crate::feeds::pyval::truthy(x)).cloned();
    let name = truthy(m.get("nick"))
        .or_else(|| truthy(user.get("global_name")))
        .or_else(|| truthy(user.get("username")))
        .unwrap_or_else(|| json!("Unknown"));
    let s = crate::feeds::pyval::py_str;
    let avatar = match (truthy(user.get("avatar")), truthy(user.get("id"))) {
        (Some(a), Some(id)) => json!(format!("https://cdn.discordapp.com/avatars/{}/{}.png?size=64", s(&id), s(&a))),
        _ => Value::Null,
    };
    json!({"id": s(user.get("id").unwrap_or(&Value::Null)), "name": name, "avatar": avatar})
}

fn rank_entry(rank: &str, linked: &HashSet<String>) -> Value {
    let up = promote_rank(rank, linked);
    let down = demote_rank(rank, linked);
    json!({
        "key": rank, "label": rank_label(rank),
        "promote_to": up, "promote_label": up.map(rank_label),
        "demote_to": down, "demote_label": down.map(rank_label),
    })
}

fn current_rank(held: &HashSet<i64>, links: &IndexMap<String, i64>) -> Option<&'static str> {
    CLUB_RANKS.iter().copied().find(|r| links.get(*r).is_some_and(|id| *id != 0 && held.contains(id)))
}

fn held_roles(m: &Value) -> Result<HashSet<i64>, ApiError> {
    match m.get("roles") {
        Some(Value::Array(a)) => a.iter().map(|r| rest::py_i64(r).map_err(raw_err)).collect(),
        _ => Ok(HashSet::new()),
    }
}

fn is_bot(m: &Value) -> bool {
    m.get("user").and_then(|u| u.get("bot")).is_some_and(crate::feeds::pyval::truthy)
}

/// `club_roster`.
pub async fn roster<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, club_id: &str) -> Res {
    require_clubs(deps, guild_id, user).await?;
    let club = get_club(deps, guild_id, club_id).await?;
    let linked: HashSet<String> = club.role_links.keys().cloned().collect();
    let ranks: Vec<Value> =
        CLUB_RANKS.iter().filter(|r| club.role_links.contains_key(**r)).map(|r| rank_entry(r, &linked)).collect();
    if ranks.is_empty() {
        return Ok(json!({"available": true, "ranks": [], "roster": {}}));
    }
    let members = match rest::guild_members(deps, guild_id).await {
        Ok(m) => m,
        Err(RestError::Rest(m)) => {
            tracing::warn!("guild_members failed: {m}");
            let hint = if m.contains("403") {
                "Enable the Server Members Intent for the bot (Discord Developer Portal → Bot → Privileged Gateway Intents) to load the roster."
            } else {
                "Couldn't load members right now."
            };
            return Ok(json!({"available": false, "ranks": ranks, "roster": {}, "error": hint}));
        }
        Err(e) => return Err(raw_err(e)),
    };
    let mut roster = serde_json::Map::new();
    for rk in &ranks {
        roster.insert(rk["key"].as_str().unwrap_or_default().to_owned(), json!([]));
    }
    for m in &members {
        if is_bot(m) {
            continue;
        }
        if let Some(rank) = current_rank(&held_roles(m)?, &club.role_links)
            && let Some(Value::Array(list)) = roster.get_mut(rank)
        {
            list.push(member_view(m));
        }
    }
    Ok(json!({"available": true, "ranks": ranks, "roster": roster}))
}

/// `club_roster_action`.
pub async fn roster_action<D: Deps>(deps: &D, user: &SiteUser, p: &ActionPath) -> Res {
    let promote = match p.action.as_str() {
        "promote" => true,
        "demote" => false,
        _ => return Err(ApiError::not_found("Unknown roster action.")),
    };
    require_clubs(deps, p.guild_id, user).await?;
    let club = get_club(deps, p.guild_id, &p.club_id).await?;
    let linked: HashSet<String> = club.role_links.keys().cloned().collect();
    if linked.is_empty() {
        return Err(ApiError::bad_request("This club has no rank roles linked yet."));
    }
    let member = rest::guild_member(deps, p.guild_id, p.member_id)
        .await
        .map_err(raw_err)?
        .ok_or_else(|| ApiError::not_found("That member is no longer in this server."))?;
    if is_bot(&member) {
        return Err(ApiError::bad_request("Bots can't hold a club rank."));
    }
    let held = held_roles(&member)?;
    let current = current_rank(&held, &club.role_links)
        .ok_or_else(|| ApiError::bad_request("This member doesn't hold any of the club's rank roles yet."))?;
    let target = if promote { promote_rank(current, &linked) } else { demote_rank(current, &linked) };
    let Some(target) = target else {
        return Err(ApiError::conflict(if promote {
            "There's no higher rank to promote to."
        } else {
            "There's no lower rank to demote to."
        }));
    };
    let target_role = club.role_links[target];
    let reason = format!("Kiwi club {}: {current} -> {target} (dashboard)", p.action);
    let applied: rest::RResult<()> = async {
        if !held.contains(&target_role) {
            rest::add_member_role(deps, p.guild_id, p.member_id, target_role, &reason).await?;
        }
        for rank in CLUB_RANKS {
            if let Some(&rid) = club.role_links.get(rank)
                && rid != 0
                && rid != target_role
                && held.contains(&rid)
            {
                rest::remove_member_role(deps, p.guild_id, p.member_id, rid, &reason).await?;
            }
        }
        Ok(())
    }
    .await;
    match applied {
        Ok(()) => {}
        Err(RestError::Rest(m)) => {
            let hint = if m.contains("403") {
                "The bot needs the Manage Roles permission, and its own role must sit above the club's rank roles in the server's role list.".to_owned()
            } else {
                format!("Discord wouldn't apply the change: {m}")
            };
            return Err(ApiError::new(StatusCode::BAD_GATEWAY, ErrorCode::ServiceUnavailable, hint));
        }
        Err(e) => return Err(raw_err(e)),
    }
    Ok(json!({
        "ok": true, "action": p.action, "member": member_view(&member)["name"],
        "from": current, "from_label": rank_label(current),
        "to": target, "to_label": rank_label(target),
    }))
}

/// `set_permissions`.
pub async fn set_permissions<D: Deps>(deps: &D, user: &SiteUser, guild_id: i64, body: PermissionsUpdate) -> Res {
    let ctx = member_ctx(deps, guild_id, user).await?;
    if !ctx.is_admin {
        return Err(ApiError::forbidden(
            "Only the server owner or Manage-Server admins can change who can configure the bot.",
        ));
    }
    let live = id_set(&rest::guild_assignable_roles(deps, guild_id).await.map_err(discord_err)?)?;
    let mut cleaned: IndexMap<String, Vec<i64>> = IndexMap::new();
    for (cap, ids) in body.0 {
        if !CAPABILITIES.iter().any(|(k, _)| *k == cap) {
            continue;
        }
        let mut valid = valid_ids(&ids, &live);
        valid.sort_unstable();
        valid.dedup();
        if !valid.is_empty() {
            cleaned.insert(cap, valid);
        }
    }
    let existing = load_config(deps.find_config(guild_id).await?, deps.now())?;
    let existed = existing.is_some();
    let mut cfg = existing.unwrap_or_else(|| GuildConfig::new(guild_id, deps.now()));
    cfg.config_perms = cleaned.clone();
    save_cfg(deps, existed, &mut cfg, user).await?;
    let mut out = serde_json::Map::new();
    for (k, v) in &cleaned {
        out.insert(k.clone(), json!(str_ids(v)));
    }
    Ok(json!({"saved": true, "permissions": out}))
}
