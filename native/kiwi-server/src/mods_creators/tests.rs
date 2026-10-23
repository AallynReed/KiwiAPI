//! Creator links and tokens against Python (`testdata/mods_creators_issues`):
//! every route past auth over an in-memory store, and the pre-store gates of
//! both modules through the real router.

use axum::{
    body::Body,
    http::{HeaderName, HeaderValue, Method, Request},
    response::{IntoResponse, Response},
};
use serde_json::Value;
use tower::ServiceExt;

use super::{
    LinkBody, ScopeBody, connect_op, disconnect_op, issue_op, list_op, panel_op, revoke_op, rotate_op, scope_op,
};
use crate::{
    auth::models::{SITE_USERS, SiteUser, USERS, User},
    error::{self, ApiError},
    features::{self, Feature},
    mods_write_auth::testkit::{arr, body, check_after, check_response, golden, mem, model, rid, s, store, url_params},
    outbound::testkit::app,
    request_id,
    validation::validate,
};

const CL: &str = "/v1/mods/hub/creator-links";

async fn dispatch(case: &Value) -> (Response, std::sync::Arc<crate::mods_write_auth::mem::MemDb>) {
    let m = mem(case);
    let st = store(&m);
    let method = s(&case["method"]);
    let url = s(&case["url"]);
    let portal = || model::<User>(&m, USERS, &case["portal"]).expect("portal user").id;
    let site = || model::<SiteUser>(&m, SITE_USERS, &case["site"]).expect("site user");
    let resp = match method {
        "GET" if url == CL => list_op(&st, portal()).await,
        "POST" if url == CL => match body(case).await.and_then(|b| validate::<LinkBody>(&b)) {
            Ok(req) => connect_op(&st, portal(), &req).await,
            Err(e) => e.into_response(),
        },
        "GET" if url == "/site/mods/creator-token" => panel_op(&st, &site()).await,
        "POST" if url == "/site/mods/creator-token" => issue_op(&st, &mut site()).await,
        "POST" if url == "/site/mods/creator-token/rotate" => rotate_op(&st, &mut site()).await,
        _ => {
            let link = |t: &str| url_params(url, t).map(|(p, _)| p[0].1.clone());
            if let Some(id) = link("/v1/mods/hub/creator-links/{link_id}") {
                disconnect_op(&st, portal(), &id).await
            } else if let Some(id) = link("/site/mods/creator-connections/{link_id}") {
                if method == "DELETE" {
                    revoke_op(&st, &site(), &id).await
                } else {
                    match body(case).await.and_then(|b| validate::<ScopeBody>(&b)) {
                        Ok(req) => scope_op(&st, &site(), &id, &req).await,
                        Err(e) => e.into_response(),
                    }
                }
            } else {
                panic!("no route for {method} {url}");
            }
        }
    };
    (resp, m)
}

#[tokio::test]
async fn routes_match_python() {
    let mut bad = Vec::new();
    for case in arr(golden(), "creators") {
        let (resp, m) = request_id::scope(rid(case), dispatch(case)).await;
        bad.extend(check_response(case, resp).await);
        bad.extend(check_after(case, &m));
    }
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

/// The response a gate gives while its flag is off (no store needed to know).
fn off_reply(case: &Value) -> Option<Response> {
    let flags = case["flags"].as_object()?;
    if flags.is_empty() || case["status"] == 405 {
        return None;
    }
    let url = s(&case["url"]);
    let hub_off = flags.get(features::MODS_HUB_FLAG) == Some(&Value::Bool(false));
    Some(match (url.starts_with("/site/"), hub_off) {
        (true, true) => error::http_exception(
            axum::http::StatusCode::NOT_FOUND,
            None,
            &Method::from_bytes(s(&case["method"]).as_bytes()).unwrap(),
            url,
            &Default::default(),
        ),
        (true, false) => ApiError::not_found(crate::mods_issues::SITE_ISSUES_OFF).into_response(),
        (false, true) => features::disabled(features::ModsHub::NAME).into_response(),
        (false, false) => features::disabled(features::ModIssues::NAME).into_response(),
    })
}

#[tokio::test]
async fn gates_match_python() {
    let router = app(super::router().merge(crate::mods_issues::router()));
    let mut bad = Vec::new();
    for case in arr(golden(), "gated") {
        if let Some(resp) = request_id::scope(rid(case), async { off_reply(case) }).await {
            bad.extend(check_response(case, resp).await);
            continue;
        }
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
        let resp = router.clone().oneshot(req).await.expect("infallible");
        bad.extend(check_response(case, resp).await);
    }
    assert!(arr(golden(), "gated").len() >= 40);
    assert!(bad.is_empty(), "{}", bad.join("\n"));
}

#[test]
fn minted_token_shape() {
    let raw = format!("{}{}", super::service::CREATOR_TOKEN_PREFIX, crate::auth::tokens::token_urlsafe(24));
    assert_eq!(raw.len(), 13 + 32);
    assert!(raw[13..].bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'));
}
