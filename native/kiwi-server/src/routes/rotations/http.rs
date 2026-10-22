//! Outbound GETs for the relay loops, like `app.core.http.fetch` / httpx:
//! redirects followed (20 at most), anything but a 2xx is an error.

use std::{sync::OnceLock, time::Duration};

use bytes::Bytes;

fn client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        crate::http::client_builder().redirect(reqwest::redirect::Policy::limited(20)).build().expect("reqwest client")
    })
}

pub async fn get(url: &str, headers: &[(&str, String)], timeout: Duration) -> anyhow::Result<Bytes> {
    let mut req = client().get(url).timeout(timeout);
    for (name, value) in headers {
        req = req.header(*name, value);
    }
    let resp = req.send().await?;
    let status = resp.status();
    if !status.is_success() {
        anyhow::bail!("GET {url}: HTTP {status}");
    }
    Ok(resp.bytes().await?)
}
