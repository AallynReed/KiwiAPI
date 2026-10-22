//! Process settings, read like `app/core/config.py` (pydantic-settings): the same
//! fields and defaults, env names matched case-insensitively, values parsed with
//! pydantic's lax rules (bools `1/0/true/false/yes/no/on/off/t/f/y/n`, ints with
//! `_` separators and `.0` fractions, list fields as JSON). An empty value counts
//! as set, as in Python. `.env` is loaded into the process env by `main`.

use std::collections::HashMap;

use anyhow::{Context, anyhow};
use serde_json::Value;

#[cfg(test)]
pub(crate) mod tests;

macro_rules! settings {
    (@ty str) => { String };
    (@ty bool) => { bool };
    (@ty int) => { i64 };
    (@ty float) => { f64 };
    (@ty opt) => { Option<String> };
    (@ty list) => { Vec<String> };

    (@default str, $d:expr) => { String::from($d) };
    (@default bool, $d:expr) => { $d };
    (@default int, $d:expr) => { $d };
    (@default float, $d:expr) => { $d };
    (@default opt) => { None };
    (@default opt, $d:expr) => { Some(String::from($d)) };
    (@default list, $d:expr) => { $d.iter().map(|s| s.to_string()).collect() };

    (@parse str, $raw:expr) => { Ok::<String, String>($raw.to_owned()) };
    (@parse bool, $raw:expr) => { parse_bool($raw) };
    (@parse int, $raw:expr) => { parse_int($raw) };
    (@parse float, $raw:expr) => { parse_float($raw) };
    (@parse opt, $raw:expr) => { Ok::<Option<String>, String>(Some($raw.to_owned())) };
    (@parse list, $raw:expr) => { parse_list($raw) };

    ($( $kind:ident $name:ident $(= $default:expr)? ; )*) => {
        #[derive(Clone, Debug)]
        pub struct Settings {
            /// Where this server listens, e.g. `0.0.0.0:8000` (`KIWI_BIND`).
            pub bind: String,
            /// The Python app every unported route is proxied to (`PY_UPSTREAM`).
            pub py_upstream: String,
            $( pub $name: settings!(@ty $kind), )*
        }

        impl Settings {
            /// The Python field names, in declaration order.
            pub const FIELDS: &'static [&'static str] = &[$( stringify!($name) ),*];

            fn build(env: &HashMap<String, String>) -> anyhow::Result<Self> {
                let set = |name: &str| env.get(name).filter(|v| !v.is_empty()).cloned();
                Ok(Settings {
                    bind: set("kiwi_bind").unwrap_or_else(|| "0.0.0.0:8000".into()),
                    py_upstream: set("py_upstream")
                        .context("PY_UPSTREAM is required (the Python app to proxy unported routes to)")?
                        .trim_end_matches('/')
                        .to_owned(),
                    $( $name: match env.get(stringify!($name)) {
                        Some(raw) => settings!(@parse $kind, raw.as_str())
                            .map_err(|e| anyhow!("{}: {e}", stringify!($name).to_uppercase()))?,
                        None => settings!(@default $kind $(, $default)?),
                    }, )*
                })
            }

            /// Every Python field as JSON (non-finite floats become null).
            pub fn fields_json(&self) -> serde_json::Map<String, Value> {
                let mut m = serde_json::Map::new();
                $( m.insert(stringify!($name).to_owned(), Value::from(self.$name.clone())); )*
                m
            }
        }
    };
}

settings! {
    str app_name = "Kiwi API";
    bool debug = false;
    int max_request_body_bytes = 8 * 1024 * 1024;
    int mods_max_request_body_bytes = 20 * 1024 * 1024;
    bool mods_hub_enabled = true;
    str mods_store_dir = "data/mods";
    int mods_hub_max_file_bytes = 20 * 1024 * 1024;
    int mods_image_max_bytes = 5 * 1024 * 1024;
    int mods_hub_max_files_per_commit = 500;
    int mods_git_max_body_bytes = 100 * 1024 * 1024;
    int embed_upload_max_bytes = 8 * 1024 * 1024;
    int leaderboards_max_request_body_bytes = 20 * 1024 * 1024;
    int market_max_request_body_bytes = 20 * 1024 * 1024;
    int ocr_max_request_body_bytes = 12 * 1024 * 1024;
    str ffdec_jar = "/opt/ffdec/ffdec.jar";
    str ffdec_java = "java";
    int ffdec_max_heap_mb = 768;
    int ffdec_timeout = 120;
    int ffdec_file_timeout = 60;
    int ffdec_method_timeout = 15;
    int ffdec_max_swf_bytes = 24 * 1024 * 1024;
    int ffdec_max_source_bytes = 12 * 1024 * 1024;
    int ffdec_max_scripts = 4000;
    int ffdec_max_concurrent = 2;
    int leaderboards_hot_retention_days = 3;
    int leaderboards_pg_tier_after_days = 7;
    int leaderboards_anon_retention_days = 7;
    int leaderboards_archive_query_threshold_days = 3;
    int leaderboards_archive_rate_limit_max = 10;
    int leaderboards_archive_rate_limit_window_seconds = 60;
    float cheaters_z_threshold = 5.0;
    float cheaters_velocity_multiplier = 10.0;
    int cheaters_min_board_size = 20;
    int cheaters_cache_ttl_seconds = 1800;
    str cheaters_excluded_board_uuids = "";
    float cheaters_elite_cohort_pct = 0.05;
    int cheaters_cluster_min_size = 3;
    float cheaters_cluster_score_band_pct = 0.02;
    int cheaters_cluster_max_edit_distance = 2;
    int cheaters_cluster_size_full = 8;
    str cheaters_cluster_excluded_board_uuids = "";
    int cheaters_comovement_candidate_top_n = 400;
    float cheaters_comovement_min_hourly_gain = 1.0;
    float cheaters_comovement_gain_percentile = 0.90;
    float cheaters_comovement_gain_tolerance_pct = 0.05;
    int cheaters_comovement_min_matching_hours = 3;
    float cheaters_comovement_min_match_ratio = 0.7;
    float cheaters_comovement_min_density = 0.6;
    int cheaters_comovement_min_group_size = 2;
    int cheaters_comovement_max_cell_accounts = 40;
    int cheaters_comovement_recompute_seconds = 3600;
    int cheaters_schedule_min_active_hours = 6;
    float cheaters_schedule_min_similarity = 0.8;
    float cheaters_fusion_corroboration_bonus = 0.06;
    float cheaters_footprint_min_jaccard = 0.6;
    float cheaters_weekly_uptime_fraction = 0.85;
    int renames_max_gap_seconds = 27000;
    int renames_min_boards = 2;
    float renames_score_drift_pct = 0.02;
    float renames_min_score = 1000.0;
    float renames_min_confidence = 0.5;
    str renames_excluded_board_uuids = "";
    int renames_cache_ttl_seconds = 300;
    int duplicates_lookback_days = 7;
    int duplicates_min_boards = 1;
    int duplicates_frozen_min_captures = 4;
    int duplicates_max_names = 500;
    int duplicates_cache_ttl_seconds = 300;
    str last_played_excluded_board_uuids = "";
    bool capture_completeness_enabled = true;
    int capture_max_missing_boards = 1;
    float capture_collapse_frac = 0.5;
    int capture_min_prev_boards = 20;
    int class_activity_power_rank_threshold = 25000;
    int class_activity_effort_threshold = 50;
    int class_activity_xp_threshold = 2_000_000;
    int market_archive_rate_limit_max = 10;
    int market_archive_rate_limit_window_seconds = 60;
    int sound_studio_max_request_body_bytes = 32 * 1024 * 1024;
    int mod_workshop_max_request_body_bytes = 32 * 1024 * 1024;
    int unlock_debug_max_request_body_bytes = 160 * 1024 * 1024;
    int drops_max_request_body_bytes = 512 * 1024 * 1024;
    str drops_store_dir = "data/drops";
    str custom_art_workspace = "data/custom-art";
    str custom_art_remote = "data/troveui.git";
    str custom_art_lib_remote = "https://github.com/AallynReed/KiwiZUI.git";
    str custom_art_env_file = "data/troveui.env";
    str custom_art_branch = "live-us";
    int custom_art_poll_seconds = 15;
    int custom_art_image_max_bytes = 3 * 1024 * 1024;
    str custom_art_steam_user = "";
    str mod_stats_handle = "aallyn";
    str site_master_handle = "aallyn";
    str site_root = "site";
    str api_url = "https://api.aallyn.net";
    str dev_url = "https://dev.aallyn.net";
    str docs_url = "https://docs.aallyn.net";
    str app_url = "https://trove.aallyn.net";
    str asset_base_url = "";
    str internal_api_url = "http://api:8000";
    str internal_key = "";
    str geoip_dir = "data/geoip";
    str mongo_uri = "mongodb://localhost:27016";
    str mongo_db = "kiwi";
    opt redis_url;
    opt postgres_dsn;
    int postgres_pool_min = 2;
    int postgres_pool_max = 10;
    str secret_key = "CHANGE_ME_insecure_dev_secret_do_not_use_in_production";
    str jwt_algorithm = "HS256";
    int access_token_expire_minutes = 15;
    int refresh_token_expire_days = 30;
    opt github_client_id;
    opt github_client_secret;
    opt discord_client_id;
    opt discord_client_secret;
    opt discord_public_key;
    opt discord_bot_token;
    bool bot_members_intent = true;
    opt discord_install_url;
    str api_token_prefix = "kiwi";
    int token_creation_daily_limit = 3;
    int token_inactivity_revoke_days = 182;
    int token_touch_interval_seconds = 60;
    int idempotency_lock_seconds = 60;
    int idempotency_result_seconds = 86400;
    int token_expiry_warning_days = 7;
    str captcha_provider = "turnstile";
    opt captcha_secret;
    opt captcha_sitekey;
    int signup_rate_limit_max = 5;
    int signup_rate_limit_window_seconds = 3600;
    int login_rate_limit_max = 10;
    int login_rate_limit_window_seconds = 300;
    int api_rate_limit_max = 120;
    int api_rate_limit_window_seconds = 60;
    int public_anon_rate_limit_max = 30;
    int public_anon_rate_limit_window_seconds = 60;
    int codexes_rate_limit_multiplier = 5;
    int bilibili_image_rate_limit_multiplier = 10;
    int updates_file_rate_limit_multiplier = 20;
    int mods_rate_limit_multiplier = 8;
    int mods_asset_rate_limit_multiplier = 30;
    int ingest_cooldown_max = 1;
    int ingest_cooldown_window_seconds = 300;
    int password_min_length = 8;
    bool require_verified_for_login = true;
    int login_max_attempts = 5;
    int login_attempt_window_seconds = 900;
    int login_lockout_seconds = 900;
    bool password_breach_check = true;
    bool disposable_email_check = true;
    bool security_email_notifications = true;
    opt admin_email;
    opt admin_password;
    int usage_retention_days = 30;
    bool pageview_tracking_enabled = true;
    int pageview_retention_days = 90;
    opt smtp_host;
    int smtp_port = 25;
    opt smtp_username;
    opt smtp_password;
    bool smtp_starttls = false;
    bool smtp_ssl = false;
    str mail_from = "no-reply@aallyn.net";
    str mail_from_name = "Kiwi";
    int email_verification_expire_hours = 48;
    int password_reset_expire_hours = 1;
    int forgot_password_rate_limit_max = 5;
    int forgot_password_rate_limit_window_seconds = 3600;
    int email_worker_poll_seconds = 10;
    int email_max_attempts = 5;
    int email_retry_base_seconds = 60;
    int email_outbox_retention_days = 30;
    bool github_secret_scanning_verify = true;
    str github_secret_scanning_keys_url = "https://api.github.com/meta/public_keys/secret_scanning";
    bool require_verified_for_tokens = true;
    str trove_news_feed_url = "https://trovegame.com/feed";
    int trove_news_refresh_seconds = 1800;
    opt twitch_client_id;
    opt twitch_client_secret;
    opt yt_api_key;
    str twitch_game_id = "412756";
    int trove_feeds_refresh_seconds = 300;
    str btt_releases_repo = "AallynReed/BetterTroveTools";
    int btt_releases_refresh_seconds = 1800;
    opt btt_releases_token;
    str site_source_repo = "AallynReed/KiwiAPI";
    str trove_delve_source_url = "";
    str trove_delve_source_referer = "";
    str trove_chaos_chest_url = "https://trovesaurus.com/api/chaos-chest";
    int trove_chaos_refresh_seconds = 1800;
    str trove_events_feed_url = "https://trovesaurus.com/calendar/feed";
    int trove_events_refresh_seconds = 900;
    int trove_events_history_days = 365;
    str steam_web_api_key = "";
    int steam_workshop_refresh_seconds = 21600;
    str events_channel = "kiwi:events";
    int events_heartbeat_seconds = 20;
    int events_watch_seconds = 30;
    int events_max_connections = 1000;
    str webhooks_queue = "kiwi:webhooks:queue";
    str dm_subs_queue = "kiwi:dmsubs:queue";
    bool trove_update_enabled = false;
    str trove_update_base_url = "http://trove-update.dyn.triongames.com";
    str trove_update_prefix = "/kiwi-live-client-patch/";
    str trove_update_store_dir = "data/updates";
    str gamedata_dir = "";
    str pkfx_dev_vfx_dir = "";
    int trove_update_probe_seconds = 1200;
    int trove_update_concurrency = 6;
    str trove_render_branch = "live-us";
    str trove_local_game_dir = "";
    int trove_render_cache_ttl = 86400;
    bool backlog_enabled = true;
    str backlog_dir = "/data/backlog";
    int backlog_retention_days = 0;
    str trove_status_auth_url = "https://auth.trionworlds.com/auth/v1_2";
    int trove_status_probe_interval_seconds = 60;
    float trove_status_timeout_seconds = 8.0;
    int trove_status_probe_attempts = 3;
    float trove_status_probe_retry_delay_seconds = 2.0;
    bool trove_status_local_check_enabled = true;
    str trove_status_local_check_hosts = "1.1.1.1:443,9.9.9.9:443,8.8.8.8:443";
    float trove_status_local_check_timeout_seconds = 4.0;
    str trove_status_eu_host = "ams-c12-b05.ams.triongames.com";
    int trove_status_eu_port = 6560;
    str trove_status_us_host = "dal-c35-b05.dal.triongames.com";
    int trove_status_us_port = 6560;
    str trove_status_pts_host = "auth-pcpts01.trovegame.com";
    int trove_status_pts_port = 6560;
    bool trove_status_game_deep_probe = true;
    bool trove_status_game_random_opener = true;
    str trove_status_eu_hello_hex = "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7c06bb098626e23accfb";
    str trove_status_us_hello_hex = "20000000003df232536bcb1518164c4685392572b843d0bcbb71be7c06bb098626e23accfb";
    str trove_status_pts_hello_hex = "";
    float trove_status_game_hold_seconds = 1.5;
    opt rate_limit_alert_email = "aallyn@aallyn.net";
    int rate_limit_alert_threshold = 20;
    int rate_limit_digest_window_hours = 24;
    list cors_origins = [
        "https://docs.aallyn.net",
        "https://dev.aallyn.net",
        "http://localhost:25468",
        "http://127.0.0.1:25468",
        "http://localhost:15546",
        "http://127.0.0.1:15546",
    ];
    str cors_origin_regex = r"https://([a-z0-9-]+\.)?aallyn\.net|http://(localhost|127\.0\.0\.1)(:\d+)?";
}

const CAPTCHA_PROVIDERS: [&str; 2] = ["hcaptcha", "turnstile"];

impl Settings {
    pub fn from_env() -> anyhow::Result<Self> {
        Self::from_vars(std::env::vars_os().filter_map(|(k, v)| Some((k.into_string().ok()?, v.into_string().ok()?))))
    }

    /// Build from explicit `(NAME, value)` pairs instead of the process env.
    pub fn from_vars<I, K, V>(vars: I) -> anyhow::Result<Self>
    where
        I: IntoIterator<Item = (K, V)>,
        K: AsRef<str>,
        V: Into<String>,
    {
        let env: HashMap<String, String> =
            vars.into_iter().map(|(k, v)| (k.as_ref().to_lowercase(), v.into())).collect();
        let settings = Self::build(&env)?;
        if !CAPTCHA_PROVIDERS.contains(&settings.captcha_provider.as_str()) {
            anyhow::bail!("CAPTCHA_PROVIDER: input should be 'hcaptcha' or 'turnstile'");
        }
        Ok(settings)
    }

    pub fn github_oauth_enabled(&self) -> bool {
        truthy(&self.github_client_id) && truthy(&self.github_client_secret)
    }

    pub fn discord_oauth_enabled(&self) -> bool {
        truthy(&self.discord_client_id) && truthy(&self.discord_client_secret)
    }

    pub fn discord_install_link(&self) -> Option<String> {
        if let Some(url) = self.discord_install_url.as_ref().filter(|u| !u.is_empty()) {
            return Some(url.clone());
        }
        self.discord_client_id
            .as_ref()
            .filter(|id| !id.is_empty())
            .map(|id| format!("https://discord.com/oauth2/authorize?client_id={id}"))
    }

    pub fn postgres_enabled(&self) -> bool {
        truthy(&self.postgres_dsn)
    }

    pub fn email_enabled(&self) -> bool {
        truthy(&self.smtp_host)
    }

    /// Base URL for API-served images embedded off-site.
    pub fn asset_url(&self) -> String {
        let base = if self.asset_base_url.is_empty() { &self.app_url } else { &self.asset_base_url };
        base.trim_end_matches('/').to_owned()
    }
}

fn truthy(v: &Option<String>) -> bool {
    v.as_deref().is_some_and(|s| !s.is_empty())
}

fn parse_bool(raw: &str) -> Result<bool, String> {
    const FALSE: [&str; 6] = ["0", "off", "f", "false", "n", "no"];
    const TRUE: [&str; 6] = ["1", "on", "t", "true", "y", "yes"];
    if FALSE.iter().any(|w| raw.eq_ignore_ascii_case(w)) {
        Ok(false)
    } else if TRUE.iter().any(|w| raw.eq_ignore_ascii_case(w)) {
        Ok(true)
    } else {
        Err("input should be a valid boolean, unable to interpret input".into())
    }
}

/// Python allows single `_` between digits; pydantic mirrors that.
fn strip_underscores(s: &str) -> Option<String> {
    if !s.contains('_') || s.starts_with('_') || s.ends_with('_') || s.contains("__") {
        return None;
    }
    Some(s.replace('_', ""))
}

fn parse_int(raw: &str) -> Result<i64, String> {
    fn plain(s: &str) -> Option<i64> {
        s.parse::<i64>().ok().or_else(|| {
            let (int, frac) = s.split_once('.')?;
            if frac.is_empty() || !frac.bytes().all(|b| b == b'0') {
                return None;
            }
            int.parse().ok()
        })
    }
    let s = raw.trim();
    plain(s)
        .or_else(|| strip_underscores(s).and_then(|s| plain(&s)))
        .ok_or_else(|| "input should be a valid integer, unable to parse string as an integer".into())
}

fn parse_float(raw: &str) -> Result<f64, String> {
    raw.trim()
        .parse::<f64>()
        .ok()
        .or_else(|| strip_underscores(raw).and_then(|s| s.parse().ok()))
        .ok_or_else(|| "input should be a valid number, unable to parse string as a number".into())
}

fn parse_list(raw: &str) -> Result<Vec<String>, String> {
    let value: Value = serde_json::from_str(raw).map_err(|e| format!("error parsing value as JSON: {e}"))?;
    let Value::Array(items) = value else {
        return Err("input should be a valid list".into());
    };
    items
        .into_iter()
        .map(|v| match v {
            Value::String(s) => Ok(s),
            _ => Err("input should be a valid string".to_owned()),
        })
        .collect()
}
