//! The game-data decode pipeline (`app/trove/decode`): after each patch every
//! decoder rebuilds its `gamedata/*.json` from the update archive, byte for
//! byte what Python writes, and the dev panel's Game data tab reports how each
//! run went.
//!
//! - [`registry`]: the decoders, their file formats and code versions.
//! - [`runner`]: the post-sync hook ([`runner::ensure_built`]), manual runs,
//!   the shared `kiwi:decode:running` lock and the `decoder_runs` records.
//! - [`routes`]: `GET /admin/gamedata`, `POST /admin/gamedata/run`, and the web
//!   container's `GET /gamedata/{name}` ([`routes::web_router`]).
//! - [`view`]: the Mod Workshop's binfab field viewer.
//! - [`value`] / [`json`] / [`tree`]: Python's data model for parsed prefabs and
//!   decoder output, and the game file tree.

pub mod ability;
pub mod common;
pub mod decoders;
pub mod json;
pub mod registry;
pub mod routes;
pub mod runner;
pub mod sections;
pub mod tree;
pub mod value;
pub mod view;

#[cfg(test)]
mod tests;
