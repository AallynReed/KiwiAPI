//! argon2-cffi `PasswordHasher()` hashes and PHC parsing as its C decoder does it, plus the HIBP halves.

use argon2::{Algorithm, Argon2, Params, Version};
use axum::http::StatusCode;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine};
use sha1::{Digest, Sha1};

use super::Rejection;

pub const MEMORY_COST: u32 = 65536;
pub const TIME_COST: u32 = 3;
pub const PARALLELISM: u32 = 4;
pub const HASH_LEN: usize = 32;
pub const SALT_LEN: usize = 16;

fn type_name(alg: Algorithm) -> &'static str {
    match alg {
        Algorithm::Argon2d => "argon2d",
        Algorithm::Argon2i => "argon2i",
        Algorithm::Argon2id => "argon2id",
    }
}

/// `PasswordHasher().hash(password)`. CPU-heavy: call from `spawn_blocking`.
pub fn hash_password(password: &str) -> String {
    let mut salt = [0u8; SALT_LEN];
    getrandom::fill(&mut salt).expect("OS RNG");
    hash_password_with_salt(password, &salt)
}

/// `argon2.low_level.hash_secret` with the PasswordHasher defaults.
pub fn hash_password_with_salt(password: &str, salt: &[u8]) -> String {
    let params = Params::new(MEMORY_COST, TIME_COST, PARALLELISM, Some(HASH_LEN)).expect("valid params");
    let mut out = [0u8; HASH_LEN];
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut out)
        .expect("argon2 hash");
    encode_phc(Algorithm::Argon2id, Version::V0x13, MEMORY_COST, TIME_COST, PARALLELISM, salt, &out)
}

fn encode_phc(alg: Algorithm, ver: Version, m: u32, t: u32, p: u32, salt: &[u8], hash: &[u8]) -> String {
    format!(
        "${}$v={}$m={m},t={t},p={p}${}${}",
        type_name(alg),
        ver as u32,
        STANDARD_NO_PAD.encode(salt),
        STANDARD_NO_PAD.encode(hash)
    )
}

/// [`hash_password`] off the async runtime.
pub async fn hash_password_blocking(password: String) -> String {
    tokio::task::spawn_blocking(move || hash_password(&password)).await.expect("argon2 task")
}

/// [`verify_password`] off the async runtime.
pub async fn verify_password_blocking(password: String, hashed: String) -> bool {
    tokio::task::spawn_blocking(move || verify_password(&password, &hashed)).await.unwrap_or(false)
}

/// `verify_password`: false on a mismatch or any decoding/parameter error.
/// CPU-heavy: call from `spawn_blocking`.
pub fn verify_password(password: &str, hashed: &str) -> bool {
    let Some(alg) = header_type(hashed) else {
        return false;
    };
    let Some(d) = decode_phc(hashed, alg) else {
        return false;
    };
    let Ok(params) = Params::new(d.m, d.t, d.p, Some(d.hash.len())) else {
        return false;
    };
    let mut out = vec![0u8; d.hash.len()];
    if Argon2::new(alg, d.version, params).hash_password_into(password.as_bytes(), &d.salt, &mut out).is_err() {
        return false;
    }
    super::tokens::ct_eq(&out, &d.hash)
}

/// argon2-cffi picks the type from the first 9 bytes.
fn header_type(hash: &str) -> Option<Algorithm> {
    match hash.as_bytes().get(..9)? {
        b"$argon2i$" => Some(Algorithm::Argon2i),
        b"$argon2d$" => Some(Algorithm::Argon2d),
        b"$argon2id" => Some(Algorithm::Argon2id),
        _ => None,
    }
}

struct Decoded {
    version: Version,
    m: u32,
    t: u32,
    p: u32,
    salt: Vec<u8>,
    hash: Vec<u8>,
}

/// `decode_string` from the reference implementation's encoding.c.
fn decode_phc(s: &str, alg: Algorithm) -> Option<Decoded> {
    let mut rest = s.strip_prefix('$')?.strip_prefix(type_name(alg))?;
    let mut version = 0x10;
    if let Some(r) = rest.strip_prefix("$v=") {
        let (v, r) = decimal_u32(r)?;
        version = v;
        rest = r;
    }
    let (m, r) = decimal_u32(rest.strip_prefix("$m=")?)?;
    let (t, r) = decimal_u32(r.strip_prefix(",t=")?)?;
    let (p, r) = decimal_u32(r.strip_prefix(",p=")?)?;
    let (salt, r) = from_base64(r.strip_prefix('$')?)?;
    let (hash, r) = from_base64(r.strip_prefix('$')?)?;
    if !r.is_empty() || salt.len() < 8 || hash.len() < 4 || t < 1 || !(1..=0xFF_FFFF).contains(&p) || m < 8 * p {
        return None;
    }
    let version = Version::try_from(version).ok()?;
    Some(Decoded { version, m, t, p, salt, hash })
}

fn decimal_u32(s: &str) -> Option<(u32, &str)> {
    let end = s.bytes().position(|b| !b.is_ascii_digit()).unwrap_or(s.len());
    let digits = &s[..end];
    if digits.is_empty() || (digits.starts_with('0') && digits.len() > 1) {
        return None;
    }
    Some((digits.parse().ok()?, &s[end..]))
}

/// The C `from_base64`: standard alphabet, no padding, stops at the first
/// non-alphabet byte, rejects a dangling 6-bit group or non-zero leftover bits.
fn from_base64(s: &str) -> Option<(Vec<u8>, &str)> {
    let end = s.bytes().position(|b| !(b.is_ascii_alphanumeric() || b == b'+' || b == b'/')).unwrap_or(s.len());
    let (chunk, rest) = s.split_at(end);
    let mut out = Vec::with_capacity(chunk.len() * 3 / 4);
    let (mut acc, mut acc_len) = (0u32, 0u32);
    for b in chunk.bytes() {
        let d = match b {
            b'A'..=b'Z' => b - b'A',
            b'a'..=b'z' => b - b'a' + 26,
            b'0'..=b'9' => b - b'0' + 52,
            b'+' => 62,
            _ => 63,
        };
        acc = (acc << 6) | u32::from(d);
        acc_len += 6;
        if acc_len >= 8 {
            acc_len -= 8;
            out.push((acc >> acc_len) as u8);
            acc &= (1 << acc_len) - 1;
        }
    }
    if acc_len > 4 || acc != 0 {
        return None;
    }
    Some((out, rest))
}

/// The HIBP range query halves: (5-char SHA-1 prefix sent, suffix kept).
pub fn hibp_prefix_suffix(password: &str) -> (String, String) {
    let hex = hex::encode_upper(Sha1::digest(password.as_bytes()));
    (hex[..5].to_owned(), hex[5..].to_owned())
}

/// Breach count from a `range/{prefix}` response body (0 when absent).
pub fn hibp_count(body: &str, suffix: &str) -> i64 {
    for line in body.lines() {
        let (h, count) = line.split_once(':').unwrap_or((line, ""));
        if super::pyutil::strip(h) == suffix {
            return super::pyutil::strip(count).parse().unwrap_or(1);
        }
    }
    0
}

/// 400 `password_breached` for a non-zero count.
pub fn breached(count: i64) -> Rejection {
    Rejection::new(
        StatusCode::BAD_REQUEST,
        "password_breached",
        "This password has appeared in known data breaches - please choose a different one.",
    )
    .with_details(serde_json::json!({ "breach_count": count }))
}
