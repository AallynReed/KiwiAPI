//! The auth slice of `app/core/config.py`, read from the same environment
//! variables (upper case) with the same defaults. Only HS256 is supported, as
//! configured in production (`jwt_algorithm`).

use std::sync::OnceLock;

use crate::config::Settings;

#[derive(Clone, Debug)]
pub struct AuthSettings {
    pub secret_key: String,
    pub access_token_expire_minutes: i64,
    pub refresh_token_expire_days: i64,
    pub api_token_prefix: String,
    pub token_creation_daily_limit: i64,
    pub token_touch_interval_seconds: i64,
    pub app_url: String,
    pub api_url: String,
    pub debug: bool,
    pub login_max_attempts: i64,
    pub login_attempt_window_seconds: i64,
    pub login_lockout_seconds: i64,
    pub password_breach_check: bool,
    /// uvicorn `--forwarded-allow-ips` (`FORWARDED_ALLOW_IPS`, default 127.0.0.1).
    pub forwarded_allow_ips: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok()
}

pub(crate) fn env_i64(name: &str, default: i64) -> i64 {
    env(name).and_then(|v| v.trim().parse().ok()).unwrap_or(default)
}

impl Default for AuthSettings {
    fn default() -> Self {
        AuthSettings {
            secret_key: "CHANGE_ME_insecure_dev_secret_do_not_use_in_production".into(),
            access_token_expire_minutes: 15,
            refresh_token_expire_days: 30,
            api_token_prefix: "kiwi".into(),
            token_creation_daily_limit: 3,
            token_touch_interval_seconds: 60,
            app_url: "https://trove.aallyn.net".into(),
            api_url: "https://api.aallyn.net".into(),
            debug: false,
            login_max_attempts: 5,
            login_attempt_window_seconds: 900,
            login_lockout_seconds: 900,
            password_breach_check: true,
            forwarded_allow_ips: "127.0.0.1".into(),
        }
    }
}

impl AuthSettings {
    /// Read through [`Settings`] so both parse the environment the same way.
    pub fn from_env() -> Self {
        match Settings::from_env() {
            Ok(c) => AuthSettings::from_settings(&c),
            Err(err) => {
                tracing::error!("settings: {err:#}; auth uses defaults");
                AuthSettings::default()
            }
        }
    }

    pub fn from_settings(c: &Settings) -> Self {
        AuthSettings {
            secret_key: c.secret_key.clone(),
            access_token_expire_minutes: c.access_token_expire_minutes,
            refresh_token_expire_days: c.refresh_token_expire_days,
            api_token_prefix: c.api_token_prefix.clone(),
            token_creation_daily_limit: c.token_creation_daily_limit,
            token_touch_interval_seconds: c.token_touch_interval_seconds,
            app_url: c.app_url.clone(),
            api_url: c.api_url.clone(),
            debug: c.debug,
            login_max_attempts: c.login_max_attempts,
            login_attempt_window_seconds: c.login_attempt_window_seconds,
            login_lockout_seconds: c.login_lockout_seconds,
            password_breach_check: c.password_breach_check,
            forwarded_allow_ips: std::env::var("FORWARDED_ALLOW_IPS").unwrap_or_else(|_| "127.0.0.1".into()),
        }
    }
}

static SETTINGS: OnceLock<AuthSettings> = OnceLock::new();

/// Process-wide settings, read from the environment on first use.
pub fn auth_settings() -> &'static AuthSettings {
    SETTINGS.get_or_init(AuthSettings::from_env)
}
