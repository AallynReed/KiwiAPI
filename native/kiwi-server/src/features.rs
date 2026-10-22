//! Feature toggles, as in `app/core/features.py`: a flag is a bool runtime
//! setting, and a gated route answers the plain 404 envelope while it is off.
//!
//! FastAPI runs the gate after routing and before any auth dependency, and a
//! wrong method still gets its 405. Both forms below keep that:
//! - `RequireFeature<features::Market>` as a handler's first argument, or
//! - `get(h).route_layer(middleware::from_fn_with_state(state, features::gate::<features::Market>))`.
//!
//! `Router::route_layer` with the same middleware also gates the 405 fallback,
//! so a wrong method answers the disabled 404 while the flag is off.

use std::marker::PhantomData;

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{StatusCode, request::Parts},
    middleware::Next,
    response::{IntoResponse, Response},
};
use serde_json::Value;

use crate::{error::ApiError, runtime_config, state::AppState};

#[cfg(test)]
mod tests;

pub const MODS_HUB_FLAG: &str = "feature_mods_hub_enabled";
pub const MARKET_FLAG: &str = "feature_market_enabled";
pub const LEADERBOARDS_FLAG: &str = "feature_leaderboards_enabled";
pub const PLAYER_ACTIVITY_FLAG: &str = "feature_player_activity_enabled";
pub const CLASS_ACTIVITY_FLAG: &str = "feature_class_activity_enabled";
pub const CLUBS_FLAG: &str = "feature_clubs_enabled";
pub const UPDATES_FLAG: &str = "feature_updates_enabled";
pub const CODEXES_FLAG: &str = "feature_codexes_enabled";
pub const SERVER_STATUS_FLAG: &str = "feature_server_status_enabled";
pub const GIVEAWAYS_FLAG: &str = "feature_giveaways_enabled";
pub const COMMANDS_FLAG: &str = "feature_commands_enabled";
pub const SERVER_TIME_FLAG: &str = "feature_server_time_enabled";
pub const WEBHOOKS_FLAG: &str = "feature_webhooks_enabled";
pub const IMAGE_STUDIO_FLAG: &str = "feature_image_studio_enabled";
pub const CALENDAR_FLAG: &str = "feature_calendar_enabled";
pub const STREAMS_FLAG: &str = "feature_streams_enabled";
pub const BTT_RELEASES_FLAG: &str = "feature_btt_releases_enabled";
pub const CLASSES_FLAG: &str = "feature_classes_enabled";
pub const STAR_CHART_FLAG: &str = "feature_star_chart_enabled";
pub const GEM_SIMULATOR_FLAG: &str = "feature_gem_simulator_enabled";
pub const GEM_EVALUATOR_FLAG: &str = "feature_gem_evaluator_enabled";
pub const GEM_BUILDS_FLAG: &str = "feature_gem_builds_enabled";
pub const CALCULATORS_FLAG: &str = "feature_calculators_enabled";
pub const GEMS_GUIDE_FLAG: &str = "feature_gems_guide_enabled";
pub const ABILITIES_FLAG: &str = "feature_abilities_enabled";
pub const GUIDES_FLAG: &str = "feature_guides_enabled";
pub const ALLIES_FLAG: &str = "feature_allies_enabled";
pub const GEM_TOOLS_FLAG: &str = "feature_gem_tools_enabled";
pub const FISHING_GUIDE_FLAG: &str = "feature_fishing_guide_enabled";
pub const GEAR_GUIDE_FLAG: &str = "feature_gear_guide_enabled";
pub const LOOT_COLLECTOR_GUIDE_FLAG: &str = "feature_loot_collector_guide_enabled";
pub const DM_SUBS_FLAG: &str = "feature_dm_subscriptions_enabled";
pub const DELVES_FLAG: &str = "feature_delves_enabled";
pub const STORE_FLAG: &str = "feature_store_enabled";
pub const EMBED_FLAG: &str = "feature_embed_viewer_enabled";
pub const DRESSING_ROOM_FLAG: &str = "feature_dressing_room_enabled";
pub const DRESSING_ROOM_PAGE_FLAG: &str = "feature_dressing_room_page_enabled";
pub const SOUND_STUDIO_FLAG: &str = "feature_sound_studio_enabled";
pub const MOD_WORKSHOP_FLAG: &str = "feature_mod_workshop_enabled";
pub const BLUEPRINT_EDITOR_FLAG: &str = "feature_blueprint_editor_enabled";
pub const TOMES_FLAG: &str = "feature_tomes_enabled";
pub const UNLOCK_DEBUG_FLAG: &str = "feature_unlock_debug_enabled";
pub const MOD_ISSUES_FLAG: &str = "feature_mod_issues_enabled";
pub const FILE_DROPS_FLAG: &str = "feature_file_drops_enabled";
pub const COMMUNITY_SITES_FLAG: &str = "feature_community_sites_enabled";
pub const CHEATER_DETECTION_FLAG: &str = "feature_cheater_detection_enabled";
pub const ALT_CLUSTERS_FLAG: &str = "feature_alt_clusters_enabled";
pub const RENAMES_FLAG: &str = "feature_leaderboard_renames_enabled";
pub const DUPLICATES_FLAG: &str = "feature_leaderboard_duplicates_enabled";

/// One gate of `app/core/features.py`: the flag and the name in its 404 message.
pub trait Feature: Send + Sync + 'static {
    const FLAG: &'static str;
    const NAME: &'static str;
}

macro_rules! gates {
    ($( $ty:ident => $flag:ident, $name:literal; )*) => {
        $(
            pub struct $ty;
            impl Feature for $ty {
                const FLAG: &'static str = $flag;
                const NAME: &'static str = $name;
            }
        )*
        /// Every gate as `(flag, name)`, in `features.py` order.
        pub const GATES: &[(&str, &str)] = &[$( ($flag, $name) ),*];
    };
}

gates! {
    ModsHub => MODS_HUB_FLAG, "Mods Hub";
    Market => MARKET_FLAG, "Market";
    Leaderboards => LEADERBOARDS_FLAG, "Leaderboards";
    PlayerActivity => PLAYER_ACTIVITY_FLAG, "Player Activity";
    ClassActivity => CLASS_ACTIVITY_FLAG, "Class Activity";
    Updates => UPDATES_FLAG, "Updates archive";
    Codexes => CODEXES_FLAG, "Codexes";
    ServerStatus => SERVER_STATUS_FLAG, "Server Status";
    Giveaways => GIVEAWAYS_FLAG, "Giveaways";
    Webhooks => WEBHOOKS_FLAG, "Webhooks";
    ImageStudio => IMAGE_STUDIO_FLAG, "Image Studio";
    DmSubs => DM_SUBS_FLAG, "DM subscriptions";
    Delves => DELVES_FLAG, "Delve rotation data";
    Store => STORE_FLAG, "Store catalog";
    LeaderboardRenames => RENAMES_FLAG, "Rename detection";
    LeaderboardDuplicates => DUPLICATES_FLAG, "Duplicate-name detection";
    Embed => EMBED_FLAG, "embeddable viewer";
    DressingRoom => DRESSING_ROOM_FLAG, "Dressing Room";
    Tomes => TOMES_FLAG, "Tomes";
    UnlockDebug => UNLOCK_DEBUG_FLAG, "Unlock Debug patcher";
    ModIssues => MOD_ISSUES_FLAG, "mod issues";
    FileDrops => FILE_DROPS_FLAG, "file drops";
}

/// `bool(get_setting(flag))`.
pub async fn is_enabled(state: &AppState, flag: &str) -> anyhow::Result<bool> {
    Ok(py_truthy(&runtime_config::get_setting(state, flag).await?))
}

pub fn disabled(name: &str) -> ApiError {
    ApiError::new(StatusCode::NOT_FOUND, "not_found", format!("The {name} is currently disabled."))
}

/// `Ok` when the feature is on, else its 404 (a lookup failure is the 500 any
/// unhandled exception becomes in Python).
pub async fn check<F: Feature>(state: &AppState) -> Result<(), ApiError> {
    match is_enabled(state, F::FLAG).await {
        Ok(true) => Ok(()),
        Ok(false) => Err(disabled(F::NAME)),
        Err(e) => {
            tracing::error!("feature flag {} lookup failed: {e:#}", F::FLAG);
            Err(ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, "internal_error", "Internal server error"))
        }
    }
}

/// Router-level gate for `route_layer(middleware::from_fn_with_state(..))`.
pub async fn gate<F: Feature>(State(state): State<AppState>, req: Request, next: Next) -> Response {
    match check::<F>(&state).await {
        Ok(()) => next.run(req).await,
        Err(e) => e.into_response(),
    }
}

/// Extractor form of the gate; list it before any auth extractor.
pub struct RequireFeature<F>(PhantomData<fn() -> F>);

impl<F: Feature> FromRequestParts<AppState> for RequireFeature<F> {
    type Rejection = ApiError;

    async fn from_request_parts(_parts: &mut Parts, state: &AppState) -> Result<Self, ApiError> {
        check::<F>(state).await.map(|()| RequireFeature(PhantomData))
    }
}

/// Python truthiness of a decoded JSON value.
pub fn py_truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}
