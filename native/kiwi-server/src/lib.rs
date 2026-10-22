//! The Rust server for Kiwi API. It answers every route it has been ported to
//! and proxies everything else to the Python app (`PY_UPSTREAM`) until nothing
//! is left there. See `native/PORT_PLAN.md`.

pub mod config;
pub mod db;
pub mod dt;
pub mod error;
#[cfg(test)]
mod foundation_golden;
pub mod proxy;
pub mod pyfloat;
pub mod pyjson;
pub mod request_id;
pub mod routes;
pub mod runtime_config;
pub mod state;
pub mod validation;
