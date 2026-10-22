//! API tokens (`<prefix>_<43 base62>_<base62 crc32>`), refresh tokens,
//! sha256 lookup hashes and the HMAC IP allowlist from `app/core/ip_hash.py`.

use std::net::IpAddr;

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use hmac::{Hmac, KeyInit, Mac};
use sha2::{Digest, Sha256};

/// `string.ascii_letters + string.digits` (lowercase first, NOT 0-9A-Za-z).
pub const ALPHABET: &[u8; 62] = b"abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
pub const BODY_LEN: usize = 43;
pub const CHECKSUM_MAX: usize = 6;

pub fn b62_encode(mut n: u64) -> String {
    if n == 0 {
        return (ALPHABET[0] as char).to_string();
    }
    let mut out = Vec::new();
    while n > 0 {
        out.push(ALPHABET[(n % 62) as usize]);
        n /= 62;
    }
    out.reverse();
    String::from_utf8(out).expect("ascii")
}

/// base62 of `zlib.crc32(body)`.
pub fn token_checksum(body: &str) -> String {
    b62_encode(u64::from(crc32fast::hash(body.as_bytes())))
}

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut buf = [0u8; N];
    getrandom::fill(&mut buf).expect("OS RNG");
    buf
}

/// `secrets.token_urlsafe(n)`.
pub fn token_urlsafe(n: usize) -> String {
    let mut buf = vec![0u8; n];
    getrandom::fill(&mut buf).expect("OS RNG");
    URL_SAFE_NO_PAD.encode(buf)
}

/// One uniform pick from the alphabet (rejection sampling, like `secrets.choice`).
fn random_alnum() -> u8 {
    loop {
        let [b] = random_bytes::<1>();
        if b < 248 {
            return ALPHABET[(b % 62) as usize];
        }
    }
}

/// `generate_api_token`: (full token, sha256 hex, display prefix).
pub fn generate_api_token(prefix: &str) -> (String, String, String) {
    let body: String = (0..BODY_LEN).map(|_| random_alnum() as char).collect();
    let full = format!("{prefix}_{body}_{}", token_checksum(&body));
    let hash = hash_token(&full);
    (full, hash, format!("{prefix}_{}", &body[..8]))
}

/// `verify_token_checksum`: `Some(true)` good, `Some(false)` our exact shape
/// but a bad checksum (reject offline), `None` not our shape (legacy, look it up).
pub fn verify_token_checksum(token: &str, prefix: &str) -> Option<bool> {
    // `re.match(...$)` also matches before one trailing newline.
    let token = token.strip_suffix('\n').unwrap_or(token);
    let rest = token.strip_prefix(prefix)?.strip_prefix('_')?;
    let (body, crc) = rest.split_at_checked(BODY_LEN)?;
    let crc = crc.strip_prefix('_')?;
    let alnum = |s: &str| s.bytes().all(|b| b.is_ascii_alphanumeric());
    if !alnum(body) || crc.is_empty() || crc.len() > CHECKSUM_MAX || !alnum(crc) {
        return None;
    }
    Some(token_checksum(body) == crc)
}

/// sha256 hex of the token's UTF-8 bytes (the lookup key, not a password hash).
pub fn hash_token(token: &str) -> String {
    hex::encode(Sha256::digest(token.as_bytes()))
}

/// `generate_refresh_token`: (raw, sha256 hex).
pub fn generate_refresh_token() -> (String, String) {
    let raw = token_urlsafe(48);
    let hash = hash_token(&raw);
    (raw, hash)
}

/// First 16 hex chars of sha256 (reset/email-change link fingerprints).
pub fn password_fingerprint(value: &str) -> String {
    hash_token(value)[..16].to_owned()
}

/// `hmac.compare_digest`.
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

pub fn make_ip_salt() -> String {
    token_urlsafe(16)
}

#[derive(Debug, PartialEq, Eq)]
pub enum IpError {
    Cidr,
    Invalid,
}

pub const CIDR_MESSAGE: &str =
    "CIDR ranges aren't supported on token IP allowlists (hashes can't range-match); pin specific addresses instead.";

/// `normalize_ip`: `str(ipaddress.ip_address(value.strip()))`. IPv4-mapped
/// IPv6 prints as `::ffff:a.b.c.d`, which is Python 3.13's spelling.
pub fn normalize_ip(value: &str) -> Result<String, IpError> {
    let value = super::pyutil::strip(value);
    if value.contains('/') {
        return Err(IpError::Cidr);
    }
    value.parse::<IpAddr>().map(|ip| ip.to_string()).map_err(|_| IpError::Invalid)
}

/// Hex HMAC-SHA256(key = salt, msg = normalized ip).
pub fn hash_ip(salt: &str, ip: &str) -> String {
    let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(salt.as_bytes()).expect("any key length");
    m.update(ip.as_bytes());
    hex::encode(m.finalize().into_bytes())
}

pub fn ip_allowed(client_ip: &str, salt: Option<&str>, allowed_hashes: &[String]) -> bool {
    let Some(salt) = salt.filter(|s| !s.is_empty()) else {
        return false;
    };
    if client_ip.is_empty() {
        return false;
    }
    let Ok(normalized) = normalize_ip(client_ip) else {
        return false;
    };
    let candidate = hash_ip(salt, &normalized);
    allowed_hashes.iter().any(|h| ct_eq(candidate.as_bytes(), h.as_bytes()))
}
