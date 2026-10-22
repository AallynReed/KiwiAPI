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

use axum::http::Method;

use crate::state::AppState;

pub use routes::router;

/// Postgres, Redis and runtime settings of the running server.
#[derive(Clone, Copy)]
pub struct Live<'a>(pub &'a AppState);

/// Starlette's `{uuid:int}` (and pydantic's `uuid: int`) on a raw segment,
/// limited to what fits the `INTEGER` column; anything else is Python's to answer.
pub fn parse_uuid(seg: &str) -> Option<i32> {
    if seg.is_empty() || !seg.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    seg.parse::<i64>().ok().and_then(|v| i32::try_from(v).ok())
}

const INT_ROUTES: [&str; 7] = [
    "/v1/leaderboards/{uuid}",
    "/v1/leaderboards/{uuid}/entries",
    "/v1/leaderboards/{uuid}/history",
    "/v1/leaderboards/{uuid}/health",
    "/site/leaderboards/{uuid}/entries",
    "/site/leaderboards/{uuid}/history",
    "/site/leaderboards/{uuid}/health",
];

/// Whether a request matched to `matched` belongs to Python: a non-GET method
/// (the sibling `POST /v1/leaderboards/insert` and friends), or a segment that
/// is not a plain int (`cheaters`, `renames`, `players`, `activity`, `%31`, ...).
pub fn defers(matched: &str, method: &Method, raw_path: &str) -> bool {
    INT_ROUTES.contains(&matched)
        && !(matches!(*method, Method::GET | Method::HEAD) && raw_path.split('/').nth(3).and_then(parse_uuid).is_some())
}
