//! `app/scanning`: the GitHub secret-scanning partner webhook. The raw body is
//! checked against GitHub's published ECDSA keys (cached in-process, refetched
//! for an unknown key id), then every reported token that is really ours is
//! revoked.

use std::{
    collections::HashMap,
    sync::{LazyLock, Mutex},
    time::Duration,
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use mongodb::bson::doc;
use ring::signature::{ECDSA_P256_SHA256_ASN1, ECDSA_P384_SHA256_ASN1, UnparsedPublicKey};
use serde_json::{Value, json};

use super::{
    emails,
    store::{find_one, save_token, user_by_id},
};
use crate::{
    auth::{
        Rejection,
        models::{API_TOKENS, ApiToken},
        pyutil::latin1,
        tokens::{hash_token, verify_token_checksum},
    },
    dt,
    pyjson::{LoadsError, PydanticJson},
    state::AppState,
};

pub const KEY_ID_HEADER: &str = "github-public-key-identifier";
pub const SIGNATURE_HEADER: &str = "github-public-key-signature";
const REVOKE_REASON: &str = "Exposed publicly (detected by secret scanning)";

static KEYS: LazyLock<Mutex<HashMap<String, String>>> = LazyLock::new(|| Mutex::new(HashMap::new()));

/// `base64.b64decode(s)` as Python 3.13's non-strict `a2b_base64` does it:
/// characters outside the alphabet are skipped, `=` never ends the data, and
/// a final partial group must be followed by enough `=` to complete it.
pub fn b64decode(s: &str) -> Option<Vec<u8>> {
    if !s.is_ascii() {
        return None;
    }
    let (mut quad, mut pads, mut left) = (0u8, 0u32, 0u32);
    let mut out = Vec::new();
    for b in s.bytes() {
        if b == b'=' {
            pads += 1;
            continue;
        }
        let v = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        pads = 0;
        let v = u32::from(v);
        match quad {
            0 => {
                left = v;
                quad = 1;
            }
            1 => {
                out.push(((left << 2) | (v >> 4)) as u8);
                left = v & 0x0f;
                quad = 2;
            }
            2 => {
                out.push(((left << 4) | (v >> 2)) as u8);
                left = v & 0x03;
                quad = 3;
            }
            _ => {
                out.push(((left << 6) | v) as u8);
                quad = 0;
            }
        }
    }
    match quad {
        0 => Some(out),
        1 => None,
        q => (u32::from(q) + pads >= 4).then_some(out),
    }
}

fn der_tlv(buf: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, rest) = buf.split_first()?;
    let (&len0, mut rest) = rest.split_first()?;
    let len = if len0 < 0x80 {
        usize::from(len0)
    } else {
        let n = usize::from(len0 & 0x7f);
        if n == 0 || n > 4 || rest.len() < n {
            return None;
        }
        let len = rest[..n].iter().fold(0usize, |acc, &b| (acc << 8) | usize::from(b));
        rest = &rest[n..];
        len
    };
    (rest.len() >= len).then(|| (tag, &rest[..len], &rest[len..]))
}

const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Curve {
    P256,
    P384,
}

/// A PEM `PUBLIC KEY` (SPKI) holding an EC point; `None` for anything else.
pub fn ec_public_key(pem: &str) -> Option<(Curve, Vec<u8>)> {
    let body = pem.split("-----BEGIN PUBLIC KEY-----").nth(1)?.split("-----END PUBLIC KEY-----").next()?;
    let b64: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let der = base64::Engine::decode(&base64::engine::general_purpose::STANDARD, b64).ok()?;
    let (0x30, spki, _) = der_tlv(&der)? else { return None };
    let (0x30, alg, rest) = der_tlv(spki)? else { return None };
    let (0x03, bits, _) = der_tlv(rest)? else { return None };
    let (0x06, oid, alg_rest) = der_tlv(alg)? else { return None };
    if oid != EC_PUBLIC_KEY {
        return None;
    }
    let (0x06, curve, _) = der_tlv(alg_rest)? else { return None };
    let curve = match curve {
        c if c == P256 => Curve::P256,
        c if c == P384 => Curve::P384,
        _ => return None,
    };
    let (&0, point) = bits.split_first()? else { return None };
    Some((curve, point.to_vec()))
}

/// ECDSA-SHA256 over `body` with a DER signature.
pub fn verify_with_pem(pem: &str, body: &[u8], sig: &[u8]) -> bool {
    let Some((curve, point)) = ec_public_key(pem) else {
        return false;
    };
    let alg = match curve {
        Curve::P256 => &ECDSA_P256_SHA256_ASN1,
        Curve::P384 => &ECDSA_P384_SHA256_ASN1,
    };
    UnparsedPublicKey::new(alg, point).verify(body, sig).is_ok()
}

async fn fetch_keys(url: &str) -> Option<HashMap<String, String>> {
    let client = crate::http::client_builder()
        .timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(crate::http::KIWI_UA)
        .build()
        .ok()?;
    let resp = client.get(url).send().await.ok()?;
    if !resp.status().is_success() {
        return None;
    }
    let data: Value = crate::pyjson::loads(&resp.bytes().await.ok()?).ok()?;
    let mut out = HashMap::new();
    if let Some(keys) = data.get("public_keys").and_then(Value::as_array) {
        for k in keys {
            let id = k.get("key_identifier")?.as_str()?;
            let pem = k.get("key")?.as_str()?;
            out.insert(id.to_owned(), pem.to_owned());
        }
    }
    Some(out)
}

async fn key_for(url: &str, key_id: &str) -> Option<String> {
    if let Some(pem) = KEYS.lock().unwrap_or_else(|e| e.into_inner()).get(key_id) {
        return Some(pem.clone());
    }
    let Some(fresh) = fetch_keys(url).await else {
        tracing::error!("Failed to fetch GitHub secret-scanning public keys");
        return None;
    };
    let mut cache = KEYS.lock().unwrap_or_else(|e| e.into_inner());
    cache.extend(fresh);
    cache.get(key_id).cloned()
}

/// `verify_signature(body, key_id, signature_b64)`.
pub async fn verify_signature(url: &str, body: &[u8], headers: &HeaderMap) -> bool {
    let get = |name| headers.get(name).map(latin1).filter(|v| !v.is_empty());
    let (Some(key_id), Some(sig)) = (get(KEY_ID_HEADER), get(SIGNATURE_HEADER)) else {
        return false;
    };
    let Some(pem) = key_for(url, &key_id).await else {
        return false;
    };
    let Some(sig) = b64decode(&sig) else {
        return false;
    };
    verify_with_pem(&pem, body, &sig)
}

/// Python `str(x)` of a JSON value.
pub fn py_str(v: &Value) -> String {
    match v {
        Value::String(s) => s.clone(),
        other => py_repr(other),
    }
}

fn py_repr(v: &Value) -> String {
    match v {
        Value::Null => "None".into(),
        Value::Bool(true) => "True".into(),
        Value::Bool(false) => "False".into(),
        Value::Number(n) => match (n.as_i64(), n.as_u64()) {
            (Some(i), _) => i.to_string(),
            (_, Some(u)) => u.to_string(),
            _ => crate::pyfloat::repr(n.as_f64().unwrap_or(f64::NAN)),
        },
        Value::String(s) => kiwi_core::pystr::repr(s),
        Value::Array(items) => format!("[{}]", items.iter().map(py_repr).collect::<Vec<_>>().join(", ")),
        Value::Object(m) => format!(
            "{{{}}}",
            m.iter()
                .map(|(k, v)| format!("{}: {}", kiwi_core::pystr::repr(k), py_repr(v)))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

async fn evaluate_and_revoke(state: &AppState, token: &str) -> Result<&'static str, Rejection> {
    let prefix = &state.settings().api_token_prefix;
    if verify_token_checksum(token, prefix) == Some(false) {
        return Ok("false_positive");
    }
    let found: Option<ApiToken> = find_one(state, API_TOKENS, doc! { "hashed_token": hash_token(token) }).await?;
    let Some(mut doc) = found else {
        return Ok("false_positive");
    };
    if !doc.revoked {
        doc.revoked = true;
        doc.revoked_at = Some(dt::to_bson(dt::now()));
        doc.revoke_reason = Some(REVOKE_REASON.into());
        save_token(state, &doc).await?;
        tracing::warn!("Auto-revoked leaked token {} ({})", doc.id, doc.prefix);
        if let Some(user) = user_by_id(state, doc.user_id).await?
            && state.settings().security_email_notifications
        {
            emails::background(state, move |st| {
                Some(emails::token_compromised_email(st.settings(), &user, &doc.name, &doc.prefix))
            });
        }
    }
    Ok("true_positive")
}

pub async fn github(State(state): State<AppState>, headers: HeaderMap, body: Bytes) -> Response {
    let run = async {
        let s = state.settings();
        if s.github_secret_scanning_verify
            && !verify_signature(&s.github_secret_scanning_keys_url, &body, &headers).await
        {
            return Err(Rejection::new(
                StatusCode::UNAUTHORIZED,
                "not_authenticated",
                "Invalid or missing secret-scanning signature",
            ));
        }
        let bad = |m: &str| Rejection::new(StatusCode::BAD_REQUEST, "bad_request", m);
        let matches = match crate::pyjson::loads(&body) {
            Ok(v) => v,
            Err(LoadsError::Syntax { .. }) => return Err(bad("Body must be JSON")),
            Err(e) => return Err(Rejection::internal(format!("secret-scanning body: {e:?}"))),
        };
        let Value::Array(matches) = matches else {
            return Err(bad("Body must be a JSON array of token matches"));
        };
        let mut out = Vec::new();
        for m in &matches {
            let Some(obj) = m.as_object() else { continue };
            let Some(token) = obj.get("token").filter(|t| crate::features::py_truthy(t)) else { continue };
            let token = py_str(token);
            let token_type = match obj.get("type").filter(|t| crate::features::py_truthy(t)) {
                Some(t) => py_str(t),
                None => "kiwi_api_token".into(),
            };
            let label = evaluate_and_revoke(&state, &token).await?;
            out.push(json!({ "token_raw": token, "token_type": token_type, "label": label }));
        }
        Ok(PydanticJson(Value::Array(out)).into_response())
    };
    run.await.unwrap_or_else(IntoResponse::into_response)
}
