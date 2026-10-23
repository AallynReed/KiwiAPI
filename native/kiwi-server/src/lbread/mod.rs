//! Leaderboards read side: `/v1/leaderboards*` and `/site/leaderboards/*`
//! reads (`app/trove/router.py`, `app/site/router.py`), over the Postgres
//! reads of `pg_store.py` ([`store`]), the Redis snapshot cache of `cache.py`
//! ([`cache`]) and the compute of `service.py` ([`service`]). The warmer calls
//! [`cache::warm`], [`cache::warm_days`], [`cache::warm_board_histories`] and
//! [`cache::set_ready_anchor`] with a [`Live`].

pub mod cache;
pub mod routes;
pub mod service;
pub mod store;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use crate::state::AppState;

pub use routes::router;

/// Postgres, Redis and runtime settings of the running server.
#[derive(Clone, Copy)]
pub struct Live<'a>(pub &'a AppState);

/// Starlette's `{uuid:int}` segment as the `INTEGER` column takes it; `None`
/// past int32 (or for anything but digits, which never routes here).
pub fn parse_uuid(seg: &str) -> Option<i32> {
    if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    seg.parse::<i64>().ok().and_then(|v| i32::try_from(v).ok())
}
