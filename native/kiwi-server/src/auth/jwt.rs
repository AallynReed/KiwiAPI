//! HS256 JWTs exactly as PyJWT 2.15 writes and checks them (zero leeway, iat/nbf/exp/aud/sub/jti rules).

use base64::{
    alphabet,
    engine::{general_purpose::URL_SAFE_NO_PAD, DecodePaddingMode, GeneralPurpose, GeneralPurposeConfig},
    Engine,
};
use chrono::{DateTime, Utc};
use hmac::{Hmac, KeyInit, Mac};
use serde_json::{Map, Value};
use sha2::Sha256;

use super::pyutil::{dumps_ascii, int_of};

pub type Claims = Map<String, Value>;

const HEADER: &str = r#"{"alg":"HS256","typ":"JWT"}"#;

/// base64.urlsafe_b64decode after PyJWT's re-padding: padding optional,
/// non-canonical trailing bits tolerated.
const LENIENT: GeneralPurpose = GeneralPurpose::new(
    &alphabet::URL_SAFE,
    GeneralPurposeConfig::new()
        .with_decode_padding_mode(DecodePaddingMode::Indifferent)
        .with_decode_allow_trailing_bits(true),
);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum JwtError {
    Malformed,
    Algorithm,
    Signature,
    Expired,
    Immature,
    Claim,
}

impl std::fmt::Display for JwtError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(self, f)
    }
}

impl std::error::Error for JwtError {}

fn mac(secret: &str) -> Hmac<Sha256> {
    <Hmac<Sha256> as KeyInit>::new_from_slice(secret.as_bytes()).expect("HMAC takes any key length")
}

/// `jwt.encode(payload, secret, algorithm="HS256")` for a payload whose time
/// claims are already ints.
pub fn encode(claims: &Claims, secret: &str) -> String {
    let payload = dumps_ascii(&Value::Object(claims.clone()));
    let mut out = URL_SAFE_NO_PAD.encode(HEADER);
    out.push('.');
    out.push_str(&URL_SAFE_NO_PAD.encode(payload));
    let mut m = mac(secret);
    m.update(out.as_bytes());
    let sig = m.finalize().into_bytes();
    out.push('.');
    out.push_str(&URL_SAFE_NO_PAD.encode(sig));
    out
}

/// `timegm(dt.utctimetuple())`: whole seconds, fraction dropped.
pub fn int_date(dt: DateTime<Utc>) -> i64 {
    dt.timestamp()
}

/// Session access token claims, in Python's insertion order:
/// `sub, ver, iat, exp, type[, kind][, sid]`.
pub fn access_claims(
    subject: &str,
    token_version: i64,
    now: DateTime<Utc>,
    expire: DateTime<Utc>,
    kind: Option<&str>,
    session_id: Option<&str>,
) -> Claims {
    let mut c = Claims::new();
    c.insert("sub".into(), subject.into());
    c.insert("ver".into(), token_version.into());
    c.insert("iat".into(), int_date(now).into());
    c.insert("exp".into(), int_date(expire).into());
    c.insert("type".into(), "access".into());
    if let Some(kind) = kind {
        c.insert("kind".into(), kind.into());
    }
    if let Some(sid) = session_id {
        c.insert("sid".into(), sid.into());
    }
    c
}

/// `create_access_token` (dev portal) at an explicit `now`.
pub fn create_access_token_at(
    secret: &str,
    subject: &str,
    token_version: i64,
    session_id: Option<&str>,
    ttl: chrono::Duration,
    now: DateTime<Utc>,
) -> String {
    encode(&access_claims(subject, token_version, now, now + ttl, None, session_id), secret)
}

/// `create_site_access_token` at an explicit `now` (adds `kind: "site"`).
pub fn create_site_access_token_at(
    secret: &str,
    subject: &str,
    token_version: i64,
    session_id: Option<&str>,
    ttl: chrono::Duration,
    now: DateTime<Utc>,
) -> String {
    encode(&access_claims(subject, token_version, now, now + ttl, Some("site"), session_id), secret)
}

/// `create_email_token`: `{sub, type: purpose, iat, exp}` then `extra`
/// merged like `dict.update` (existing keys keep their position).
pub fn create_email_token_at(
    secret: &str,
    subject: &str,
    purpose: &str,
    ttl: chrono::Duration,
    extra: Option<&Claims>,
    now: DateTime<Utc>,
) -> String {
    let mut c = Claims::new();
    c.insert("sub".into(), subject.into());
    c.insert("type".into(), purpose.into());
    c.insert("iat".into(), int_date(now).into());
    c.insert("exp".into(), int_date(now + ttl).into());
    if let Some(extra) = extra {
        for (k, v) in extra {
            c.insert(k.clone(), v.clone());
        }
    }
    encode(&c, secret)
}

/// `jwt.decode(token, secret, algorithms=["HS256"])` evaluated at `now`
/// (float epoch seconds, as PyJWT takes it).
pub fn decode_at(token: &str, secret: &str, now: f64) -> Result<Claims, JwtError> {
    let (signing_input, sig_seg) = token.rsplit_once('.').ok_or(JwtError::Malformed)?;
    let (header_seg, payload_seg) = signing_input.split_once('.').ok_or(JwtError::Malformed)?;

    let header_raw = LENIENT.decode(header_seg).map_err(|_| JwtError::Malformed)?;
    let header: Value = serde_json::from_slice(&header_raw).map_err(|_| JwtError::Malformed)?;
    let header = header.as_object().ok_or(JwtError::Malformed)?;
    let b64 = header.get("b64");
    if b64 == Some(&Value::Bool(false)) {
        return Err(JwtError::Malformed);
    }
    let payload_raw = LENIENT.decode(payload_seg).map_err(|_| JwtError::Malformed)?;
    let sig = LENIENT.decode(sig_seg).map_err(|_| JwtError::Malformed)?;

    if header.get("kid").is_some_and(|kid| !kid.is_string()) {
        return Err(JwtError::Malformed);
    }
    if let Some(crit) = header.get("crit") {
        let ok = crit.as_array().is_some_and(|items| {
            !items.is_empty() && items.iter().all(|e| e.as_str() == Some("b64") && header.contains_key("b64"))
        });
        if !ok {
            return Err(JwtError::Malformed);
        }
    }
    if header.get("alg").and_then(Value::as_str) != Some("HS256") {
        return Err(JwtError::Algorithm);
    }
    let mut m = mac(secret);
    m.update(signing_input.as_bytes());
    m.verify_slice(&sig).map_err(|_| JwtError::Signature)?;

    let payload: Value = serde_json::from_slice(&payload_raw).map_err(|_| JwtError::Malformed)?;
    let Value::Object(claims) = payload else {
        return Err(JwtError::Malformed);
    };
    validate_claims(&claims, now)?;
    Ok(claims)
}

fn validate_claims(c: &Claims, now: f64) -> Result<(), JwtError> {
    if let Some(iat) = c.get("iat") {
        let iat = int_of(iat).ok_or(JwtError::Claim)?;
        if iat as f64 > now {
            return Err(JwtError::Immature);
        }
    }
    if let Some(nbf) = c.get("nbf") {
        let nbf = int_of(nbf).ok_or(JwtError::Claim)?;
        if nbf as f64 > now {
            return Err(JwtError::Immature);
        }
    }
    if let Some(exp) = c.get("exp") {
        let exp = int_of(exp).ok_or(JwtError::Claim)?;
        if exp as f64 <= now {
            return Err(JwtError::Expired);
        }
    }
    if c.get("aud").is_some_and(truthy) {
        return Err(JwtError::Claim);
    }
    if c.get("sub").is_some_and(|s| !s.is_string()) || c.get("jti").is_some_and(|s| !s.is_string()) {
        return Err(JwtError::Claim);
    }
    Ok(())
}

fn truthy(v: &Value) -> bool {
    match v {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64() != Some(0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

pub fn now_f64() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs_f64()).unwrap_or(0.0)
}

/// `decode_access_token` / the decode half of `decode_email_token`.
pub fn decode(token: &str, secret: &str) -> Result<Claims, JwtError> {
    decode_at(token, secret, now_f64())
}

/// `decode_email_token`: also requires `type == expected_purpose`.
pub fn decode_email_token(token: &str, secret: &str, expected_purpose: &str) -> Result<Claims, JwtError> {
    let claims = decode(token, secret)?;
    if claims.get("type").and_then(Value::as_str) != Some(expected_purpose) {
        return Err(JwtError::Claim);
    }
    Ok(claims)
}
