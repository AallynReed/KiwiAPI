//! The Rust server for Kiwi API. It answers every route it has been ported to
//! and proxies everything else to the Python app (`PY_UPSTREAM`) until nothing
//! is left there. See `native/PORT_PLAN.md`.

pub mod auth;
pub mod config;
pub mod db;
pub mod dt;
pub mod error;
pub mod features;
pub mod feeds;
pub mod gamedata;
pub mod gems;
pub mod http;
pub mod jobs;
pub mod middleware;
pub mod proxy;
pub mod pyfloat;
pub mod pyjson;
pub mod ratelimit;
pub mod request_id;
pub mod routes;
pub mod runtime_config;
pub mod state;
pub mod usage;
pub mod validation;

#[cfg(test)]
mod foundation_golden;
