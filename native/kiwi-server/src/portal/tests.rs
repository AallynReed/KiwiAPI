//! Byte-for-byte checks against `testdata/portal/golden.json`, captured from
//! the real Python app and services by `native/port/parity/gen_portal_fixtures.py`.

use std::sync::{Arc, OnceLock};

use axum::{
    Router,
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode},
    response::Response,
};
use hyper_util::{client::legacy::Client, rt::TokioExecutor};
use mongodb::bson::{Bson, DateTime as BsonDateTime, Document, oid::ObjectId};
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{
    account::email_field,
    activity::{pipelines, summary},
    emails,
    gdpr::export_doc,
    scanning::{b64decode, verify_with_pem},
    store::{TokenTimes, session_fields, token_fields, token_public, user_fields, user_public},
    tokens::hash_pinned_ips,
};
use crate::{
    auth::models::{ApiToken, SessionRow, User},
    config::Settings,
    db::Db,
    dt,
    middleware::{self, EmbedSource, Mw, MwConfig, forwarded::TrustedHosts},
    pyjson,
    state::{AppState, Inner},
    validation::{Field, FromBody, Input, Validator},
};

fn golden() -> &'static Value {
    static G: OnceLock<Value> = OnceLock::new();
    G.get_or_init(|| serde_json::from_str(include_str!("../../testdata/portal/golden.json")).expect("fixture"))
}

fn s(v: &Value) -> &str {
    v.as_str().unwrap_or_else(|| panic!("string: {v}"))
}

fn arr<'a>(v: &'a Value, key: &str) -> &'a Vec<Value> {
    v[key].as_array().unwrap_or_else(|| panic!("fixture section {key}"))
}

fn latin1(s: &str) -> Vec<u8> {
    s.chars().map(|c| c as u32 as u8).collect()
}

fn settings(extra: &[(&str, &str)]) -> Settings {
    let env = golden()["env"].as_object().expect("env");
    let mut vars: Vec<(String, String)> = env.iter().map(|(k, v)| (k.clone(), s(v).to_owned())).collect();
    vars.extend(extra.iter().map(|(k, v)| ((*k).to_owned(), (*v).to_owned())));
    vars.push(("PY_UPSTREAM".into(), "http://py:8000".into()));
    Settings::from_vars(vars).expect("settings")
}

fn app(settings: Settings) -> Router {
    let upstream = Client::builder(TokioExecutor::new()).build_http();
    let state =
        AppState { inner: Arc::new(Inner { settings, db: Db { mongo: None, redis: None, pg: None }, upstream }) };
    let mw =
        Mw::new(MwConfig::from_lookup(&|_| None), None, EmbedSource::Fixed(vec![]), TrustedHosts::parse("127.0.0.1"));
    let inner = middleware::wrap(super::router(), Arc::new(mw), None)
        .fallback(|| async { StatusCode::IM_A_TEAPOT })
        .with_state(state);
    middleware::outer(inner)
}

fn headers_of(case: &Value) -> HeaderMap {
    let mut h = HeaderMap::new();
    for pair in arr(case, "headers") {
        let name = HeaderName::from_bytes(s(&pair[0]).as_bytes()).expect("name");
        h.append(name, HeaderValue::from_bytes(&latin1(s(&pair[1]))).expect("value"));
    }
    h
}

async fn parts(resp: Response) -> (u16, HeaderMap, String) {
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let body = axum::body::to_bytes(resp.into_body(), usize::MAX).await.expect("body");
    (status, headers, String::from_utf8(body.to_vec()).expect("utf-8"))
}

const COMPARED: [&str; 8] = [
    "content-type",
    "allow",
    "www-authenticate",
    "cache-control",
    "location",
    "content-disposition",
    "retry-after",
    "content-security-policy",
];

async fn replay(section: &str, settings: Settings) {
    let app = app(settings);
    let cases = arr(golden(), section);
    let mut bad = Vec::new();
    for case in cases {
        let name = s(&case["name"]);
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&case["method"]).as_bytes()).expect("method"))
            .uri(s(&case["url"]))
            .body(Body::from(latin1(s(&case["body"]))))
            .expect("request");
        *req.headers_mut() = headers_of(case);
        let (status, headers, body) = parts(app.clone().oneshot(req).await.expect("infallible")).await;
        if u64::from(status) != case["status"].as_u64().expect("status") {
            bad.push(format!("{name}: status {status} want {}", case["status"]));
        }
        for h in COMPARED {
            let ours = headers.get(h).map(|v| String::from_utf8_lossy(v.as_bytes()).into_owned());
            let want = case["resp_headers"].get(h).map(|v| s(v).to_owned());
            if ours != want {
                bad.push(format!("{name}: {h} {ours:?} want {want:?}"));
            }
        }
        if body != s(&case["resp"]) {
            bad.push(format!("{name}:\n  got  {body}\n  want {}", s(&case["resp"])));
        }
    }
    assert!(bad.is_empty(), "{section}: {} of {} differ:\n{}", bad.len(), cases.len(), bad.join("\n"));
}

#[tokio::test]
async fn http_replay() {
    replay("http", settings(&[])).await;
}

#[tokio::test]
async fn scanning_replay() {
    replay("scan", settings(&[("GITHUB_SECRET_SCANNING_VERIFY", "false")])).await;
}

#[tokio::test]
async fn oauth_replay() {
    let env = golden()["oauth_env"].as_object().expect("oauth env");
    let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), s(v))).collect();
    replay("oauth", settings(&extra)).await;
}

#[test]
fn authorize_url_matches_urlencode() {
    let env = golden()["oauth_env"].as_object().expect("oauth env");
    let extra: Vec<(&str, &str)> = env.iter().map(|(k, v)| (k.as_str(), s(v))).collect();
    let st = settings(&extra);
    let url = super::oauth::authorize_url(&st, "St4te_-x");
    assert_eq!(url, s(&golden()["authorize_url"]));
    let resp = super::oauth::redirect(&url);
    assert_eq!(resp.headers()["location"], s(&golden()["authorize_location"]));
}

struct EmailBody(String);

impl FromBody for EmailBody {
    fn from_body(body: Field<'_>) -> Option<Self> {
        body.model(|m| Some(EmailBody(email_field(m.field("email"))))).required()
    }
}

#[test]
fn email_str() {
    let mut bad = Vec::new();
    for case in arr(golden(), "emails") {
        let body = json!({ "email": case["input"] });
        let mut v = Validator::default();
        let got = EmailBody::from_body(Field::new(&mut v, vec!["body".into()], Input::Json(&body)));
        let label = case["input"].to_string();
        match (case.get("ok"), v.errors.first()) {
            (Some(want), None) => match got.map(|b| b.0) {
                Some(e) if e == s(want) => {}
                e => bad.push(format!("{label}: got {e:?} want {want}")),
            },
            (Some(want), Some(e)) => bad.push(format!("{label}: error {} want {want}", e.msg)),
            (None, None) => bad.push(format!("{label}: got {:?} want {}", got.map(|b| b.0), case["err"])),
            (None, Some(e)) => {
                let item = e.to_value();
                let want = &case["err"];
                let ctx = want.get("ctx").filter(|c| !c.is_null());
                if item["type"] != want["type"] || item["msg"] != want["msg"] || item.get("ctx") != ctx {
                    bad.push(format!("{label}: got {item} want {want}"));
                }
            }
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

fn fake_user() -> User {
    User {
        id: ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap(),
        email: "user@example.com".into(),
        hashed_password: "$argon2id$v=19$m=65536,t=3,p=4$c2FsdA$aGFzaA".into(),
        display_name: Some("Ann".into()),
        is_active: true,
        is_verified: false,
        is_superuser: false,
        roles: vec![],
        token_version: 0,
        github_id: None,
        email_bounced: false,
        created_at: BsonDateTime::from_millis(0),
        updated_at: BsonDateTime::from_millis(0),
        last_login_at: None,
    }
}

fn untoken(s: &str) -> String {
    let re = regex::Regex::new(r"token=[A-Za-z0-9_\-.]+").unwrap();
    re.replace_all(s, "token=TOKEN").into_owned()
}

#[test]
fn mail_bodies() {
    let st = settings(&[]);
    let u = fake_user();
    let (name, prefix) = ("My \"token\" <&>", "kiwi_AbCd1234");
    let mut bad = Vec::new();
    for case in arr(golden(), "mail") {
        let m = match s(&case["name"]) {
            "verification" => emails::verification_email(&st, &u),
            "reset" => emails::password_reset_email(&st, &u),
            "email_change" => emails::email_change_verification(&st, &u, "new@example.com"),
            "new_login" => emails::new_login_email(&st, &u, Some("203.0.113.7"), Some("Chrome on Windows")),
            "new_login_unknown" => emails::new_login_email(&st, &u, Some(""), None),
            "password_changed" => emails::password_changed_email(&st, &u),
            "token_created" => emails::token_created_email(&st, &u, name, prefix),
            "token_expiring" => emails::token_expiring_email(&st, &u, name, prefix, 3),
            "token_expiring_today" => emails::token_expiring_email(&st, &u, name, prefix, 0),
            "github_linked" => emails::github_linked_email(&st, &u),
            "token_compromised" => emails::token_compromised_email(&st, &u, name, prefix),
            "email_changed_old" => emails::email_changed_notice_to_old(&st, "old@example.com", "new@example.com"),
            other => panic!("mail case {other}"),
        };
        let label = s(&case["name"]);
        for (field, got) in [
            ("to", m.to.clone()),
            ("subject", m.subject.clone()),
            ("text", untoken(&m.text)),
            ("html", untoken(&m.html)),
        ] {
            if got != s(&case[field]) {
                bad.push(format!("{label}.{field}:\n  got  {got:?}\n  want {:?}", case[field]));
            }
        }
    }
    assert!(bad.is_empty(), "{} differ:\n{}", bad.len(), bad.join("\n"));
}

#[test]
fn mail_link_claims() {
    let st = settings(&[]);
    let u = fake_user();
    let secret = &crate::auth::auth_settings().secret_key;
    let token_of = |m: &emails::Message| {
        let re = regex::Regex::new(r"token=([A-Za-z0-9_\-.]+)").unwrap();
        re.captures(&m.text).unwrap()[1].to_owned()
    };
    let check = |m: emails::Message, purpose: &str, hours: i64, keys: &[&str]| {
        let claims = crate::auth::jwt::decode_email_token(&token_of(&m), secret, purpose).expect("decodes");
        let order: Vec<&str> = claims.keys().map(String::as_str).collect();
        assert_eq!(order, keys, "{purpose} claim order");
        assert_eq!(claims["sub"], "65a1b2c3d4e5f60718293a4b");
        let span = claims["exp"].as_i64().unwrap() - claims["iat"].as_i64().unwrap();
        assert_eq!(span, hours * 3600);
        claims
    };
    check(emails::verification_email(&st, &u), "email_verify", 48, &["sub", "type", "iat", "exp"]);
    let c = check(emails::password_reset_email(&st, &u), "password_reset", 1, &["sub", "type", "iat", "exp", "fp"]);
    assert_eq!(c["fp"], crate::auth::tokens::password_fingerprint(&u.hashed_password));
    let c = check(
        emails::email_change_verification(&st, &u, "n@x.io"),
        "email_change",
        48,
        &["sub", "type", "iat", "exp", "new", "fp"],
    );
    assert_eq!(
        (c["new"].as_str(), c["fp"].as_str()),
        (Some("n@x.io"), Some(&*crate::auth::tokens::password_fingerprint("user@example.com")))
    );
}

fn date(v: &Value) -> Option<BsonDateTime> {
    v.as_str().map(|t| dt::to_bson(dt::parse_utc(t).expect("date")))
}

fn token_of(d: &Value) -> ApiToken {
    ApiToken {
        id: ObjectId::parse_str(s(&d["id"])).unwrap(),
        user_id: ObjectId::parse_str(s(&d["user_id"])).unwrap(),
        name: s(&d["name"]).into(),
        prefix: s(&d["prefix"]).into(),
        hashed_token: s(&d["hashed_token"]).into(),
        scopes: d["scopes"].as_i64().unwrap(),
        ip_salt: d["ip_salt"].as_str().map(str::to_owned),
        allowed_ip_hashes: d["allowed_ip_hashes"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect(),
        revoked: d["revoked"].as_bool().unwrap(),
        revoked_at: date(&d["revoked_at"]),
        revoke_reason: d["revoke_reason"].as_str().map(str::to_owned),
        expires_at: date(&d["expires_at"]),
        expiry_warned: d["expiry_warned"].as_bool().unwrap(),
        created_at: date(&d["created_at"]).unwrap(),
        last_used_at: date(&d["last_used_at"]),
        rotated_at: date(&d["rotated_at"]),
        request_count: d["request_count"].as_i64().unwrap(),
    }
}

#[test]
fn view_models() {
    for case in arr(&golden()["views"], "tokens") {
        let t = token_of(&case["doc"]);
        let public = token_public(&t, TokenTimes::stored(&t));
        assert_eq!(pyjson::pydantic(&public), s(&case["public"]));
        let mut created = public.clone();
        created.as_object_mut().unwrap().insert("token".into(), json!("kiwi_secret"));
        assert_eq!(pyjson::pydantic(&created), s(&case["created"]));
    }
    for case in arr(&golden()["views"], "users") {
        let d = &case["doc"];
        let created = dt::parse_utc(s(&d["created_at"])).unwrap();
        let u = User {
            email: "ann@example.com".into(),
            display_name: d["display_name"].as_str().map(str::to_owned),
            is_active: d.get("is_active").and_then(Value::as_bool).unwrap_or(true),
            is_verified: d.get("is_verified").and_then(Value::as_bool).unwrap_or(false),
            is_superuser: d.get("is_superuser").and_then(Value::as_bool).unwrap_or(false),
            roles: d["roles"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect(),
            last_login_at: date(&d["last_login_at"]),
            created_at: dt::to_bson(created),
            ..fake_user()
        };
        assert_eq!(pyjson::pydantic(&user_public(&u, created)), s(&case["public"]));
    }
}

fn bson_of(v: &Value) -> Bson {
    match v {
        Value::Null => Bson::Null,
        Value::Bool(b) => Bson::Boolean(*b),
        Value::Number(n) => match n.as_i64() {
            Some(i) => i32::try_from(i).map_or(Bson::Int64(i), Bson::Int32),
            None => Bson::Double(n.as_f64().unwrap()),
        },
        Value::String(s) => Bson::String(s.clone()),
        Value::Array(a) => Bson::Array(a.iter().map(bson_of).collect()),
        Value::Object(m) => Bson::Document(m.iter().map(|(k, v)| (k.clone(), bson_of(v))).collect()),
    }
}

fn docs(v: &Value) -> Vec<Document> {
    v.as_array().unwrap().iter().map(|d| bson_of(d).as_document().unwrap().clone()).collect()
}

#[test]
fn export_body() {
    let g = &golden()["export"];
    let user = User {
        email: s(&g["user"]["email"]).into(),
        display_name: g["user"]["display_name"].as_str().map(str::to_owned),
        roles: vec!["r".into()],
        github_id: g["user"]["github_id"].as_i64(),
        created_at: date(&g["user"]["created_at"]).unwrap(),
        ..fake_user()
    };
    let tokens: Vec<ApiToken> = arr(g, "tokens").iter().map(token_of).collect();
    let sessions: Vec<SessionRow> = arr(g, "sessions")
        .iter()
        .map(|d| SessionRow {
            id: ObjectId::parse_str(s(&d["id"])).unwrap(),
            owner_id: user.id,
            refresh_token_hash: String::new(),
            device: d["device"].as_str().map(str::to_owned),
            created_at: date(&d["created_at"]).unwrap(),
            last_used_at: date(&d["last_used_at"]).unwrap(),
            expires_at: date(&d["expires_at"]).unwrap(),
            revoked: d["revoked"].as_bool().unwrap(),
        })
        .collect();
    let usage: Vec<Document> = arr(g, "usage")
        .iter()
        .map(|e| {
            let mut d = bson_of(e).as_document().unwrap().clone();
            d.insert("created_at", date(&e["created_at"]).unwrap());
            d
        })
        .collect();
    let exported = dt::parse_utc(s(&g["exported_at"])).unwrap();
    let body = pyjson::dumps(&export_doc(exported, &user, &tokens, &sessions, &usage));
    assert_eq!(body, s(&g["body"]));
}

fn bson_json(b: &Bson) -> Value {
    match b {
        Bson::Null => Value::Null,
        Bson::Int32(i) => json!(i),
        Bson::Int64(i) => json!(i),
        Bson::String(s) => json!(s),
        Bson::ObjectId(o) => json!(o.to_hex()),
        Bson::DateTime(_) => json!("<since>"),
        Bson::Array(a) => Value::Array(a.iter().map(bson_json).collect()),
        Bson::Document(d) => Value::Object(d.iter().map(|(k, v)| (k.clone(), bson_json(v))).collect()),
        other => panic!("unexpected {other:?}"),
    }
}

#[test]
fn activity_summaries() {
    for case in arr(golden(), "activity") {
        let days = case["days"].as_i64().unwrap();
        let out = summary(days, &docs(&case["totals"]), &docs(&case["by_day"]), &docs(&case["by_endpoint"])).unwrap();
        assert_eq!(pyjson::pydantic(&out), s(&case["out"]), "days={days}");
        let base = mongodb::bson::doc! { "user_id": ObjectId::parse_str("65a1b2c3d4e5f60718293a4b").unwrap() };
        let ours: Vec<Value> = pipelines(&base, BsonDateTime::now())
            .iter()
            .map(|p| Value::Array(p.iter().map(|d| bson_json(&Bson::Document(d.clone()))).collect()))
            .collect();
        let mut theirs = case["pipelines"].clone();
        for p in theirs.as_array_mut().unwrap() {
            p[0]["$match"]["created_at"]["$gte"] = json!("<since>");
        }
        assert_eq!(Value::Array(ours), theirs);
    }
}

#[test]
fn pinned_ips() {
    for case in arr(golden(), "ips") {
        let ips: Vec<String> = case["ips"].as_array().unwrap().iter().map(|v| s(v).to_owned()).collect();
        match hash_pinned_ips(&ips, "S4lt_-") {
            Ok(h) => assert_eq!(json!(h), case["hashes"], "{ips:?}"),
            Err(e) => assert_eq!(e.error.message, s(&case["error"]), "{ips:?}"),
        }
    }
}

#[test]
fn scanning_signatures() {
    let g = &golden()["signatures"];
    for key in arr(g, "keys") {
        for case in arr(key, "cases") {
            let ok = b64decode(s(&case["sig"]))
                .is_some_and(|sig| verify_with_pem(s(&case["pem"]), s(&case["body"]).as_bytes(), &sig));
            assert_eq!(json!(ok), case["ok"], "{} {}", key["name"], case["label"]);
        }
    }
    for case in arr(g, "b64") {
        let got = b64decode(s(&case["in"])).map(hex::encode);
        assert_eq!(json!(got), case["out"], "{:?}", case["in"]);
    }
}

#[test]
fn document_field_order() {
    let m = &golden()["models"];
    let keys = |d: Document| -> Value { json!(d.keys().collect::<Vec<_>>()) };
    let u = fake_user();
    assert_eq!(keys(user_fields(&u)), m["user"]);
    let t = token_of(&arr(&golden()["views"], "tokens")[0]["doc"]);
    assert_eq!(keys(token_fields(&t)), m["api_token"]);
    let sess = SessionRow {
        id: ObjectId::new(),
        owner_id: u.id,
        refresh_token_hash: String::new(),
        device: None,
        created_at: BsonDateTime::now(),
        last_used_at: BsonDateTime::now(),
        expires_at: BsonDateTime::now(),
        revoked: false,
    };
    assert_eq!(keys(session_fields(&sess)), m["session"]);
    let msg = emails::Message { to: "a".into(), subject: "b".into(), text: "c".into(), html: "d".into() };
    let mut outbox: Vec<String> = emails::outbox_doc(&msg, dt::now(), dt::now()).keys().cloned().collect();
    assert_eq!(outbox.remove(0), "_id");
    assert_eq!(json!(outbox), m["outbox"]);
}

/// A stand-in for api-py that answers `py` and keeps each raw request.
async fn upstream() -> (std::net::SocketAddr, Arc<std::sync::Mutex<Vec<String>>>) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let seen = Arc::new(std::sync::Mutex::new(Vec::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let mut raw = Vec::new();
            let mut buf = vec![0u8; 8192];
            loop {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                raw.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&raw).into_owned();
                let done = text.split_once("\r\n\r\n").is_some_and(|(head, body)| {
                    let len = head
                        .lines()
                        .find_map(|l| {
                            l.to_ascii_lowercase().strip_prefix("content-length:").map(|v| v.trim().to_owned())
                        })
                        .and_then(|v| v.parse::<usize>().ok())
                        .unwrap_or(0);
                    body.len() >= len
                });
                if n == 0 || done {
                    break;
                }
            }
            log.lock().unwrap().push(String::from_utf8_lossy(&raw).into_owned());
            let _ = sock.write_all(b"HTTP/1.1 201 Created\r\ncontent-length: 2\r\nconnection: close\r\n\r\npy").await;
        }
    });
    (addr, seen)
}

#[tokio::test]
async fn unicode_emails_stay_in_rust() {
    let (addr, seen) = upstream().await;
    let settings = Settings::from_vars([("PY_UPSTREAM", format!("http://{addr}"))]).unwrap();
    let client = Client::builder(TokioExecutor::new()).build_http();
    let db = Db { mongo: None, redis: None, pg: None };
    let app = crate::routes::app(AppState { inner: Arc::new(Inner { settings, db, upstream: client }) });
    let peer: std::net::SocketAddr = "203.0.113.9:5000".parse().unwrap();
    let send = |path: &'static str, body: &'static str| {
        let app = app.clone();
        async move {
            let req = Request::builder()
                .method("POST")
                .uri(path)
                .header("content-type", "application/json")
                .extension(axum::extract::ConnectInfo(peer))
                .body(Body::from(body))
                .unwrap();
            let resp = app.oneshot(req).await.unwrap();
            let status = resp.status();
            (status, axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap())
        }
    };
    let (status, _) = send("/auth/signup", r#"{"email": "Ünï@example.com", "password": "x"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    let (status, _) = send("/auth/delete-account", r#"{"confirm_email": "a@xn--bcher-kva.de"}"#).await;
    assert_ne!(status, StatusCode::CREATED);
    let (status, _) = send("/auth/signup", r#"{"email": "a@[1.2.3.4]", "password": "x"}"#).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY);
    assert!(seen.lock().unwrap().is_empty(), "a Unicode email reached Python");
}
