//! Golden tests from `testdata/gen_golden.py`; `KIWI_PY=<python.exe>` also runs `testdata/verify_rust.py`.

use std::sync::LazyLock;

use axum::http::{HeaderMap, HeaderValue};
use chrono::{DateTime, Duration, TimeZone, Utc};
use serde_json::{Value, json};

use super::{
    AuthSettings,
    client::{TrustedHosts, device_label, resolve_client},
    extract::bearer,
    jwt, passwords, scopes,
    sessions::TokenPair,
    site, tokens,
};
use crate::{
    ratelimit::{self, fixed_window, py_float_str, redis_outcome},
    usage::{fastapi_template, starlette_path},
};

static GOLDEN: LazyLock<Value> =
    LazyLock::new(|| serde_json::from_str(include_str!("testdata/golden.json")).expect("golden.json"));

fn g(key: &str) -> &'static Value {
    &GOLDEN[key]
}

fn arr(key: &str) -> &'static Vec<Value> {
    g(key).as_array().expect(key)
}

fn s(v: &Value) -> &str {
    v.as_str().expect("string")
}

fn secret() -> &'static str {
    s(g("secret"))
}

fn micros(t: f64) -> i64 {
    (t * 1e6).round_ties_even() as i64
}

fn at(t: f64) -> DateTime<Utc> {
    Utc.timestamp_micros(micros(t)).unwrap()
}

#[test]
fn jwt_encode_is_byte_identical() {
    let now = at(g("fixed_now").as_f64().unwrap());
    let ttl = Duration::minutes(15);
    for case in arr("jwt_encode") {
        let sub = s(&case["sub"]);
        let ver = case["ver"].as_i64().unwrap_or(0);
        let sid = case["sid"].as_str();
        let got = match s(&case["kind"]) {
            "access" => jwt::create_access_token_at(secret(), sub, ver, sid, ttl, now),
            "site" => jwt::create_site_access_token_at(secret(), sub, ver, sid, ttl, now),
            _ => jwt::create_email_token_at(
                secret(),
                sub,
                s(&case["purpose"]),
                Duration::seconds(case["seconds"].as_i64().unwrap()),
                case["extra"].as_object(),
                now,
            ),
        };
        assert_eq!(got, s(&case["token"]), "{case}");
        let claims = jwt::decode_at(&got, secret(), now.timestamp() as f64 + 1.0).expect("decodes");
        assert_eq!(claims["sub"], case["sub"]);
    }
}

#[test]
fn jwt_decode_matches_pyjwt() {
    let now = jwt::now_f64();
    for case in arr("jwt_decode") {
        let got = jwt::decode_at(s(&case["token"]), secret(), now);
        assert_eq!(got.is_ok(), case["ok"].as_bool().unwrap(), "{} -> {got:?}", case["name"]);
        if let Ok(claims) = got {
            assert_eq!(Value::Object(claims), case["payload"], "{}", case["name"]);
        }
    }
}

#[test]
fn email_token_purpose_is_checked() {
    let now = Utc::now();
    let tok = jwt::create_email_token_at(secret(), "x", "email_verify", Duration::hours(1), None, now);
    assert!(jwt::decode_email_token(&tok, secret(), "email_verify").is_ok());
    assert!(jwt::decode_email_token(&tok, secret(), "password_reset").is_err());
}

#[test]
fn argon2_hash_with_fixed_salt_matches_argon2_cffi() {
    use base64::{Engine, engine::general_purpose::STANDARD};
    let case = &arr("argon2_fixed")[0];
    let salt = STANDARD.decode(s(&case["salt_b64"])).unwrap();
    assert_eq!(passwords::hash_password_with_salt(s(&case["password"]), &salt), s(&case["hash"]));
}

#[test]
fn argon2_verifies_python_hashes() {
    for case in arr("argon2_fixed") {
        assert!(passwords::verify_password(s(&case["password"]), s(&case["hash"])), "{case}");
    }
    for case in arr("argon2_verify") {
        let pw = s(&case["password"]);
        let hash = s(&case["hash"]);
        assert_eq!(passwords::verify_password(pw, hash), case["ok"].as_bool().unwrap(), "{case}");
        assert_eq!(passwords::verify_password(&format!("{pw}x"), hash), case["wrong_ok"].as_bool().unwrap());
    }
}

#[test]
fn argon2_rejects_malformed_phc() {
    let good = s(&arr("argon2_verify")[1]["hash"]);
    for bad in [
        "",
        "plaintext",
        "$argon2id$v=19$m=65536,t=3,p=4$",
        &good.replace("m=1024", "m=01024"),
        &good.replace(",t=1", ",t=0"),
        &format!("{good}$"),
        &format!("{good}="),
        &good.replace("$v=19", "$v=18"),
    ] {
        assert!(!passwords::verify_password("cheap-pw", bad), "{bad}");
    }
}

#[test]
fn argon2_default_params_prefix() {
    let h = passwords::hash_password("x");
    assert!(h.starts_with(&format!("{}$", s(g("argon2_default_prefix")))), "{h}");
    assert_eq!(h.len(), 97);
}

#[test]
fn hibp_halves() {
    let (prefix, suffix) = passwords::hibp_prefix_suffix("password");
    assert_eq!(prefix, "5BAA6");
    assert_eq!(suffix, "1E4C9B93F3F0682250B6CF8331B7EE68FD8");
    let body = "0018A45C4D1DEF81644B54AB7F969B88D65:1\r\n1E4C9B93F3F0682250B6CF8331B7EE68FD8:10434004\r\n";
    assert_eq!(passwords::hibp_count(body, &suffix), 10434004);
    assert_eq!(passwords::hibp_count(body, "FFFF"), 0);
}

#[test]
fn api_token_format_matches_python() {
    for case in arr("b62") {
        assert_eq!(tokens::b62_encode(case["n"].as_u64().unwrap()), s(&case["s"]));
    }
    for case in arr("checksums") {
        assert_eq!(tokens::token_checksum(s(&case["body"])), s(&case["checksum"]));
    }
    for case in arr("generated_tokens") {
        let t = s(&case["token"]);
        assert_eq!(tokens::verify_token_checksum(t, "kiwi"), Some(true));
        assert_eq!(tokens::hash_token(t), s(&case["hash"]));
        assert_eq!(format!("kiwi_{}", &t[5..13]), s(&case["prefix"]));
    }
    for case in arr("checksum_verify") {
        assert_eq!(tokens::verify_token_checksum(s(&case["token"]), "kiwi"), case["result"].as_bool(), "{case}");
    }
    for case in arr("sha256") {
        assert_eq!(tokens::hash_token(s(&case["in"])), s(&case["hex"]));
    }
    for case in arr("fingerprint") {
        assert_eq!(tokens::password_fingerprint(s(&case["in"])), s(&case["fp"]));
    }
    let (full, hash, prefix) = tokens::generate_api_token("kiwi");
    assert_eq!(full.len(), "kiwi_".len() + 43 + 1 + tokens::token_checksum(&full[5..48]).len());
    assert_eq!(tokens::verify_token_checksum(&full, "kiwi"), Some(true));
    assert_eq!(hash, tokens::hash_token(&full));
    assert!(full.starts_with(&prefix) && prefix.len() == 13);
    let (raw, raw_hash) = tokens::generate_refresh_token();
    assert_eq!((raw.len(), raw_hash), (64, tokens::hash_token(&raw)));
}

#[test]
fn scopes_match_python() {
    assert_eq!(serde_json::to_string(scopes::catalog()).unwrap(), s(g("scope_catalog_json")));
    for case in arr("scope_masks") {
        let mask = case["mask"].as_i64().unwrap();
        assert_eq!(scopes::is_valid_mask(mask), case["valid"].as_bool().unwrap(), "{mask}");
        if let Some(decoded) = case["decode"].as_array() {
            let want: Vec<&str> = decoded.iter().map(s).collect();
            assert_eq!(scopes::decode(mask), want, "{mask}");
        }
        for (key, want) in case["grants"].as_object().unwrap() {
            assert_eq!(scopes::mask_grants(mask, key), want.as_bool().unwrap(), "{mask} {key}");
            if let Some(bit) = scopes::bit_for(key) {
                assert_eq!(scopes::mask_grants_bit(mask, bit), want.as_bool().unwrap());
            }
        }
    }
}

#[test]
fn ip_allowlist_matches_python() {
    for case in arr("normalize_ip") {
        let got = tokens::normalize_ip(s(&case["in"])).ok();
        assert_eq!(got.as_deref(), case["out"].as_str(), "{case}");
    }
    assert_eq!(tokens::normalize_ip("10.0.0.0/8"), Err(tokens::IpError::Cidr));
    for case in arr("ip_hash") {
        assert_eq!(tokens::hash_ip(s(&case["salt"]), s(&case["ip"])), s(&case["hex"]));
    }
    for case in arr("ip_allowed") {
        let allowed: Vec<String> = case["allowed"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
        let got = tokens::ip_allowed(s(&case["client"]), case["salt"].as_str(), &allowed);
        assert_eq!(got, case["ok"].as_bool().unwrap(), "{case}");
    }
    assert_eq!(tokens::make_ip_salt().len(), 22);
}

#[test]
fn device_labels_match_python() {
    for case in arr("device_label") {
        assert_eq!(device_label(case["ua"].as_str()).as_deref(), case["label"].as_str(), "{case}");
    }
}

#[test]
fn bearer_parsing_matches_fastapi() {
    for case in arr("bearer") {
        // Control characters never reach Python either (h11 rejects them).
        let Ok(value) = HeaderValue::from_bytes(s(&case["header"]).as_bytes()) else {
            continue;
        };
        let mut h = HeaderMap::new();
        h.insert("authorization", value);
        assert_eq!(bearer(&h).as_deref(), case["cred"].as_str(), "{case}");
    }
    assert_eq!(bearer(&HeaderMap::new()), None);
}

#[test]
fn site_cookies_match_starlette() {
    let tokens = TokenPair {
        access_token: "aaa.bbb.ccc".into(),
        refresh_token: "r-_efresh".into(),
        token_type: "bearer",
        expires_in: 900,
    };
    for case in arr("cookies") {
        let settings = AuthSettings {
            app_url: s(&case["app_url"]).into(),
            api_url: s(&case["api_url"]).into(),
            debug: case["debug"].as_bool().unwrap(),
            ..AuthSettings::default()
        };
        let want: Vec<&str> = case["set"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(site::session_cookie_lines(&settings, &tokens).to_vec(), want);
        let now = Utc.timestamp_opt(case["clear_now"].as_f64().unwrap().floor() as i64, 0).unwrap();
        let want: Vec<&str> = case["clear"].as_array().unwrap().iter().map(s).collect();
        assert_eq!(site::clear_cookie_lines(&settings, now).to_vec(), want);
        for a in case["allowed"].as_array().unwrap() {
            let mut h = HeaderMap::new();
            if let Some(o) = a["origin"].as_str() {
                h.insert("origin", HeaderValue::from_str(o).unwrap());
            }
            if let Some(f) = a["sec_fetch_site"].as_str() {
                h.insert("sec-fetch-site", HeaderValue::from_str(f).unwrap());
            }
            assert_eq!(site::cookie_auth_allowed(&h, &settings), a["ok"].as_bool().unwrap(), "{a}");
        }
    }
}

#[test]
fn cookie_header_parsing() {
    let mut h = HeaderMap::new();
    h.insert("cookie", HeaderValue::from_static("a=1; kiwi_site_access=x.y.z ; q=\"\\101b\\\"c\"; =v; flag"));
    let c = site::parse_cookies(&h);
    assert_eq!(c["kiwi_site_access"], "x.y.z");
    assert_eq!(c["q"], "Ab\"c");
    assert_eq!(c[""], "flag");
}

#[test]
fn client_ip_matches_uvicorn() {
    for case in arr("client_ip") {
        let trusted = TrustedHosts::parse(s(&case["trusted"]));
        let got = resolve_client(&trusted, Some(s(&case["peer"]).to_owned()), case["xff"].as_str());
        assert_eq!(got.as_deref(), Some(s(&case["client"])), "{case}");
    }
    assert_eq!(resolve_client(&TrustedHosts::parse("127.0.0.1"), None, Some("1.2.3.4")), None);
    assert_eq!(resolve_client(&TrustedHosts::parse("*"), None, Some("1.2.3.4")).as_deref(), Some("1.2.3.4"));
}

#[test]
fn rate_limit_math_matches_python() {
    for case in arr("ratelimit_redis") {
        let now = case["now"].as_f64().unwrap();
        let (window, limit, count) =
            (case["window"].as_i64().unwrap(), case["limit"].as_i64().unwrap(), case["count"].as_i64().unwrap());
        let now2 = micros(now + case["later"].as_f64().unwrap());
        let (info, retry) = match redis_outcome(now, window, limit, count, now2) {
            Ok(info) => (info, None),
            Err((info, retry)) => (info, Some(retry.max(1))),
        };
        let h = &case["headers"];
        assert_eq!(info.limit.to_string(), s(&h["X-RateLimit-Limit"]));
        assert_eq!(info.remaining.to_string(), s(&h["X-RateLimit-Remaining"]));
        assert_eq!(info.reset.to_string(), s(&h["X-RateLimit-Reset"]));
        if let Some(retry) = retry {
            assert_eq!(retry.to_string(), s(&case["retry_after"]), "{case}");
        }
        assert_eq!(format!("{now:.6}:7"), s(&case["member"]));
        assert_eq!(py_float_str(now), s(&case["now_str"]));
    }
    for case in arr("ratelimit_mongo") {
        let now_us = micros(case["now"].as_f64().unwrap());
        let (start, reset) = fixed_window(now_us, case["window"].as_i64().unwrap());
        assert_eq!(start.to_string(), s(&case["bucket_suffix"]));
        assert_eq!(reset, case["reset"].as_i64().unwrap());
        assert_eq!(((reset * 1_000_000 - now_us) / 1_000_000).max(1), case["retry_after"].as_i64().unwrap());
    }
    assert_eq!(ratelimit::SLIDING_WINDOW_LUA, s(g("lua_sliding_window")));
    assert_eq!(super::token_use::FLUSH_LUA, s(g("lua_token_flush")));
}

#[test]
fn rate_limit_rejection_shape() {
    let info = ratelimit::RateLimitInfo { limit: 30, remaining: 0, reset: 1761136556 };
    let r = ratelimit::too_many(&info, 60, 0);
    assert_eq!(r.status().as_u16(), 429);
    assert_eq!(r.error.code, "rate_limited");
    assert_eq!(r.error.details, Some(json!({"limit": 30, "window_seconds": 60})));
    let names: Vec<(&str, &str)> = r.headers.iter().map(|(k, v)| (k.as_str(), v.to_str().unwrap())).collect();
    assert_eq!(
        names,
        [
            ("x-ratelimit-limit", "30"),
            ("x-ratelimit-remaining", "0"),
            ("x-ratelimit-reset", "1761136556"),
            ("retry-after", "1")
        ]
    );
}

#[test]
fn usage_rounding_and_routes() {
    for case in arr("round2") {
        assert_eq!(super::pyutil::round2(case["in"].as_f64().unwrap()), case["out"].as_f64().unwrap(), "{case}");
    }
    assert_eq!(fastapi_template("/v1/updates/{branch}/file/{*path}"), "/v1/updates/{branch}/file/{path:path}");
    assert_eq!(fastapi_template("/v1/x/{id}"), "/v1/x/{id}");
    assert_eq!(fastapi_template("/v1/leaderboards/{uuid}/entries"), "/v1/leaderboards/{uuid:int}/entries");
    assert_eq!(starlette_path("/v1/a%20b/%3Fq"), "/v1/a b/%3Fq");
    assert_eq!(starlette_path("/v1/caf%C3%A9"), "/v1/caf\u{e9}");
}

#[test]
fn python_json_ascii_dumps() {
    let v = json!({"new": "\u{fc}ser\u{1f600}\u{7f}", "n": -3, "q": "a\"b\\\n"});
    let want = r#"{"new":"~u00fcser~ud83d~ude00~u007f","n":-3,"q":"a~"b~~~n"}"#.replace('~', "\\");
    assert_eq!(super::pyutil::dumps_ascii(&v), want);
}

/// Rust-made artifacts must be accepted by the Python code. Runs when
/// `KIWI_PY` points at the project venv's python.
#[test]
fn rust_artifacts_verify_in_python() {
    let Ok(py) = std::env::var("KIWI_PY") else {
        eprintln!("KIWI_PY not set; skipping the Python cross-check");
        return;
    };
    let now = Utc::now();
    let ttl = Duration::minutes(15);
    let pw = "p\u{e4}ssw\u{f6}rd \u{1f600}";
    let (api_token, api_hash, api_prefix) = tokens::generate_api_token("kiwi");
    let salt = tokens::make_ip_salt();
    let mut extra = serde_json::Map::new();
    extra.insert("new".into(), "\u{fc}ser@ex\u{e4}mple.com".into());
    let artifacts = json!({
        "secret": secret(),
        "password": pw,
        "argon2": passwords::hash_password(pw),
        "access": jwt::create_access_token_at(secret(), "65f0a1b2c3d4e5f601234567", 4, Some("6718a0b1c2d3e4f5a6b7c8d9"), ttl, now),
        "site": jwt::create_site_access_token_at(secret(), "65f0a1b2c3d4e5f601234567", 2, None, ttl, now),
        "email": jwt::create_email_token_at(secret(), "65f0a1b2c3d4e5f601234567", "email_change", Duration::hours(48), Some(&extra), now),
        "api_token": api_token,
        "api_hash": api_hash,
        "api_prefix": api_prefix,
        "ip_salt": salt,
        "ip_hash": tokens::hash_ip(&salt, "203.0.113.7"),
    });
    let dir = std::env::temp_dir().join(format!("kiwi-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("rust_artifacts.json");
    std::fs::write(&path, artifacts.to_string()).unwrap();
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/src/auth/testdata/verify_rust.py");
    let out = std::process::Command::new(py).arg(script).arg(&path).output().expect("run python");
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        out.status.success(),
        "python rejected Rust artifacts:\n{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}
