//! `app/core/lockout.py`: per-account login lockout in Redis
//! (`loginfail:{email}` counter, `loginlock:{email}` flag). A silent no-op
//! without Redis, like Python.

use axum::http::StatusCode;
use redis::AsyncCommands;

use super::Rejection;
use crate::state::AppState;

fn fail_key(email: &str) -> String {
    format!("loginfail:{}", email.to_lowercase())
}

fn lock_key(email: &str) -> String {
    format!("loginlock:{}", email.to_lowercase())
}

/// Seconds left on the lock, 0 when not locked.
pub async fn lock_ttl(state: &AppState, email: &str) -> Result<i64, Rejection> {
    let Some(mut r) = state.inner.db.redis.clone() else {
        return Ok(0);
    };
    let ttl: i64 = r.ttl(lock_key(email)).await.map_err(Rejection::internal)?;
    Ok(ttl.max(0))
}

/// Count a failed attempt; lock once `login_max_attempts` is reached.
pub async fn record_failure(state: &AppState, email: &str) -> Result<(), Rejection> {
    let Some(mut r) = state.inner.db.redis.clone() else {
        return Ok(());
    };
    let s = super::auth_settings();
    let count: i64 = r.incr(fail_key(email), 1).await.map_err(Rejection::internal)?;
    if count == 1 {
        let _: bool = r.expire(fail_key(email), s.login_attempt_window_seconds).await.map_err(Rejection::internal)?;
    }
    if count >= s.login_max_attempts {
        let secs = u64::try_from(s.login_lockout_seconds).unwrap_or(0);
        let _: () = r.set_ex(lock_key(email), "1", secs).await.map_err(Rejection::internal)?;
    }
    Ok(())
}

/// Reset both keys after a successful login.
pub async fn clear(state: &AppState, email: &str) -> Result<(), Rejection> {
    let Some(mut r) = state.inner.db.redis.clone() else {
        return Ok(());
    };
    let _: i64 = r.del(&[fail_key(email), lock_key(email)]).await.map_err(Rejection::internal)?;
    Ok(())
}

/// The 429 `account_locked` the login route raises while locked.
pub fn locked(ttl: i64) -> Rejection {
    Rejection::new(StatusCode::TOO_MANY_REQUESTS, "account_locked", "Too many failed attempts. Try again later.")
        .with_header("retry-after", ttl.to_string())
}
