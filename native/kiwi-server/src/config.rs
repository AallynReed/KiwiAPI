//! Process settings, read from the same environment variables as
//! `app/core/config.py` (names are case-insensitive there; we read upper case).

use anyhow::Context;

#[derive(Clone, Debug)]
pub struct Settings {
    /// Where this server listens, e.g. `0.0.0.0:8000`.
    pub bind: String,
    /// The Python app every unported route is proxied to, e.g. `http://api-py:8000`.
    pub py_upstream: String,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Settings {
    pub fn from_env() -> anyhow::Result<Self> {
        Ok(Settings {
            bind: env("KIWI_BIND").unwrap_or_else(|| "0.0.0.0:8000".into()),
            py_upstream: env("PY_UPSTREAM")
                .context("PY_UPSTREAM is required (the Python app to proxy unported routes to)")?
                .trim_end_matches('/')
                .to_owned(),
        })
    }
}
