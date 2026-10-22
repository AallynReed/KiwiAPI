//! The auth slice of `app/core/config.py`, read from the same environment
//! variables (upper case) with the same defaults. Only HS256 is supported, as
//! configured in production (`jwt_algorithm`).

use std::sync::OnceLock;

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

fn env_str(name: &str, default: &str) -> String {
    env(name).unwrap_or_else(|| default.to_owned())
}

/// pydantic's str -> bool coercion.
fn env_bool(name: &str, default: bool) -> bool {
    match env(name).map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if ["1", "true", "t", "yes", "y", "on"].contains(&v.as_str()) => true,
        Some(v) if ["0", "false", "f", "no", "n", "off"].contains(&v.as_str()) => false,
        _ => default,
    }
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
    pub fn from_env() -> Self {
        let d = AuthSettings::default();
        AuthSettings {
            secret_key: env_str("SECRET_KEY", &d.secret_key),
            access_token_expire_minutes: env_i64("ACCESS_TOKEN_EXPIRE_MINUTES", d.access_token_expire_minutes),
            refresh_token_expire_days: env_i64("REFRESH_TOKEN_EXPIRE_DAYS", d.refresh_token_expire_days),
            api_token_prefix: env_str("API_TOKEN_PREFIX", &d.api_token_prefix),
            token_creation_daily_limit: env_i64("TOKEN_CREATION_DAILY_LIMIT", d.token_creation_daily_limit),
            token_touch_interval_seconds: env_i64("TOKEN_TOUCH_INTERVAL_SECONDS", d.token_touch_interval_seconds),
            app_url: env_str("APP_URL", &d.app_url),
            api_url: env_str("API_URL", &d.api_url),
            debug: env_bool("DEBUG", d.debug),
            login_max_attempts: env_i64("LOGIN_MAX_ATTEMPTS", d.login_max_attempts),
            login_attempt_window_seconds: env_i64("LOGIN_ATTEMPT_WINDOW_SECONDS", d.login_attempt_window_seconds),
            login_lockout_seconds: env_i64("LOGIN_LOCKOUT_SECONDS", d.login_lockout_seconds),
            password_breach_check: env_bool("PASSWORD_BREACH_CHECK", d.password_breach_check),
            forwarded_allow_ips: env_str("FORWARDED_ALLOW_IPS", &d.forwarded_allow_ips),
        }
    }
}

static SETTINGS: OnceLock<AuthSettings> = OnceLock::new();

/// Process-wide settings, read from the environment on first use.
pub fn auth_settings() -> &'static AuthSettings {
    SETTINGS.get_or_init(AuthSettings::from_env)
}
