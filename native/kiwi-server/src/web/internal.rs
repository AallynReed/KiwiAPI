//! `app.core.internal_api.internal_get`: a GET to the api over the compose
//! network that never fails, only answers `None` (non-2xx, timeout, not JSON).

use std::time::Duration;

use serde_json::Value;

#[derive(Clone)]
pub struct InternalApi {
    base: String,
    client: reqwest::Client,
}

impl InternalApi {
    pub fn new(base: &str) -> Self {
        let client =
            crate::http::client_builder().redirect(reqwest::redirect::Policy::none()).build().expect("reqwest client");
        InternalApi { base: base.trim_end_matches('/').to_owned(), client }
    }

    pub fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base)
    }

    /// `internal_get(path, params, timeout=..., headers=...)`; httpx's timeout
    /// is per phase (connect, read), not a total.
    pub async fn get(
        &self,
        path: &str,
        params: &[(&str, String)],
        timeout: f64,
        headers: &[(&str, &str)],
    ) -> Option<Value> {
        let t = Duration::from_secs_f64(timeout);
        let mut req = self.client.get(self.url(path)).timeout(t * 2);
        if !params.is_empty() {
            req = req.query(params);
        }
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let res = match tokio::time::timeout(t * 2, req.send()).await {
            Ok(Ok(r)) if r.status().is_success() => r,
            Ok(Ok(r)) => {
                tracing::warn!("internal GET {path} failed: {}", r.status());
                return None;
            }
            _ => {
                tracing::warn!("internal GET {path} failed");
                return None;
            }
        };
        let bytes = res.bytes().await.ok()?;
        match crate::pyjson::loads(&bytes) {
            Ok(v) => Some(v),
            Err(_) => {
                tracing::warn!("internal GET {path} failed: not JSON");
                None
            }
        }
    }

    /// The JSON object at `path`, if it is one.
    pub async fn get_obj(&self, path: &str, params: &[(&str, String)]) -> Option<serde_json::Map<String, Value>> {
        match self.get(path, params, 8.0, &[]).await? {
            Value::Object(m) => Some(m),
            _ => None,
        }
    }

    /// POST a JSON body (the page-view beacon); errors are the caller's.
    pub async fn post_json(
        &self,
        path: &str,
        body: String,
        headers: &[(&str, &str)],
        timeout: f64,
    ) -> anyhow::Result<()> {
        let mut req = self
            .client
            .post(self.url(path))
            .timeout(Duration::from_secs_f64(timeout) * 2)
            .header("content-type", "application/json")
            .body(body);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        req.send().await?.error_for_status()?;
        Ok(())
    }
}
