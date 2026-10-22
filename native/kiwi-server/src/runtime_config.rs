//! Live settings from the Mongo `runtime_config` collection with the registry
//! defaults of `app/admin/runtime_config.py`, cached a few seconds like Python.
//! (Foundation wave fills this in.)

use serde_json::Value;

use crate::state::AppState;

/// The effective value of a runtime setting (stored override, else default).
pub async fn get_setting(_state: &AppState, key: &str) -> anyhow::Result<Value> {
    anyhow::bail!("runtime_config not ported yet: {key}")
}
