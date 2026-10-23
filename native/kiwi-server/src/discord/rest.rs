//! `app/bot/discord_rest.py` (the dashboard's bot-token REST reads, role
//! writes and the pure permission math) and `site_auth.oauth.fetch_discord_guilds`.
//! Every call goes through [`Rest`], so tests answer from a fake.

use std::{collections::HashSet, fmt::Display, sync::OnceLock, time::Duration};

use serde_json::{Map, Value, json};

use crate::{config::Settings, feeds::pyval, middleware::pyurl::quote, pyjson};

pub const API: &str = "https://discord.com/api/v10";
const OAUTH_API: &str = "https://discord.com/api";
const HTTPX_UA: &str = "python-httpx/0.28.1";

pub const PERM_ADMINISTRATOR: i128 = 1 << 3;
pub const PERM_MANAGE_GUILD: i128 = 1 << 5;
pub const PERM_VIEW_CHANNEL: i128 = 1 << 10;
pub const PERM_SEND_MESSAGES: i128 = 1 << 11;
pub const PERM_EMBED_LINKS: i128 = 1 << 14;
const ALL: i128 = !0;

const ANNOUNCE_PERMS: [(&str, i128); 3] =
    [("View Channel", PERM_VIEW_CHANNEL), ("Send Messages", PERM_SEND_MESSAGES), ("Embed Links", PERM_EMBED_LINKS)];

/// What a REST helper raises.
#[derive(Clone, Debug, PartialEq)]
pub enum RestError {
    /// `DiscordRestError(message)`.
    Rest(String),
    /// Any other exception (a bad payload): unhandled in Python.
    Other(String),
}

impl RestError {
    pub fn other(what: impl std::fmt::Display) -> RestError {
        RestError::Other(what.to_string())
    }
}

pub type RResult<T> = Result<T, RestError>;

/// A Python `int` path value: an `i64`, or the digits of one past it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Snowflake {
    Id(i64),
    Big(String),
}

impl Snowflake {
    /// From a value pydantic already took as an `int`.
    pub fn parse(raw: &str) -> Snowflake {
        if let Ok(v) = crate::validation::str_as_int(raw)
            && let Ok(v) = i64::try_from(v)
        {
            return Snowflake::Id(v);
        }
        let t = raw.trim();
        let t = t.split_once('.').map_or(t, |(whole, _)| whole);
        let (neg, digits) = match t.strip_prefix('-') {
            Some(d) => (true, d),
            None => (false, t.strip_prefix('+').unwrap_or(t)),
        };
        let digits: String = digits.chars().filter(|&c| c != '_').collect();
        let digits = digits.trim_start_matches('0');
        Snowflake::Big(match (digits.is_empty(), neg) {
            (true, _) => "0".to_owned(),
            (false, true) => format!("-{digits}"),
            (false, false) => digits.to_owned(),
        })
    }
}

impl Display for Snowflake {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Snowflake::Id(i) => write!(f, "{i}"),
            Snowflake::Big(s) => f.write_str(s),
        }
    }
}

/// The HTTP layer: a bot-token GET (404 is `None`), a body-less write, the
/// cached bot user id and the user-token guild list.
pub trait Rest: Sync {
    fn get(&self, path: &str, timeout: u64) -> impl Future<Output = RResult<Option<Value>>> + Send;
    fn send(&self, method: &str, path: &str, reason: Option<&str>) -> impl Future<Output = RResult<()>> + Send;
    fn bot_user_id(&self) -> impl Future<Output = RResult<Option<i64>>> + Send;
    /// `fetch_discord_guilds(access_token)`.
    fn user_guilds(&self, token: &str) -> impl Future<Output = Option<Vec<Value>>> + Send;
}

/// The real Discord API over reqwest.
pub struct Live {
    token: Option<String>,
}

impl Live {
    pub fn new(s: &Settings) -> Live {
        Live { token: s.discord_bot_token.clone().filter(|t| !t.is_empty()) }
    }

    fn token(&self) -> RResult<&str> {
        self.token.as_deref().ok_or_else(|| RestError::Rest("DISCORD_BOT_TOKEN is not configured.".into()))
    }
}

fn client(timeout: u64) -> RResult<reqwest::Client> {
    crate::http::client_builder().timeout(Duration::from_secs(timeout)).build().map_err(RestError::other)
}

fn unreachable_err(e: impl std::fmt::Display) -> RestError {
    RestError::Rest(format!("Could not reach Discord: {e}"))
}

fn head200(body: &[u8]) -> String {
    kiwi_core::pystr::prefix(&String::from_utf8_lossy(body), 200).to_owned()
}

static BOT_ID: OnceLock<i64> = OnceLock::new();

impl Rest for Live {
    async fn get(&self, path: &str, timeout: u64) -> RResult<Option<Value>> {
        let token = self.token()?;
        let resp = client(timeout)?
            .get(format!("{API}{path}"))
            .header("Authorization", format!("Bot {token}"))
            .header("User-Agent", HTTPX_UA)
            .send()
            .await
            .map_err(unreachable_err)?;
        let status = resp.status().as_u16();
        if status == 404 {
            return Ok(None);
        }
        let body = resp.bytes().await.map_err(unreachable_err)?;
        if status >= 300 {
            return Err(RestError::Rest(format!("GET {path} -> {status}: {}", head200(&body))));
        }
        pyjson::loads(&body).map(Some).map_err(|e| RestError::Other(format!("{e:?}")))
    }

    async fn send(&self, method: &str, path: &str, reason: Option<&str>) -> RResult<()> {
        let token = self.token()?;
        let m = reqwest::Method::from_bytes(method.as_bytes()).map_err(RestError::other)?;
        let mut req = client(15)?
            .request(m, format!("{API}{path}"))
            .header("Authorization", format!("Bot {token}"))
            .header("User-Agent", HTTPX_UA);
        if let Some(r) = reason.filter(|r| !r.is_empty()) {
            req = req.header("X-Audit-Log-Reason", quote(kiwi_core::pystr::prefix(r, 400), ""));
        }
        let resp = req.send().await.map_err(unreachable_err)?;
        let status = resp.status().as_u16();
        if status >= 300 {
            let body = resp.bytes().await.map_err(unreachable_err)?;
            return Err(RestError::Rest(format!("{method} {path} -> {status}: {}", head200(&body))));
        }
        Ok(())
    }

    async fn bot_user_id(&self) -> RResult<Option<i64>> {
        if let Some(id) = BOT_ID.get() {
            return Ok(Some(*id));
        }
        let me = self.get("/users/@me", 15).await?;
        let id = me.as_ref().and_then(|m| m.get("id")).filter(|v| pyval::truthy(v));
        match id {
            Some(v) => {
                let id = py_i64(v)?;
                Ok(Some(*BOT_ID.get_or_init(|| id)))
            }
            None => Ok(None),
        }
    }

    async fn user_guilds(&self, token: &str) -> Option<Vec<Value>> {
        if token.is_empty() {
            return None;
        }
        let resp = client(10)
            .ok()?
            .get(format!("{OAUTH_API}/users/@me/guilds"))
            .header("Authorization", format!("Bearer {token}"))
            .header("User-Agent", HTTPX_UA)
            .send()
            .await
            .ok()?;
        if resp.status().as_u16() != 200 {
            return None;
        }
        let body = resp.bytes().await.ok()?;
        normalize_user_guilds(&pyjson::loads(&body).ok()?)
    }
}

/// `fetch_discord_guilds`' normalization of Discord's answer.
pub fn normalize_user_guilds(raw: &Value) -> Option<Vec<Value>> {
    let Value::Array(items) = raw else { return None };
    let mut out = Vec::new();
    for g in items {
        let Value::Object(m) = g else { continue };
        let Some(id) = m.get("id").filter(|v| pyval::truthy(v)) else { continue };
        out.push(json!({
            "id": pyval::py_str(id),
            "name": m.get("name").cloned().unwrap_or_else(|| json!("")),
            "icon": m.get("icon").cloned().unwrap_or(Value::Null),
            "owner": m.get("owner").is_some_and(pyval::truthy),
            "permissions": pyval::py_str(m.get("permissions").unwrap_or(&json!("0"))),
        }));
    }
    Some(out)
}

/// `int(v)` of a Discord id or bitfield.
pub fn py_i64(v: &Value) -> RResult<i64> {
    pyval::py_int(v).and_then(|i| i64::try_from(i).ok()).ok_or_else(|| RestError::other(format!("int() of {v}")))
}

fn py_i128(v: &Value) -> RResult<i128> {
    pyval::py_int(v).ok_or_else(|| RestError::other(format!("int() of {v}")))
}

fn key<'a>(v: &'a Value, k: &str) -> RResult<&'a Value> {
    v.get(k).ok_or_else(|| RestError::other(format!("KeyError: {k}")))
}

fn list<'a>(v: &'a Value, k: &str) -> &'a [Value] {
    match v.get(k) {
        Some(Value::Array(a)) => a,
        _ => &[],
    }
}

// --- pure permission math ----------------------------------------------------------

/// `guild_permissions`.
pub fn guild_permissions(guild: &Value, member: &Value) -> RResult<i128> {
    if py_i128(key(key(member, "user")?, "id")?)? == py_i128(key(guild, "owner_id")?)? {
        return Ok(ALL);
    }
    let mut roles: Vec<(i128, i128)> = Vec::new();
    for r in list(guild, "roles") {
        roles.push((py_i128(key(r, "id")?)?, py_i128(key(r, "permissions")?)?));
    }
    let perm_of = |id: i128| roles.iter().rev().find(|(r, _)| *r == id).map_or(0, |(_, p)| *p);
    let mut perms = perm_of(py_i128(key(guild, "id")?)?);
    for rid in list(member, "roles") {
        perms |= perm_of(py_i128(rid)?);
    }
    Ok(if perms & PERM_ADMINISTRATOR != 0 { ALL } else { perms })
}

/// `effective_channel_permissions`.
pub fn effective_channel_permissions(guild: &Value, member: &Value, channel: &Value) -> RResult<i128> {
    let base = guild_permissions(guild, member)?;
    if base == ALL {
        return Ok(ALL);
    }
    let guild_id = py_i128(key(guild, "id")?)?;
    let member_id = py_i128(key(key(member, "user")?, "id")?)?;
    let member_roles = list(member, "roles").iter().map(py_i128).collect::<RResult<Vec<_>>>()?;
    let mut overwrites: Vec<(i128, &Value)> = Vec::new();
    for o in list(channel, "permission_overwrites") {
        overwrites.push((py_i128(key(o, "id")?)?, o));
    }
    let find = |id: i128| overwrites.iter().rev().find(|(k, _)| *k == id).map(|(_, o)| *o);
    let bits = |o: &Value, k: &str| -> RResult<i128> { py_i128(key(o, k)?) };
    let mut perms = base;
    if let Some(ow) = find(guild_id).filter(|o| pyval::truthy(o)) {
        perms = (perms & !bits(ow, "deny")?) | bits(ow, "allow")?;
    }
    let (mut allow, mut deny) = (0i128, 0i128);
    for rid in member_roles {
        if let Some(ow) = find(rid).filter(|o| pyval::truthy(o)) {
            allow |= bits(ow, "allow")?;
            deny |= bits(ow, "deny")?;
        }
    }
    perms = (perms & !deny) | allow;
    if let Some(ow) = find(member_id).filter(|o| pyval::truthy(o)) {
        perms = (perms & !bits(ow, "deny")?) | bits(ow, "allow")?;
    }
    Ok(perms)
}

fn int_key(v: &Value, k: &str, default: i64) -> Value {
    v.get(k).cloned().unwrap_or_else(|| json!(default))
}

/// Python's ordering of the sort keys these lists use (ints, floats, bools).
fn sort_key(v: &Value) -> f64 {
    match v {
        Value::Bool(b) => f64::from(u8::from(*b)),
        Value::Number(n) => n.as_f64().unwrap_or(0.0),
        _ => 0.0,
    }
}

/// `ch.get("type") in {0, 5}`.
fn is_text_type(v: Option<&Value>) -> bool {
    match v {
        Some(Value::Number(n)) => n.as_f64().is_some_and(|f| f == 0.0 || f == 5.0),
        Some(Value::Bool(b)) => !b,
        _ => false,
    }
}

/// `text_channels`: GUILD_TEXT / GUILD_ANNOUNCEMENT channels by position.
pub fn text_channels(channels: &[Value]) -> RResult<Vec<Value>> {
    let mut out = Vec::new();
    for ch in channels.iter().filter(|ch| is_text_type(ch.get("type"))) {
        out.push(json!({
            "id": pyval::py_str(key(ch, "id")?),
            "name": ch.get("name").cloned().unwrap_or_else(|| json!("")),
            "position": int_key(ch, "position", 0),
        }));
    }
    out.sort_by(|a, b| sort_key(&a["position"]).total_cmp(&sort_key(&b["position"])));
    Ok(out)
}

/// `assignable_roles`: no @everyone, no managed roles, highest first.
pub fn assignable_roles(roles: &[Value], guild_id: i64) -> RResult<Vec<Value>> {
    let gid = guild_id.to_string();
    let mut out = Vec::new();
    for r in roles {
        let id = pyval::py_str(key(r, "id")?);
        if id == gid || r.get("managed").is_some_and(pyval::truthy) {
            continue;
        }
        out.push(json!({
            "id": id,
            "name": key(r, "name")?.clone(),
            "color": int_key(r, "color", 0),
            "position": int_key(r, "position", 0),
        }));
    }
    out.sort_by(|a, b| sort_key(&b["position"]).total_cmp(&sort_key(&a["position"])));
    Ok(out)
}

/// `preflight_for`: the announce permissions the bot lacks in `channel_id`.
pub fn preflight_for(guild: &Value, me: &Value, channels: &[Value], channel_id: i64) -> RResult<Value> {
    let want = channel_id.to_string();
    let channel = channels.iter().find(|ch| pyval::py_str(ch.get("id").unwrap_or(&Value::Null)) == want);
    let Some(channel) = channel else {
        return Ok(json!({"ok": false, "missing": [], "error": "That channel no longer exists."}));
    };
    let perms = effective_channel_permissions(guild, me, channel)?;
    let missing: Vec<&str> = ANNOUNCE_PERMS.iter().filter(|(_, bit)| perms & bit == 0).map(|(n, _)| *n).collect();
    Ok(json!({"ok": missing.is_empty(), "missing": missing}))
}

// --- REST calls ----------------------------------------------------------------------

/// `bot_guilds`: `[{id, name, icon}]`.
pub async fn bot_guilds<R: Rest>(r: &R) -> RResult<Vec<Value>> {
    let data = r.get("/users/@me/guilds", 15).await?;
    let items: &[Value] = match &data {
        Some(Value::Array(a)) => a,
        Some(v) if pyval::truthy(v) => return Err(RestError::other("bot guilds is not a list")),
        _ => &[],
    };
    items
        .iter()
        .map(|g| {
            Ok(json!({
                "id": pyval::py_str(key(g, "id")?),
                "name": g.get("name").cloned().unwrap_or_else(|| json!("")),
                "icon": g.get("icon").cloned().unwrap_or(Value::Null),
            }))
        })
        .collect()
}

/// `guild_member_context`'s result.
#[derive(Clone, Debug, PartialEq)]
pub struct MemberCtx {
    pub is_owner: bool,
    pub is_admin: bool,
    pub role_ids: HashSet<i64>,
}

/// `guild_member_context`: `None` when they aren't a member. `guild_id` is any
/// Python int (`Snowflake` for one past `i64`).
pub async fn guild_member_context<R: Rest, G: Display + Sync>(
    r: &R,
    guild_id: G,
    user_id: i64,
) -> RResult<Option<MemberCtx>> {
    let guild = r.get(&format!("/guilds/{guild_id}"), 15).await?;
    let member = r.get(&format!("/guilds/{guild_id}/members/{user_id}"), 15).await?;
    let (Some(guild), Some(member)) = (guild.filter(pyval::truthy), member.filter(pyval::truthy)) else {
        return Ok(None);
    };
    let perms = guild_permissions(&guild, &member)?;
    let owner = py_i128(key(&guild, "owner_id")?)? == i128::from(user_id);
    let role_ids = list(&member, "roles").iter().map(py_i64).collect::<RResult<HashSet<_>>>()?;
    Ok(Some(MemberCtx { is_owner: owner, is_admin: perms & (PERM_ADMINISTRATOR | PERM_MANAGE_GUILD) != 0, role_ids }))
}

fn as_list(v: Option<Value>) -> RResult<Vec<Value>> {
    match v {
        Some(Value::Array(a)) => Ok(a),
        Some(v) if pyval::truthy(&v) => Err(RestError::other("not a list")),
        _ => Ok(Vec::new()),
    }
}

/// `guild_assignable_roles`.
pub async fn guild_assignable_roles<R: Rest>(r: &R, guild_id: i64) -> RResult<Vec<Value>> {
    let roles = as_list(r.get(&format!("/guilds/{guild_id}/roles"), 15).await?)?;
    assignable_roles(&roles, guild_id)
}

/// `guild_members`: every member, 1000 a page by ascending id.
pub async fn guild_members<R: Rest>(r: &R, guild_id: i64) -> RResult<Vec<Value>> {
    let mut out = Vec::new();
    let mut after = "0".to_owned();
    for _ in 0..50 {
        let batch = as_list(r.get(&format!("/guilds/{guild_id}/members?limit=1000&after={after}"), 20).await?)?;
        if batch.is_empty() {
            break;
        }
        let n = batch.len();
        let mut max: Option<i128> = None;
        for m in &batch {
            let id = py_i128(key(key(m, "user")?, "id")?)?;
            max = Some(max.map_or(id, |x| x.max(id)));
        }
        out.extend(batch);
        if n < 1000 {
            break;
        }
        after = max.unwrap_or(0).to_string();
    }
    Ok(out)
}

/// `guild_member`.
pub async fn guild_member<R: Rest>(r: &R, guild_id: i64, user_id: &Snowflake) -> RResult<Option<Value>> {
    r.get(&format!("/guilds/{guild_id}/members/{user_id}"), 15).await
}

pub async fn add_member_role<R: Rest>(
    r: &R,
    guild_id: i64,
    user_id: &Snowflake,
    role_id: i64,
    reason: &str,
) -> RResult<()> {
    r.send("PUT", &format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"), Some(reason)).await
}

pub async fn remove_member_role<R: Rest>(
    r: &R,
    guild_id: i64,
    user_id: &Snowflake,
    role_id: i64,
    reason: &str,
) -> RResult<()> {
    r.send("DELETE", &format!("/guilds/{guild_id}/members/{user_id}/roles/{role_id}"), Some(reason)).await
}

/// `guild_snapshot`'s result.
pub struct Snapshot {
    pub guild: Value,
    pub me: Value,
    pub channels: Vec<Value>,
    pub roles: Vec<Value>,
}

/// `guild_snapshot`: `None` when the bot can't see the guild.
pub async fn guild_snapshot<R: Rest>(r: &R, guild_id: i64) -> RResult<Option<Snapshot>> {
    let bot_id = r.bot_user_id().await?;
    let guild = r.get(&format!("/guilds/{guild_id}"), 15).await?;
    let me = match bot_id {
        Some(id) if id != 0 => r.get(&format!("/guilds/{guild_id}/members/{id}"), 15).await?,
        _ => None,
    };
    let channels = r.get(&format!("/guilds/{guild_id}/channels"), 15).await?;
    let roles = r.get(&format!("/guilds/{guild_id}/roles"), 15).await?;
    let (Some(guild), Some(me), Some(channels)) = (guild.filter(pyval::truthy), me.filter(pyval::truthy), channels)
    else {
        return Ok(None);
    };
    let channels = match channels {
        Value::Array(a) => a,
        _ => return Err(RestError::other("channels is not a list")),
    };
    Ok(Some(Snapshot { guild, me, channels, roles: as_list(roles)? }))
}

/// A JSON object from pairs, keeping their order.
pub fn obj(pairs: Vec<(&str, Value)>) -> Value {
    let mut m = Map::new();
    for (k, v) in pairs {
        m.insert(k.to_owned(), v);
    }
    Value::Object(m)
}
