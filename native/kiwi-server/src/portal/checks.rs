//! Signup/login guards: captcha (`app/core/captcha.py`), the HIBP breach check
//! (`app/core/passwords.py`) and the disposable-domain blocklist
//! (`app/auth/disposable.py`).

use std::time::Duration;

use axum::http::StatusCode;
use serde_json::Value;

use crate::{
    auth::{
        Rejection,
        passwords::{breached, hibp_count, hibp_prefix_suffix},
    },
    config::Settings,
    features::py_truthy,
};

pub const DISPOSABLE_DOMAINS: [&str; 69] = [
    "0clock.net",
    "10minutemail.com",
    "10minutemail.net",
    "20minutemail.com",
    "33mail.com",
    "anonbox.net",
    "burnermail.io",
    "byom.de",
    "deadaddress.com",
    "discard.email",
    "dispostable.com",
    "dropmail.me",
    "emailondeck.com",
    "emailtemporanea.com",
    "fakeinbox.com",
    "fakemail.net",
    "fakemailgenerator.com",
    "getairmail.com",
    "getnada.com",
    "grr.la",
    "guerrillamail.biz",
    "guerrillamail.com",
    "guerrillamail.de",
    "guerrillamail.net",
    "guerrillamail.org",
    "guerrillamailblock.com",
    "harakirimail.com",
    "inboxbear.com",
    "inboxkitten.com",
    "incognitomail.com",
    "jetable.org",
    "mail-temp.com",
    "mail7.io",
    "mailcatch.com",
    "maildrop.cc",
    "maileater.com",
    "mailinator.com",
    "mailinator.net",
    "mailnesia.com",
    "mailsac.com",
    "mailtothis.com",
    "mintemail.com",
    "mohmal.com",
    "moakt.com",
    "mvrht.com",
    "mytemp.email",
    "nada.email",
    "nwytg.net",
    "owlymail.com",
    "sharklasers.com",
    "shitmail.org",
    "spam4.me",
    "spamgourmet.com",
    "tempinbox.com",
    "temp-mail.io",
    "temp-mail.org",
    "tempmail.com",
    "tempmail.dev",
    "tempmailo.com",
    "tempr.email",
    "throwawaymail.com",
    "trashmail.com",
    "trashmail.de",
    "trbvm.com",
    "vomoto.com",
    "wegwerfmail.de",
    "yopmail.com",
    "yopmail.fr",
    "yopmail.net",
];

/// `is_disposable_email` on an already-lowercased, validated address.
pub fn is_disposable(email: &str) -> bool {
    let domain = email.rsplit('@').next().unwrap_or_default();
    let domain = kiwi_core::pystr::strip(domain).to_lowercase();
    DISPOSABLE_DOMAINS.contains(&domain.as_str())
}

fn verify_url(provider: &str) -> Option<&'static str> {
    match provider {
        "hcaptcha" => Some("https://api.hcaptcha.com/siteverify"),
        "turnstile" => Some("https://challenges.cloudflare.com/turnstile/v0/siteverify"),
        _ => None,
    }
}

fn set(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// `verify_captcha(token, remote_ip)`: skipped (true) unless both keys are
/// set; a provider reply that isn't a JSON object is an unhandled error.
pub async fn verify_captcha(s: &Settings, token: Option<&str>, remote_ip: Option<&str>) -> Result<bool, Rejection> {
    let (Some(secret), Some(_)) = (set(&s.captcha_secret), set(&s.captcha_sitekey)) else {
        return Ok(true);
    };
    let Some(url) = verify_url(&s.captcha_provider) else {
        return Ok(false);
    };
    let Some(token) = token.filter(|t| !t.is_empty()) else {
        return Ok(false);
    };
    let mut form = vec![("secret", secret), ("response", token)];
    if let Some(ip) = remote_ip.filter(|ip| !ip.is_empty()) {
        form.push(("remoteip", ip));
    }
    let reply = async {
        let client = crate::http::client_builder()
            .timeout(Duration::from_secs(10))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(crate::http::KIWI_UA)
            .build()?;
        let resp = client
            .post(url)
            .header(reqwest::header::CONTENT_TYPE, "application/x-www-form-urlencoded")
            .body(super::oauth::urlencode(&form))
            .send()
            .await?;
        resp.bytes().await
    };
    let body = match reply.await {
        Ok(b) => b,
        Err(err) => {
            tracing::warn!("Captcha verification request failed: {err}");
            return Ok(false);
        }
    };
    let Ok(result) = crate::pyjson::loads(&body) else {
        return Ok(false);
    };
    match result {
        Value::Object(m) => Ok(m.get("success").is_some_and(py_truthy)),
        _ => Err(Rejection::internal("captcha reply is not a JSON object")),
    }
}

pub fn captcha_failed() -> Rejection {
    Rejection::new(StatusCode::BAD_REQUEST, "captcha_failed", "Captcha verification failed")
}

/// `password_breach_count`: fails open (0) on any transport or HTTP error.
pub async fn breach_count(password: &str) -> i64 {
    let (prefix, suffix) = hibp_prefix_suffix(password);
    let fetch = async {
        let client = crate::http::client_builder()
            .timeout(Duration::from_secs(5))
            .redirect(reqwest::redirect::Policy::none())
            .user_agent(crate::http::KIWI_UA)
            .build()?;
        let resp = client
            .get(format!("https://api.pwnedpasswords.com/range/{prefix}"))
            .header("Add-Padding", "true")
            .send()
            .await?;
        let ok = resp.status().is_success();
        Ok::<_, reqwest::Error>(ok.then_some(resp.text().await?))
    };
    match fetch.await {
        Ok(Some(text)) => hibp_count(&text, &suffix),
        _ => {
            tracing::warn!("HIBP lookup failed - allowing the password");
            0
        }
    }
}

/// `ensure_password_not_breached`.
pub async fn ensure_not_breached(s: &Settings, password: &str) -> Result<(), Rejection> {
    if !s.password_breach_check {
        return Ok(());
    }
    match breach_count(password).await {
        0 => Ok(()),
        n => Err(breached(n)),
    }
}
