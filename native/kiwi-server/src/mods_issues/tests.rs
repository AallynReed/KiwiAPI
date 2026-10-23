//! Issues against Python (`testdata/mods_creators_issues`): every route past
//! auth over an in-memory store, with the stored documents and the
//! rate-limit keys each write spends. The router gates are checked in
//! `mods_creators::tests`.

use std::sync::Arc;

use axum::response::{IntoResponse, Response};
use serde_json::Value;

use super::{
    BASE, CommentBody, CreateBody, IssuePath, ListArgs, SITE_BASE, StatusBody, comment_op, create_op,
    delete_comment_op, delete_issue_op, get_op, list_op, mark_seen_op, notifications_op, site_notifications_op,
    status_op, validate_params, validate_write, with_cache,
};
use crate::{
    auth::models::{SITE_USERS, SiteUser},
    mods_write_auth::{
        ModWriteAuth,
        mem::MemDb,
        store::Store,
        testkit::{
            arr, body, check_after, check_response, golden, limits, mem, model, request_headers, rid, s, store,
            url_params, want_limits,
        },
    },
    pyjson::PydanticJson,
    request_id,
};

fn site_user(m: &MemDb, id: &Value) -> Option<SiteUser> {
    model::<SiteUser>(m, SITE_USERS, id)
}

fn writer(m: &MemDb, case: &Value) -> ModWriteAuth {
    ModWriteAuth { user: site_user(m, &case["write"]).expect("writer"), link: None, token: None, rate_limit: None }
}

async fn write<B: crate::validation::FromBody>(case: &Value, template: &str) -> Result<(IssuePath, B), Response> {
    let (params, query) = url_params(s(&case["url"]), template).unwrap();
    let input = body(case).await.map_err(IntoResponse::into_response)?;
    validate_write::<B>(params, query.as_deref(), &request_headers(case), &input).map_err(IntoResponse::into_response)
}

fn read<T: crate::validation::FromParams>(case: &Value, template: &str) -> Result<T, Box<Response>> {
    let (params, query) = url_params(s(&case["url"]), template).unwrap();
    validate_params::<T>(params, query.as_deref(), &request_headers(case)).map_err(|e| Box::new(e.into_response()))
}

async fn dispatch(case: &Value, m: &Arc<MemDb>) -> Response {
    let st: Store = store(m);
    let url = s(&case["url"]);
    let path = url.split('?').next().unwrap();
    let method = s(&case["method"]);
    let viewer = site_user(m, &case["viewer"]);
    let is = |t: &str| url_params(path, t).is_some();
    let item = format!("{BASE}/{{number}}");
    let comments = format!("{BASE}/{{number}}/comments");
    let comment = format!("{BASE}/{{number}}/comments/{{event_id}}");
    let site_item = format!("{SITE_BASE}/{{number}}");
    match method {
        "GET" if is(BASE) => match read::<ListArgs>(case, BASE) {
            Ok(q) => list_op(&st, None, &q)
                .await
                .map(|v| PydanticJson(v).into_response())
                .unwrap_or_else(|e| e.into_response()),
            Err(r) => *r,
        },
        "GET" if is(&item) => match read::<IssuePath>(case, &item) {
            Ok(q) => get_op(&st, None, &q)
                .await
                .map(|v| PydanticJson(v).into_response())
                .unwrap_or_else(|e| e.into_response()),
            Err(r) => *r,
        },
        "GET" if is(SITE_BASE) => match read::<ListArgs>(case, SITE_BASE) {
            Ok(q) => list_op(&st, viewer.as_ref(), &q)
                .await
                .map(|v| with_cache(v, "no-cache"))
                .unwrap_or_else(|e| e.into_response()),
            Err(r) => *r,
        },
        "GET" if is(&site_item) => match read::<IssuePath>(case, &site_item) {
            Ok(q) => get_op(&st, viewer.as_ref(), &q)
                .await
                .map(|v| with_cache(v, "no-cache"))
                .unwrap_or_else(|e| e.into_response()),
            Err(r) => *r,
        },
        "GET" if path == "/site/mods/notifications" => site_notifications_op(&st, viewer.as_ref()).await,
        "GET" if path == "/v1/mods/hub/me/issue-notifications" => notifications_op(&st, &writer(m, case)).await,
        "POST" if path == "/v1/mods/hub/me/issue-notifications/seen" => mark_seen_op(&st, &writer(m, case)).await,
        "POST" if is(BASE) => match write::<CreateBody>(case, BASE).await {
            Ok((p, b)) => create_op(&st, &writer(m, case), &p, &b).await,
            Err(r) => r,
        },
        "POST" if is(&comments) => match write::<CommentBody>(case, &comments).await {
            Ok((p, b)) => comment_op(&st, &writer(m, case), &p, &b).await,
            Err(r) => r,
        },
        "PATCH" if is(&item) => match write::<StatusBody>(case, &item).await {
            Ok((p, b)) => status_op(&st, &writer(m, case), &p, &b).await,
            Err(r) => r,
        },
        "DELETE" if is(&item) => match read::<IssuePath>(case, &item) {
            Ok(p) => delete_issue_op(&st, &writer(m, case), &p).await,
            Err(r) => *r,
        },
        "DELETE" if is(&comment) => match read::<IssuePath>(case, &comment) {
            Ok(p) => delete_comment_op(&st, &writer(m, case), &p).await,
            Err(r) => *r,
        },
        _ => panic!("no route for {method} {url}"),
    }
}

#[tokio::test]
async fn routes_match_python() {
    let mut bad = Vec::new();
    let mut n = 0;
    for case in arr(golden(), "issues") {
        let name = s(&case["name"]);
        let m = mem(case);
        let resp = request_id::scope(rid(case), dispatch(case, &m)).await;
        if name == "is_uncomment_python_bug" {
            // Python's `to_oid(event_id, "event id")` raises; the port deletes the reply.
            assert_eq!(case["status"], 500);
            assert_eq!(resp.status().as_u16(), 204, "{name}");
            continue;
        }
        n += 1;
        bad.extend(check_response(case, resp).await);
        bad.extend(check_after(case, &m));
        if limits(&m) != want_limits(case) {
            bad.push(format!("{name}: rate-limit keys {:?} want {:?}", limits(&m), want_limits(case)));
        }
    }
    assert!(n > 80, "issue cases missing");
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn rate_limit_defaults_match_python() {
    let g = &golden()["rate_limits"];
    for (prefix, want) in g.as_object().unwrap() {
        let max = crate::runtime_config::spec(&format!("{prefix}_max")).unwrap();
        let window = crate::runtime_config::spec(&format!("{prefix}_window_seconds")).unwrap();
        let settings = crate::config::Settings::default();
        assert_eq!(max.default_value(&settings), want[0], "{prefix}");
        assert_eq!(window.default_value(&settings), want[1], "{prefix}");
    }
}
