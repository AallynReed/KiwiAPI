//! The Rust server for Kiwi API. It answers every route it has been ported to
//! and proxies everything else to the Python app (`PY_UPSTREAM`) until nothing
//! is left there. See `native/PORT_PLAN.md`.

pub mod activity;
pub mod admin;
pub mod archiver;
pub mod auth;
pub mod blueprints;
pub mod cheaters;
pub mod codexes;
pub mod config;
pub mod db;
pub mod decode;
pub mod dm_subs;
pub mod dressing;
pub mod drops;
pub mod dt;
pub mod email;
pub mod embed;
pub mod embed_templates;
pub mod error;
pub mod events;
pub mod features;
pub mod feeds;
pub mod gamedata;
pub mod gems;
pub mod giveaways;
pub mod http;
pub mod images;
pub mod internal;
pub mod jobs;
pub mod lb_ingest;
pub mod lb_warmer;
pub mod lbread;
pub mod maintenance;
pub mod market;
pub mod media;
pub mod middleware;
pub mod mod_releases;
pub mod mods_catalog;
pub mod mods_creators;
pub mod mods_git;
pub mod mods_issues;
pub mod mods_moderation;
pub mod mods_write_auth;
pub mod ocr;
pub mod og;
pub mod outbound;
pub mod portal;
pub mod proxy;
pub mod pyfloat;
pub mod pyjson;
pub mod ratelimit;
pub mod request_id;
pub mod routes;
pub mod runtime_config;
pub mod site_auth;
pub mod state;
pub mod tools;
pub mod updates;
pub mod usage;
pub mod validation;
pub mod webhooks;

#[cfg(test)]
mod foundation_golden;
