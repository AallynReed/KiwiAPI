//! The master-only routes of `app/admin/router.py` that are not another
//! area's: dev-portal users and tokens, the usage feed and activity, the
//! review-queue badges, Site Analytics, runtime config, site users, Trove-name
//! claims and username requests, Discord command registration, bot stats and
//! live embed uploads. Every route sits behind `get_current_superuser`.

pub mod config;
pub mod discord;
pub mod docs;
pub mod pageviews;
pub mod pending;
pub mod site_users;
pub mod users;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequestParts, MatchedPath},
    http::request::Parts,
    routing::{get, post, put},
};
use mongodb::{Collection, bson::Document};
use serde_json::{Value, json};

use crate::{
    error::{self, ApiError},
    middleware::pyurl::scope_path,
    pyjson::PydanticJson,
    state::AppState,
    validation::{FromParams, Params},
};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/admin/users", get(users::list_users))
        .route(
            "/admin/users/{user_id}/tokens",
            get(users::list_user_tokens)
                .delete(users::revoke_user_tokens)
                .fallback(|| async { error::method_not_allowed("GET") }),
        )
        .route("/admin/users/{user_id}/terminate", post(users::terminate_user))
        .route("/admin/users/{user_id}/reactivate", post(users::reactivate_user))
        .route("/admin/users/{user_id}/activity", get(users::user_activity))
        .route("/admin/users/{user_id}", get(users::get_user))
        .route("/admin/tokens/{token_id}/revoke", post(users::revoke_token))
        .route("/admin/events", get(users::list_events))
        .route("/admin/pending", get(pending::pending_counts))
        .route("/admin/activity", get(users::activity_overview))
        .route("/admin/pageviews", get(pageviews::site_pageviews))
        .route("/admin/pageviews/live", get(pageviews::site_pageviews_live))
        .route("/admin/config", get(config::list_runtime_config))
        .route(
            "/admin/config/{key}",
            put(config::update_runtime_config)
                .delete(config::reset_runtime_config)
                .fallback(|| async { error::method_not_allowed("PUT") }),
        )
        .route("/admin/discord/commands", get(discord::commands_preview))
        .route("/admin/discord/register-commands", post(discord::register_commands))
        .route("/admin/bot/stats", get(discord::bot_stats))
        .route("/admin/discord/clear-guild-commands", post(discord::clear_guild_commands))
        .route("/admin/site-claims", get(site_users::list_site_claims))
        .route("/admin/site-claims/{user_id}/approve", post(site_users::approve_site_claim))
        .route("/admin/site-claims/{user_id}/reject", post(site_users::reject_site_claim))
        .route("/admin/username-requests", get(site_users::list_username_requests))
        .route("/admin/username-requests/{request_id}/approve", post(site_users::approve_username_request))
        .route("/admin/username-requests/{request_id}/reject", post(site_users::reject_username_request))
        .route("/admin/site-users", get(site_users::list_site_users))
        .route("/admin/site-users/{user_id}", get(site_users::get_site_user))
        .route("/admin/site-users/{user_id}/deactivate", post(site_users::deactivate_site_user))
        .route("/admin/site-users/{user_id}/activate", post(site_users::activate_site_user))
        .route("/admin/site-users/{user_id}/logout", post(site_users::logout_site_user))
        .route("/admin/site-users/{user_id}/username", post(site_users::set_site_username))
        .route("/admin/site-users/{user_id}/claimed-name", post(site_users::set_site_claimed_name))
        .route("/admin/site-users/{user_id}/refresh-discord", post(site_users::refresh_site_discord))
        .route("/admin/embed/uploads", get(embed_upload_stats))
}

pub fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("admin route: {e}");
    ApiError::internal()
}

pub fn coll(state: &AppState, name: &str) -> Collection<Document> {
    state.mongo().collection(name)
}

pub type Res<T = PydanticJson> = Result<T, ApiError>;

/// Path parameters as Starlette decodes them (uvicorn's unquoted path), paired
/// with the matched template's `{name}` segments.
pub fn path_params(parts: &Parts) -> Vec<(String, String)> {
    let Some(m) = parts.extensions.get::<MatchedPath>() else { return Vec::new() };
    let path = scope_path(&parts.uri);
    m.as_str()
        .split('/')
        .zip(path.split('/'))
        .filter_map(|(t, v)| {
            t.strip_prefix('{').and_then(|n| n.strip_suffix('}')).map(|n| (n.to_owned(), v.to_owned()))
        })
        .collect()
}

/// One decoded path parameter (a plain `str` that cannot fail validation).
pub fn path_param(parts: &Parts, name: &str) -> String {
    path_params(parts).into_iter().find(|(k, _)| k == name).map(|(_, v)| v).unwrap_or_default()
}

/// Path and query parameters read with [`Params`], path values Starlette-decoded.
pub struct Args<T>(pub T);

impl<S: Send + Sync, T: FromParams> FromRequestParts<S> for Args<T> {
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, ApiError> {
        let query = parts.uri.query().map(str::to_owned);
        let mut p = Params::new(path_params(parts), query.as_deref(), &parts.headers);
        let value = T::from_params(&mut p);
        Ok(Args(p.v.finish(value)?))
    }
}

/// A plain `str` path parameter, decoded.
pub struct Seg<const I: usize>(pub String);

/// The `{name}` each [`Seg`] index reads.
const SEG_NAMES: [&str; 3] = ["key", "user_id", "request_id"];

impl<S: Send + Sync, const I: usize> FromRequestParts<S> for Seg<I> {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        Ok(Seg(path_param(parts, SEG_NAMES[I])))
    }
}

pub type KeySeg = Seg<0>;
pub type UserSeg = Seg<1>;
pub type RequestSeg = Seg<2>;

/// `embed.uploads.stats()`: live `.tmod` previews held in Redis.
async fn embed_upload_stats(
    axum::extract::State(state): axum::extract::State<AppState>,
    _: crate::auth::Superuser,
) -> Res {
    let Some(mut r) = state.inner.db.redis.clone() else {
        return Ok(PydanticJson(json!({"live": 0, "bytes": 0, "redis": false})));
    };
    let mut keys: Vec<String> = Vec::new();
    let mut cursor: u64 = 0;
    loop {
        let (next, batch): (u64, Vec<String>) = redis::cmd("SCAN")
            .arg(cursor)
            .arg("MATCH")
            .arg("embed:tmod:*")
            .arg("COUNT")
            .arg(500)
            .query_async(&mut r)
            .await
            .map_err(internal)?;
        keys.extend(batch);
        if next == 0 {
            break;
        }
        cursor = next;
    }
    let mut encoded: i64 = 0;
    for chunk in keys.chunks(100) {
        let mut pipe = redis::pipe();
        for k in chunk {
            pipe.cmd("STRLEN").arg(k);
        }
        let lens: Vec<i64> = pipe.query_async(&mut r).await.map_err(internal)?;
        encoded += lens.iter().sum::<i64>();
    }
    Ok(PydanticJson(embed_stats_body(keys.len(), encoded)))
}

pub fn embed_stats_body(live: usize, encoded: i64) -> Value {
    json!({"live": live, "bytes": encoded * 3 / 4, "redis": true})
}
