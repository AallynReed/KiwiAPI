//! Golden tests against `native/port/parity/gen_misc.py` output.

use std::path::{Path, PathBuf};

use kiwi_core::{form, pydt};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

use super::{feedback, modding_software, time};
use crate::pyjson;

fn testdata(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("testdata/misc").join(name)
}

fn load(name: &str) -> Value {
    serde_json::from_str(&std::fs::read_to_string(testdata(name)).unwrap()).unwrap()
}

fn source() -> time::ZoneSource {
    time::ZoneSource::new(vec![testdata("zoneinfo")], None)
}

#[test]
fn fromisoformat_matches_python() {
    let rows = load("isoformat.json");
    let mut checked = 0;
    for row in rows.as_array().unwrap() {
        let s = row[0].as_str().unwrap();
        let got = match pydt::fromisoformat(s) {
            Ok(d) => json!([d.year, d.month, d.day, d.hour, d.minute, d.second, d.microsecond, d.offset_us]),
            Err(e) => Value::String(e),
        };
        assert_eq!(got, row[1], "fromisoformat({s:?})");
        checked += 1;
    }
    assert!(checked > 3000);
}

#[test]
fn zones_match_python() {
    let src = source();
    let fixture = load("zones.json");
    for (key, rows) in fixture.as_object().unwrap() {
        let tz = time::Tz::Zone(src.get(key).unwrap_or_else(|| panic!("{key} loads")));
        for r in rows["utc"].as_array().unwrap() {
            let unix = r[0].as_i64().unwrap();
            let (_, off) = time::fromtimestamp(unix, &tz).unwrap();
            assert_eq!(off, r[1].as_i64().unwrap(), "{key} fromutc({unix})");
        }
        for r in rows["local"].as_array().unwrap() {
            let wall = r[0].as_i64().unwrap();
            let year = pydt::Wall::from_secs(wall).year;
            let got = time::naive_timestamp(i128::from(wall) * 1_000_000, &tz, year);
            assert_eq!(got, r[1].as_i64().unwrap(), "{key} local {wall}");
        }
    }
    for r in load("zone_render.json").as_array().unwrap() {
        let tz = time::tzinfo(&src, r[0].as_str().unwrap()).unwrap();
        let (local, off) = time::fromtimestamp(r[1].as_i64().unwrap(), &tz).unwrap();
        assert_eq!(
            json!([local.isoformat(off), local.strftime_date(), local.strftime_time()]),
            json!(r.as_array().unwrap()[2..5])
        );
    }
}

#[test]
fn misc_functions_match_python() {
    let src = source();
    let fx = load("misc.json");
    let software = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../app/trove/gamedata/modding_software.json");
    assert_eq!(pyjson::pydantic(&modding_software(&software).unwrap()), fx["software"].as_str().unwrap());
    assert_eq!(pyjson::pydantic(&time::timezones()), fx["timezones"].as_str().unwrap());
    for r in fx["now"].as_array().unwrap() {
        let unix = r[0].as_f64().unwrap().trunc() as i64;
        assert_eq!(pyjson::pydantic(&time::time_now(&src, unix).unwrap()), r[1].as_str().unwrap());
    }
    for r in fx["convert"].as_array().unwrap() {
        let got = time::convert_time(&src, r[0].as_str(), r[1].as_str().unwrap(), r[2].as_i64());
        match (got, r[3].as_str(), r[4].as_str()) {
            (Ok(v), Some(body), None) => assert_eq!(pyjson::pydantic(&v), body, "{r}"),
            (Err(time::TimeError::Bad(m)), None, Some(err)) => assert_eq!(m, err, "{r}"),
            (got, _, _) => panic!("{r}: {got:?}"),
        }
    }
}

#[test]
fn linux_only_lookups() {
    let src = source();
    // A directory is not a zone, and keys are case-sensitive (on Linux).
    for key in ["America", "America/", "Europe/../Europe/Paris", "/Europe/Paris"] {
        assert!(time::tzinfo(&src, key).is_err(), "{key}");
    }
    if cfg!(unix) {
        assert!(time::tzinfo(&src, "america/new_york").is_err());
    }
    let pkg = time::ZoneSource::new(Vec::new(), Some(testdata("zoneinfo")));
    assert!(pkg.get("America/New_York").is_some());
    assert!(pkg.get("America.Argentina/Buenos_Aires").is_some());
    assert!(pkg.get("America/Argentina").is_none());
}

#[test]
fn out_of_range_is_a_500() {
    let src = source();
    assert_eq!(time::convert_time(&src, None, "UTC", Some(253_402_300_799)), Err(time::TimeError::Internal));
    assert_eq!(time::convert_time(&src, None, "UTC", Some(-62_135_596_800)), Err(time::TimeError::Internal));
    assert!(time::convert_time(&src, None, "UTC", Some(253_402_261_199)).is_ok());
    let early = time::convert_time(&src, None, "UTC", Some(-61_000_000_000)).unwrap();
    assert_eq!(early["zones"][1]["date"], "Dec 26, 36");
    let missing = time::ZoneSource::new(Vec::new(), None);
    assert_eq!(
        time::convert_time(&missing, None, "UTC", Some(0)),
        Err(time::TimeError::Bad("Unknown timezone 'America/Sao_Paulo'.".into()))
    );
    assert!(time::time_now(&missing, 0).is_err());
}

#[test]
fn feedback_helpers_match_python() {
    let fx = load("feedback_helpers.json");
    for r in fx["ua"].as_array().unwrap() {
        let ua = r[0].as_str().map(|u| kiwi_core::pystr::prefix(u, 300));
        let (os, client) = feedback::parse_user_agent(ua);
        assert_eq!(json!([os, client]), json!(r.as_array().unwrap()[1..3]), "{ua:?}");
    }
    for r in fx["filenames"].as_array().unwrap() {
        assert_eq!(feedback::sanitise_filename(r[0].as_str()), r[1].as_str().unwrap(), "{r}");
    }
}

fn expand(text: &str) -> Vec<u8> {
    let text = text.replace("\0BIG\0", &"z".repeat(1024 * 1024 + 5)).replace("\0MIB\0", &"z".repeat(1024 * 1024));
    text.chars().map(|c| c as u32 as u8).collect()
}

#[test]
fn forms_match_python() {
    let rows = load("forms.json");
    for (n, row) in rows.as_array().unwrap().iter().enumerate() {
        let ctype = row[0].as_str().unwrap();
        let body = expand(row[1].as_str().unwrap());
        let ct = (!ctype.is_empty()).then_some(ctype.as_bytes());
        let (status, result) = match form::parse_form(ct, &body) {
            Err(form::FormError::Http(m)) => (400, json!({"code": "bad_request", "message": m, "details": null})),
            Ok(items) => match feedback::validate(&items) {
                Err(errors) => {
                    let mut details: Vec<Value> = errors.iter().map(|e| e.to_value()).collect();
                    for d in &mut details {
                        if let Some(s) = d["input"].as_str().filter(|s| s.chars().count() > 4096) {
                            d["input"] = json!({"sha1": hex::encode(Sha1::digest(s.as_bytes()))});
                        }
                    }
                    (
                        422,
                        json!({"code": "validation_error", "message": "Request validation failed", "details": details}),
                    )
                }
                Ok(f) => {
                    let files: Vec<Value> = f
                        .attachments
                        .iter()
                        .map(|a| {
                            json!([a.filename, a.content_type(), a.data.len(), hex::encode(Sha1::digest(&a.data))])
                        })
                        .collect();
                    (
                        200,
                        json!({"message": f.message, "contact": f.contact, "category": f.category,
                               "app_version": f.app_version, "files": files}),
                    )
                }
            },
        };
        assert_eq!((status, &result), (row[2].as_i64().unwrap(), &row[3]), "case {n}: {ctype:?} {:?}", row[1]);
    }
}

#[test]
fn webhook_payload() {
    let hook = feedback::Webhook {
        doc_id: "65f0a1b2c3d4e5f601234567".into(),
        category: "bug".into(),
        message: "é".repeat(1501),
        contact: Some("me".into()),
        app_version: None,
        os_label: Some("Windows 10/11".into()),
        client_label: None,
        files: vec![("a.png".into(), vec![1], "image/png".into()), ("a.png".into(), vec![2], "image/png".into())],
    };
    assert!(feedback::http_client().is_some());
    let (payload, names) = hook.payload("2026-10-22T10:00:00.000001+00:00");
    assert_eq!(names, ["0_a.png", "1_a.png"]);
    let main = &payload["embeds"][0];
    assert_eq!(main["description"].as_str().unwrap().chars().count(), 1502);
    assert!(main["description"].as_str().unwrap().ends_with("é …"));
    assert_eq!(main["color"], 0xf85149);
    assert_eq!(
        main["fields"],
        json!([{"name": "OS", "value": "Windows 10/11", "inline": true},
        {"name": "Contact", "value": "me", "inline": false}])
    );
    assert_eq!(main["image"]["url"], "attachment://0_a.png");
    assert_eq!(
        payload["embeds"][1],
        json!({"url": "https://api.aallyn.net/feedback/65f0a1b2c3d4e5f601234567", "image": {"url": "attachment://1_a.png"}})
    );
}

mod http {
    use std::sync::Arc;

    use axum::{Router, body::Body, http::Request};
    use http_body_util::BodyExt;
    use hyper_util::{client::legacy::Client, rt::TokioExecutor};
    use mongodb::bson::{Bson, Document, doc};
    use serde_json::Value;
    use tower::ServiceExt;

    use crate::{
        config::Settings,
        db::Db,
        state::{AppState, Inner},
    };

    fn bare_app() -> Router {
        let settings = Settings::from_vars([("PY_UPSTREAM", "http://127.0.0.1:9")]).unwrap();
        let upstream = Client::builder(TokioExecutor::new()).build_http();
        let db = Db { mongo: None, redis: None, pg: None };
        crate::routes::app(AppState { inner: Arc::new(Inner { settings, db, upstream }) })
    }

    async fn send(app: &Router, req: Request<Body>) -> (u16, axum::http::HeaderMap, Value) {
        let res = app.clone().oneshot(req).await.unwrap();
        let (parts, body) = res.into_parts();
        let bytes = body.collect().await.unwrap().to_bytes();
        (parts.status.as_u16(), parts.headers, serde_json::from_slice(&bytes).unwrap_or(Value::Null))
    }

    fn post(uri: &str, ctype: &str, body: &'static [u8]) -> Request<Body> {
        Request::post(uri).header("content-type", ctype).body(Body::from(body)).unwrap()
    }

    /// Errors FastAPI raises before any dependency touches a store.
    #[tokio::test]
    async fn early_errors_need_no_store() {
        let app = bare_app();
        let (status, _, body) = send(&app, post("/v1/misc/time/convert", "application/json", b"{\"unix\": ")).await;
        assert_eq!(status, 422);
        assert_eq!(body["error"]["details"][0]["type"], "json_invalid");

        let (status, _, body) = send(&app, post("/v1/misc/feedback", "multipart/form-data", b"x")).await;
        assert_eq!((status, body["error"]["message"].as_str()), (400, Some("Missing boundary in multipart.")));

        let urlenc = "application/x-www-form-urlencoded";
        let (status, _, body) = send(&app, post("/v1/misc/feedback", urlenc, b"message=hi")).await;
        assert_eq!(status, 422);
        assert_eq!(body["error"]["details"][0]["type"], "string_too_short");

        let (status, _, body) =
            send(&app, post("/v1/misc/feedback", urlenc, b"message=hello+world&category=spam")).await;
        assert_eq!(status, 400);
        assert_eq!(body["error"]["message"], "category must be one of 'bug', 'feature', 'general'.");

        let req = Request::get("/v1/misc/feedback").body(Body::empty()).unwrap();
        let (status, headers, _) = send(&app, req).await;
        assert_eq!((status, headers.get("allow").map(|v| v.to_str().unwrap())), (405, Some("POST")));
    }

    /// Needs throwaway stores (MONGO_URI / MONGO_DB / REDIS_URL) and the repo
    /// as the working directory for `app/trove/gamedata`:
    /// `cargo test -p kiwi-server misc_store_roundtrip -- --ignored`.
    #[tokio::test]
    #[ignore = "writes to MONGO_URI / REDIS_URL"]
    async fn misc_store_roundtrip() {
        if std::env::var("PY_UPSTREAM").is_err() {
            // SAFETY: only this ignored test runs in its process invocation.
            unsafe { std::env::set_var("PY_UPSTREAM", "http://127.0.0.1:9") };
        }
        let state = AppState::new(Settings::from_env().unwrap()).await.unwrap();
        let db = state.inner.db.mongo.clone().expect("MONGO_URI");
        let app = crate::routes::app(state);

        for path in ["/v1/misc/timezones", "/v1/misc/time/now", "/v1/misc/interest-items"] {
            let (status, headers, body) = send(&app, Request::get(path).body(Body::empty()).unwrap()).await;
            assert_eq!(status, 200, "{path}: {body}");
            assert!(headers.contains_key("x-ratelimit-limit"));
        }
        let convert = b"{\"datetime\": \"2026-06-05T14:30\", \"timezone\": \"trove\"}";
        let (status, _, body) = send(&app, post("/v1/misc/time/convert", "application/json", convert)).await;
        assert_eq!((status, body["unix"].as_i64()), (200, Some(1_780_713_000)));

        let form: &'static [u8] =
            b"--b\r\nContent-Disposition: form-data; name=\"message\"\r\n\r\n  store roundtrip  \r\n\
            --b\r\nContent-Disposition: form-data; name=\"attachments\"; filename=\"../x y.png\"\r\n\
            Content-Type: image/PNG\r\n\r\nPNG\r\n--b--\r\n";
        let req = Request::post("/v1/misc/feedback")
            .header("content-type", "multipart/form-data; boundary=b")
            .header("user-agent", "BetterTroveTools/2026.06.07 (Android 14; Pixel 8)")
            .body(Body::from(form))
            .unwrap();
        let (status, headers, ack) = send(&app, req).await;
        assert_eq!(status, 200, "{ack}");
        assert!(headers.contains_key("x-ratelimit-remaining"));
        assert_eq!(ack["ok"], true);
        let coll = db.collection::<Document>(super::feedback::COLLECTION);
        let stored = coll.find_one(doc! {"message": "store roundtrip"}).sort(doc! {"_id": -1}).await.unwrap().unwrap();
        let keys: Vec<&str> = stored.keys().map(String::as_str).collect();
        assert_eq!(
            keys,
            [
                "_id",
                "message",
                "contact",
                "category",
                "app_version",
                "os",
                "client",
                "user_agent",
                "attachments",
                "created_at",
                "read"
            ]
        );
        assert_eq!(stored.get_str("os").unwrap(), "Android 14");
        assert_eq!(
            stored.get_array("attachments").unwrap()[0],
            Bson::Document(doc! {"filename": "x_y.png", "content_type": "image/png", "size": 3})
        );
        coll.delete_one(doc! {"_id": stored.get_object_id("_id").unwrap()}).await.unwrap();
    }
}
