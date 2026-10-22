//! Webhooks against Python (`testdata/webhooks/golden.json`, from
//! `native/port/parity/gen_webhooks_fixtures.py`): contexts and rendering,
//! URL rules, delivery health, the Discord retry loop, queue fan-out, every
//! route past auth over an in-memory collection, and the pre-store gates
//! through the real router.

use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, StatusCode},
    response::{IntoResponse, Response},
};
use mongodb::bson::oid::ObjectId;
use serde_json::{Value, json};
use tower::ServiceExt;

use super::{delivery, embeds, service, url};
use crate::{
    dt,
    embed_templates::EmbedTemplate,
    error::ApiError,
    features::{self, Feature},
    outbound::{
        self, Health, Outcome,
        discord::post_webhook,
        testkit::{
            Canned, FakeColl, Scripted, app, arr, check_response, diff_docs, docs, golden, new_id, parts, rid, s,
        },
    },
    pyjson::PydanticJson,
    request_id,
    validation::{BodyInput, read_body, validate},
};

fn asset() -> &'static str {
    s(&golden()["asset"])
}

fn now() -> chrono::DateTime<chrono::Utc> {
    dt::parse_utc(s(&golden()["now"])).expect("now")
}

/// The custom template the generator renders every payload with.
fn custom() -> EmbedTemplate {
    crate::embed_templates::tests::template(&json!({
        "enabled": true, "title": "{name}{project_title}{version_tag}",
        "description": "{type_label}|{tags}|{size_label}|{size_delta}|{downloads}|{files_added}|{sha_short}|{version_change}",
        "content": "{status} {is_friday} {ordinal}",
        "fields": [{"name": "{update_kind}", "value": "{summary}"}],
        "show_image": true,
    }))
}

fn same(a: &Value, b: &Value) -> bool {
    serde_json::to_string(a).unwrap() == serde_json::to_string(b).unwrap()
}

/// `{"ok": v}` or `{"raises": ...}` against a Rust result.
fn check<T: Into<Value>>(what: &str, got: anyhow::Result<T>, want: &Value, bad: &mut Vec<String>) {
    match (got, want.get("ok")) {
        (Ok(v), Some(w)) => {
            let v = v.into();
            if !same(&v, w) {
                bad.push(format!("{what}:\n  got  {v}\n  want {w}"));
            }
        }
        (Err(_), None) => {}
        (Ok(v), None) => bad.push(format!("{what}: got {} want raise {}", v.into(), want["raises"])),
        (Err(e), Some(w)) => bad.push(format!("{what}: raised {e:#} want {w}")),
    }
}

#[test]
fn contexts_and_rendering_match_python() {
    let g = &golden()["contexts"];
    let mut bad = Vec::new();
    let empty = json!({});
    let disabled = EmbedTemplate { enabled: false, title: Some("never".into()), ..Default::default() };
    for item in arr(g, "items") {
        let event = s(&item["event"]);
        if let Some(want) = item.get("sample_default") {
            let got = embeds::render_sample(event, None, asset()).unwrap_or(Value::Null);
            if !same(&got, want) {
                bad.push(format!("sample {event}:\n  got  {got}\n  want {want}"));
            }
            let vars = json!(embeds::variables(event, asset()));
            if !same(&vars, &item["variables"]) {
                bad.push(format!("variables {event}: {vars}"));
            }
            continue;
        }
        let data = &item["data"];
        let what = format!("{event} {data}");
        if let Some(want) = item.get("context") {
            let d = if crate::features::py_truthy(data) { data } else { &empty };
            let got = embeds::context(event, d, asset()).map(|c| Value::Object(c.unwrap_or_default()));
            check(&format!("context {what}"), got, want, &mut bad);
        }
        for (key, tmpl) in
            [("render_none", None), ("render_custom", Some(custom())), ("render_disabled", Some(disabled.clone()))]
        {
            let got = embeds::render(event, data, tmpl.as_ref(), asset()).map(|b| b.unwrap_or(Value::Null));
            check(&format!("{key} {what}"), got, &item[key], &mut bad);
        }
    }
    if !same(&embeds::test_body(), &g["test_body"]) {
        bad.push("test_body".into());
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn urls_match_python() {
    let g = &golden()["urls"];
    let mut bad = Vec::new();
    for case in arr(g, "normalise") {
        let raw = s(&case["raw"]);
        let got = url::normalise_url(raw);
        let ok = match (&got, case.get("ok")) {
            (Ok(u), Some(w)) => u == s(w),
            (Err(Some(e)), None) => {
                e.status.as_u16() as u64 == case["status"].as_u64().unwrap_or(0) && e.message == s(&case["message"])
            }
            (Err(None), None) => case["status"] == 500,
            _ => false,
        };
        if !ok {
            bad.push(format!("normalise {raw:?}: {:?} want {case}", got.map_err(|e| e.map(|e| e.message))));
        }
    }
    for case in arr(g, "mask") {
        let got = url::mask_url(s(&case["url"])).ok();
        if got.as_deref() != case["out"].as_str() {
            bad.push(format!("mask {}: {got:?} want {}", case["url"], case["out"]));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn delivery_health_matches_python() {
    let mut bad = Vec::new();
    for case in arr(golden(), "health") {
        let rules = if case["feature"] == "webhooks" { &delivery::RULES } else { &crate::dm_subs::delivery::RULES };
        let st = &case["start"];
        let mut h = Health {
            active: st["active"].as_bool().unwrap(),
            consecutive_failures: st["consecutive_failures"].as_i64().unwrap(),
            last_status: st["last_status"].as_i64(),
            last_error: st["last_error"].as_str().map(str::to_owned),
            last_delivered_at: None,
            disabled_reason: None,
        };
        let mut updated = chrono::DateTime::<chrono::Utc>::MIN_UTC;
        let o = &case["outcome"];
        let out =
            Outcome { ok: o[0].as_bool().unwrap(), status: o[1].as_i64(), error: o[2].as_str().map(str::to_owned) };
        outbound::record_delivery(&mut h, &mut updated, &out, rules);
        let a = &case["after"];
        let got = json!({
            "active": h.active, "consecutive_failures": h.consecutive_failures, "last_status": h.last_status,
            "last_error": h.last_error, "disabled_reason": h.disabled_reason,
            "delivered": h.last_delivered_at.is_some(),
        });
        let want = json!({
            "active": a["active"], "consecutive_failures": a["consecutive_failures"], "last_status": a["last_status"],
            "last_error": a["last_error"], "disabled_reason": a["disabled_reason"],
            "delivered": !a["last_delivered_at"].is_null(),
        });
        if !same(&got, &want) || updated == chrono::DateTime::<chrono::Utc>::MIN_UTC {
            bad.push(format!("{case}:\n  got {got}"));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// What the scripted httpx transport saw, in the generator's shape.
fn requests(t: &Scripted) -> Vec<Value> {
    t.requests.lock().unwrap().clone()
}

fn want_requests(case: &Value) -> Vec<Value> {
    arr(case, "requests")
        .iter()
        .map(|r| {
            json!({"url": r["url"], "auth": r["auth"], "raw": r["raw"], "multipart": r["multipart"],
                   "payload_json": r["payload_json"]})
        })
        .collect()
}

#[tokio::test]
async fn post_to_discord_matches_python() {
    let mut bad = Vec::new();
    let body = json!({"embeds": [{"title": "Hé ✓", "color": 1}], "content": "x"});
    for case in arr(golden(), "post") {
        let t = Scripted::new(arr(case, "script"));
        let png = case["png"].as_bool().unwrap().then(|| b"\x89PNGdata".to_vec());
        let out = post_webhook(&t, "https://discord.com/api/webhooks/1/2", &body, png.as_deref()).await;
        let got = json!([out.ok, out.status, out.error]);
        if !same(&got, &case["outcome"]) {
            bad.push(format!("{}: outcome {got}", case["script"]));
        }
        if !same(&json!(*t.sleeps.lock().unwrap()), &case["sleeps"]) {
            bad.push(format!("{}: sleeps {:?} want {}", case["script"], t.sleeps.lock().unwrap(), case["sleeps"]));
        }
        if !same(&json!(requests(&t)), &json!(want_requests(case))) {
            bad.push(format!("{}: requests {:?}\n want {:?}", case["script"], requests(&t), want_requests(case)));
        }
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

struct FakePoster(Canned);

impl delivery::Poster for FakePoster {
    async fn post(&self, url: &str, body: &Value, design: Option<&str>) -> Outcome {
        self.0.next(json!({"url": url, "body": body, "design": design}))
    }
}

fn poster(outcomes: &[Value]) -> FakePoster {
    FakePoster(Canned::new(outcomes, Outcome { ok: true, status: Some(204), error: None }))
}

#[tokio::test]
async fn queue_fanout_matches_python() {
    let mut bad = Vec::new();
    let mut n = 0;
    for case in arr(golden(), "queue").iter().filter(|c| c["feature"] == "webhooks") {
        n += 1;
        let coll = FakeColl::new(docs(case, "before", super::model::COLLECTION), None);
        let p = poster(arr(case, "outcomes"));
        let res = delivery::deliver(&coll, &p, &case["job"], asset()).await;
        let what = format!("job {}", case["job"]);
        if res.is_err() != !case["raises"].is_null() {
            bad.push(format!("{what}: {res:?} want raises {}", case["raises"]));
        }
        let sends = json!(*p.0.sends.lock().unwrap());
        if !same(&sends, &case["sends"]) {
            bad.push(format!("{what}: sends\n  got  {sends}\n  want {}", case["sends"]));
        }
        let want = docs(case, "after", super::model::COLLECTION);
        bad.extend(diff_docs(&what, &coll.snapshot(), &want, true));
    }
    assert!(n >= 8, "queue cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

fn user(case: &Value) -> ObjectId {
    ObjectId::parse_str(s(&case["user"]["id"])).expect("user id")
}

async fn body_of<T: crate::validation::FromBody>(case: &Value) -> Result<T, ApiError> {
    let mut headers = HeaderMap::new();
    for p in arr(case, "headers") {
        headers.append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
    }
    let bytes = match (case.get("body"), case.get("raw")) {
        (_, Some(raw)) => s(raw).as_bytes().to_vec(),
        (Some(b), _) => crate::pyjson::dumps_spaced(b).into_bytes(),
        _ => Vec::new(),
    };
    let input: BodyInput = read_body(&headers, Body::from(bytes)).await?;
    validate::<T>(&input)
}

fn reply(r: Result<Value, ApiError>, ok: StatusCode) -> Response {
    match r {
        Ok(v) => (ok, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

async fn dispatch(case: &Value, coll: &FakeColl, p: &FakePoster) -> Response {
    let url = s(&case["url"]);
    let (path, query) = url.split_once('?').unwrap_or((url, ""));
    let method = s(&case["method"]);
    if path == "/v1/webhooks/events" && method == "GET" {
        return PydanticJson(json!({"events": super::model::EVENT_TYPES, "meta": embeds::event_meta(asset())}))
            .into_response();
    }
    let actor = user(case);
    let id = path.strip_prefix("/v1/webhooks/").map(|r| r.strip_suffix("/test").unwrap_or(r));
    match (method, id) {
        ("GET", None) => reply(service::list(coll, actor).await, StatusCode::OK),
        ("POST", None) => match body_of::<service::WebhookCreate>(case).await {
            Ok(req) => reply(service::create(coll, actor, &req, now()).await, StatusCode::CREATED),
            Err(e) => e.into_response(),
        },
        ("PATCH", Some(id)) => match body_of::<service::WebhookUpdate>(case).await {
            Ok(req) => reply(service::update(coll, actor, id, &req, now()).await, StatusCode::OK),
            Err(e) => e.into_response(),
        },
        ("DELETE", Some(id)) => match service::delete(coll, actor, id).await {
            Ok(()) => StatusCode::NO_CONTENT.into_response(),
            Err(e) => e.into_response(),
        },
        ("POST", Some(id)) => {
            let uri: axum::http::Uri = format!("http://x{path}?{query}").parse().unwrap();
            let event = crate::middleware::pyurl::query_param(&uri, "event");
            reply(service::send_test(coll, p, actor, id, event.as_deref(), asset()).await, StatusCode::OK)
        }
        other => panic!("unrouted {other:?}"),
    }
}

#[tokio::test]
async fn routes_match_python() {
    let mut bad = Vec::new();
    let cases: Vec<&Value> = arr(golden(), "http").iter().filter(|c| s(&c["name"]).starts_with("wh_")).collect();
    for case in &cases {
        let name = s(&case["name"]);
        let c = super::model::COLLECTION;
        let coll = FakeColl::new(docs(case, "before", c), new_id(case, c));
        let p = poster(arr(case, "outcomes"));
        let resp = request_id::scope(rid(case), dispatch(case, &coll, &p)).await;
        let (status, headers, body) = parts(resp).await;
        bad.extend(check_response(case, status, &headers, &body, &[]));
        bad.extend(diff_docs(name, &coll.snapshot(), &docs(case, "after", c), false));
        let sends = json!(*p.0.sends.lock().unwrap());
        if !same(&sends, &case["sends"]) {
            bad.push(format!("{name}: sends\n  got  {sends}\n  want {}", case["sends"]));
        }
    }
    assert!(cases.len() > 30, "route cases missing");
    assert!(bad.is_empty(), "{} problems:\n{}", bad.len(), bad.join("\n"));
}

/// The cases Python answers before any store: 401s, 405s, malformed JSON,
/// through the real router and middleware.
#[tokio::test]
async fn gates_match_python() {
    let app = app(super::router().merge(crate::dm_subs::router()));
    let mut bad = Vec::new();
    let mut n = 0;
    let public = arr(golden(), "http").iter().filter(|c| matches!(s(&c["name"]), "wh_events" | "dm_events"));
    for case in arr(golden(), "gated").iter().chain(public) {
        let name = s(&case["name"]);
        if name.ends_with("_off") || name.ends_with("_off_noauth") {
            let feature =
                if name.starts_with("dm_") { crate::features::DmSubs::NAME } else { features::Webhooks::NAME };
            let resp = request_id::scope(rid(case), async { features::disabled(feature).into_response() }).await;
            let (status, headers, body) = parts(resp).await;
            bad.extend(check_response(case, status, &headers, &body, &[]));
            continue;
        }
        n += 1;
        let body = match (case.get("raw"), case.get("body")) {
            (Some(r), _) => Body::from(s(r).to_owned()),
            (_, Some(b)) => Body::from(crate::pyjson::dumps_spaced(b)),
            _ => Body::empty(),
        };
        let mut req = Request::builder()
            .method(Method::from_bytes(s(&case["method"]).as_bytes()).unwrap())
            .uri(s(&case["url"]))
            .body(body)
            .unwrap();
        for p in arr(case, "headers") {
            req.headers_mut()
                .append(HeaderName::from_bytes(s(&p[0]).as_bytes()).unwrap(), HeaderValue::from_str(s(&p[1])).unwrap());
        }
        let (status, headers, body) = parts(app.clone().oneshot(req).await.expect("infallible")).await;
        bad.extend(check_response(case, status, &headers, &body, &["allow", "www-authenticate", "cache-control"]));
    }
    assert!(n >= 20, "gated cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn template_attachment_needs_enabled_and_show_image() {
    let mut t = EmbedTemplate { enabled: true, image_design_id: Some("d".into()), ..Default::default() };
    assert_eq!(EmbedTemplate::attached_design(Some(&t)).as_deref(), Some("d"));
    t.show_image = false;
    assert_eq!(EmbedTemplate::attached_design(Some(&t)), None);
}

/// Against a real Mongo + Redis (`MONGO_URI`, `REDIS_URL`): a queued job is
/// popped once and the webhook's health is saved.
#[tokio::test]
#[ignore = "needs MONGO_URI and REDIS_URL"]
async fn store_queue_round_trip() {
    let db = mongodb::Client::with_uri_str(std::env::var("MONGO_URI").unwrap()).await.unwrap().database("kiwi_test_wh");
    let coll = outbound::store::MongoColl::new(&db, "site_webhooks");
    let w = super::model::Webhook {
        health: Health::default(),
        id: None,
        owner_id: ObjectId::new(),
        label: "t".into(),
        url: "https://discord.com/api/webhooks/1/2".into(),
        events: vec!["challenge".into()],
        templates: Vec::new(),
        created_at: dt::now(),
        updated_at: dt::now(),
    };
    use outbound::store::Coll;
    let id = coll.insert(w.to_doc()).await.unwrap();
    let p = poster(&[json!([false, 500, "HTTP 500"])]);
    let job = json!({"type": "challenge", "data": {"name": "Racing Challenge"}});
    delivery::deliver(&coll, &p, &job, "https://trove.aallyn.net").await.unwrap();
    let saved = coll.get(id).await.unwrap().unwrap();
    assert_eq!(saved.get_i32("consecutive_failures").unwrap(), 1);
    db.drop().await.unwrap();
}

/// The real reqwest transport against a local server: JSON and multipart
/// bodies as httpx sends them, `Retry-After` honoured, no redirects followed.
#[tokio::test]
async fn http_transport_sends_what_httpx_sends() {
    use std::sync::{Arc, Mutex};

    use axum::{extract::State, routing::post};

    use crate::outbound::discord::Http;

    type Seen = Arc<Mutex<Vec<(String, Vec<u8>)>>>;
    async fn hook(State(seen): State<Seen>, req: Request<Body>) -> Response {
        let path = req.uri().path().to_owned();
        let ctype = req.headers().get("content-type").map(|v| v.to_str().unwrap().to_owned()).unwrap_or_default();
        let body = axum::body::to_bytes(req.into_body(), usize::MAX).await.unwrap().to_vec();
        let n = {
            let mut s = seen.lock().unwrap();
            s.push((format!("{path} {ctype}"), body));
            s.len()
        };
        match (path.as_str(), n) {
            ("/limited", 1) => (StatusCode::TOO_MANY_REQUESTS, [("retry-after", "0.01")], "slow down").into_response(),
            ("/moved", _) => (StatusCode::FOUND, [("location", "/elsewhere")], "").into_response(),
            _ => StatusCode::NO_CONTENT.into_response(),
        }
    }
    let seen: Seen = Arc::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let router = axum::Router::new().route("/{p}", post(hook)).with_state(seen.clone());
    tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });

    let body = json!({"embeds": [{"title": "Hé ✓"}], "content": "x"});
    let out = post_webhook(Http::webhooks(), &format!("http://{addr}/limited"), &body, None).await;
    assert_eq!(out, Outcome { ok: true, status: Some(204), error: None });
    let png = b"\x89PNG-bytes".to_vec();
    let out = post_webhook(Http::webhooks(), &format!("http://{addr}/files"), &body, Some(&png)).await;
    assert!(out.ok);
    let moved = post_webhook(Http::webhooks(), &format!("http://{addr}/moved"), &body, None).await;
    assert_eq!(moved.status, Some(302));

    let seen = seen.lock().unwrap().clone();
    assert_eq!(seen[0].0, "/limited application/json");
    assert_eq!(seen[0].1, r#"{"embeds":[{"title":"Hé ✓"}],"content":"x"}"#.as_bytes());
    assert_eq!(seen.len(), 6, "two posts to /limited, one to /files, three to /moved");
    let multipart = String::from_utf8_lossy(&seen[2].1);
    assert!(seen[2].0.starts_with("/files multipart/form-data; boundary="));
    assert!(multipart.contains(
        "name=\"payload_json\"\r\n\r\n{\"embeds\": [{\"title\": \"H\\u00e9 \\u2713\"}], \"content\": \"x\"}\r\n"
    ));
    assert!(multipart.contains("name=\"files[0]\"; filename=\"embed-image.png\"\r\nContent-Type: image/png\r\n\r\n"));
    assert!(seen.iter().all(|(p, _)| !p.starts_with("/elsewhere")));

    let refused = post_webhook(Http::webhooks(), "http://127.0.0.1:9/x", &body, None).await;
    assert_eq!(refused.status, None);
    assert!(refused.error.as_deref().is_some_and(|e| !e.is_empty() && !e.contains("127.0.0.1:9/x")), "{refused:?}");
    eprintln!("connection refused reads: {:?}", refused.error);
}
