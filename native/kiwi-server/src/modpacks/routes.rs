//! The handlers of `app/trove/modpacks/router.py` (`/v1/modpacks/hub/*`
//! behind the Mods Hub gate, reads tokenless `mods:read`, writes the site
//! session), the documented `/v1/modpacks/*` catalog, the `/site/modpacks/*`
//! proxies of `app/site/router.py` and the master routes of
//! `app/admin/router.py`.

use axum::{
    extract::{FromRequestParts, Request, State},
    http::{HeaderValue, StatusCode, Uri, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::form::{self, FormError};
use serde_json::{Map, Value, json};

use super::{
    body::{
        AdminListQuery, ApiDownload, CollaboratorBody, CreateBody, CreateVariantBody, ListQuery, SetEntriesBody,
        SiteDownload, TakedownBody, UpdateBody, UpdateVariantBody,
    },
    model::Pack,
    service::{self, Res},
};
use crate::{
    auth::{CurrentSiteUser, OptionalSiteUser, Superuser, extract::Public, scopes::MODS_READ},
    error::{self, ApiError},
    features::{self, ModsHub, RequireFeature},
    middleware::pyurl::{quote, scope_path},
    pyjson::{PyJson, PydanticJson, no_content},
    state::AppState,
    validation::{DepsThenBody, Query},
};

type Gate = RequireFeature<ModsHub>;
type Writer = (Gate, CurrentSiteUser);

/// Path segment `i` as Starlette decodes it.
fn seg(uri: &Uri, i: usize) -> String {
    scope_path(uri).split('/').nth(i).unwrap_or_default().to_owned()
}

fn reply(r: Res<Response>) -> Response {
    r.unwrap_or_else(IntoResponse::into_response)
}

fn obj(m: Map<String, Value>) -> Value {
    Value::Object(m)
}

/// `Content-Disposition: attachment; filename="..."`. Starlette sends it
/// latin-1; a name outside latin-1 (a 500 there) also gets `filename*`.
pub fn disposition(name: &str) -> HeaderValue {
    if let Ok(v) = crate::media::latin1_header(&format!("attachment; filename=\"{name}\"")) {
        return v;
    }
    let fallback: String = name.chars().map(|c| if u32::from(c) < 0x100 { c } else { '_' }).collect();
    crate::media::latin1_header(&format!("attachment; filename=\"{fallback}\"; filename*=UTF-8''{}", quote(name, "")))
        .unwrap_or_else(|_| HeaderValue::from_static("attachment"))
}

fn artifact_response(blob: Vec<u8>, name: &str, media: &'static str, no_cache: bool) -> Response {
    let mut res = (StatusCode::OK, blob).into_response();
    let h = res.headers_mut();
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(media));
    h.insert(header::CONTENT_DISPOSITION, disposition(name));
    if no_cache {
        h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    }
    res
}

async fn download(state: &AppState, pack: Pack, variant: Option<&str>, fmt: &str, site: bool) -> Res<Response> {
    let (blob, name, media) = service::build_artifact(state, &pack, variant, fmt).await?;
    service::record_download(state, pack.id);
    Ok(artifact_response(blob, &name, media, site))
}

async fn detail(state: &AppState, pack: &Pack, viewer: &crate::auth::models::SiteUser) -> Res<Value> {
    Ok(obj(service::pack_detail(state, pack, Some(viewer.id)).await?))
}

// --- /v1/modpacks/hub: reads -----------------------------------------------

pub async fn hub_list(
    State(state): State<AppState>,
    _: Gate,
    Public(ctx): Public<{ MODS_READ }>,
    Query(q): Query<ListQuery>,
) -> Response {
    reply(
        async {
            let (docs, total) = service::browse(&state, &q).await?;
            let items: Vec<Value> = docs.iter().map(|p| obj(service::pack_card(p))).collect();
            let n = items.len();
            Ok((ctx.rate_limit, PydanticJson(json!({"items": items, "count": n, "total": total}))).into_response())
        }
        .await,
    )
}

pub async fn hub_get(State(state): State<AppState>, _: Gate, Public(ctx): Public<{ MODS_READ }>, uri: Uri) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 5), &seg(&uri, 6), None).await?;
            let body = obj(service::pack_detail(&state, &pack, None).await?);
            Ok((ctx.rate_limit, PydanticJson(body)).into_response())
        }
        .await,
    )
}

pub async fn hub_download(
    State(state): State<AppState>,
    _: Gate,
    _: Public<{ MODS_READ }>,
    Query(ApiDownload(q)): Query<ApiDownload>,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 5), &seg(&uri, 6), None).await?;
            download(&state, pack, q.variant.as_deref(), &q.format, false).await
        }
        .await,
    )
}

// --- /v1/modpacks/hub: owner listing and writes ----------------------------

pub async fn hub_mine(State(state): State<AppState>, _: Gate, CurrentSiteUser(user): CurrentSiteUser) -> Response {
    reply(async { Ok(PydanticJson(json!({"items": service::list_owned(&state, &user).await?})).into_response()) }.await)
}

pub async fn hub_create(
    State(state): State<AppState>,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, CreateBody>,
) -> Response {
    reply(
        async {
            let pack = service::create_modpack(&state, &user, &req).await?;
            Ok((StatusCode::CREATED, PydanticJson(detail(&state, &pack, &user).await?)).into_response())
        }
        .await,
    )
}

pub async fn hub_update(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, UpdateBody>,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::update_modpack(&state, pack, &user, &req).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_delete(
    State(state): State<AppState>,
    _: Gate,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            service::delete_modpack(&state, &pack, &user).await?;
            Ok(StatusCode::NO_CONTENT.into_response())
        }
        .await,
    )
}

fn parse_error(msg: &str) -> Response {
    ApiError::http(StatusCode::BAD_REQUEST, Some(msg)).into_response()
}

/// A `file: UploadFile = File(...)` route: the form is read before the gate
/// and the session, the field validated after them.
async fn upload_parts(state: &AppState, req: Request) -> Result<(Parts, CurrentSiteUser, Vec<u8>), Response> {
    let (mut parts, body) = req.into_parts();
    let Ok(bytes) = axum::body::to_bytes(body, usize::MAX).await else {
        return Err(parse_error("There was an error parsing the body"));
    };
    let items = match form::parse_form(parts.headers.get(header::CONTENT_TYPE).map(|v| v.as_bytes()), &bytes) {
        Ok(items) => items,
        Err(FormError::Http(msg)) => return Err(parse_error(&msg)),
    };
    drop(bytes);
    features::check::<ModsHub>(state).await.map_err(IntoResponse::into_response)?;
    let user = CurrentSiteUser::from_request_parts(&mut parts, state).await.map_err(IntoResponse::into_response)?;
    let data = crate::images::upload_file(items).map_err(IntoResponse::into_response)?;
    Ok((parts, user, data))
}

pub async fn hub_banner(State(state): State<AppState>, req: Request) -> Response {
    let (parts, CurrentSiteUser(user), data) = match upload_parts(&state, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&parts.uri, 5), &seg(&parts.uri, 6), &user).await?;
            let sha = super::hub::store_image(&state, user.id, data).await?;
            let pack = service::set_banner(&state, pack, sha).await?;
            Ok(PydanticJson(json!({"banner_sha": pack.banner_sha})).into_response())
        }
        .await,
    )
}

pub async fn hub_add_variant(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, CreateVariantBody>,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::add_variant(&state, pack, &req.name, req.copy_from.as_deref()).await?;
            Ok((StatusCode::CREATED, PydanticJson(detail(&state, &pack, &user).await?)).into_response())
        }
        .await,
    )
}

pub async fn hub_update_variant(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, UpdateVariantBody>,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::update_variant(&state, pack, &seg(&uri, 8), req.label.as_deref()).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_delete_variant(
    State(state): State<AppState>,
    _: Gate,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::delete_variant(&state, pack, &seg(&uri, 8)).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_set_entries(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, SetEntriesBody>,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::set_entries(&state, pack, &seg(&uri, 8), &req.entries).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_upload_entry(State(state): State<AppState>, req: Request) -> Response {
    let (parts, CurrentSiteUser(user), data) = match upload_parts(&state, req).await {
        Ok(x) => x,
        Err(r) => return r,
    };
    reply(
        async {
            let uri = &parts.uri;
            let pack = service::require_owned(&state, &seg(uri, 5), &seg(uri, 6), &user).await?;
            let up = service::upload_entry(&state, pack, &seg(uri, 8), data).await?;
            let detail = service::pack_detail(&state, &up.pack, Some(user.id)).await?;
            let mut head = Map::new();
            head.insert("matched_existing".into(), up.matched.is_some().into());
            if let Some((handle, slug)) = up.matched {
                head.insert("handle".into(), handle.into());
                head.insert("slug".into(), slug.into());
            }
            Ok((StatusCode::CREATED, PydanticJson(obj(service::merge(head, detail)))).into_response())
        }
        .await,
    )
}

pub async fn hub_add_collaborator(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody((_, CurrentSiteUser(user)), req): DepsThenBody<Writer, CollaboratorBody>,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::add_collaborator(&state, pack, &user, &req.username).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_remove_collaborator(
    State(state): State<AppState>,
    _: Gate,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::require_owned(&state, &seg(&uri, 5), &seg(&uri, 6), &user).await?;
            let pack = service::remove_collaborator(&state, pack, &user, &seg(&uri, 8)).await?;
            Ok(PydanticJson(detail(&state, &pack, &user).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_star(
    State(state): State<AppState>,
    _: Gate,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 5), &seg(&uri, 6), Some(user.id)).await?;
            Ok(PydanticJson(service::star_pack(&state, &user, pack).await?).into_response())
        }
        .await,
    )
}

pub async fn hub_unstar(
    State(state): State<AppState>,
    _: Gate,
    CurrentSiteUser(user): CurrentSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 5), &seg(&uri, 6), Some(user.id)).await?;
            Ok(PydanticJson(service::unstar_pack(&state, &user, pack).await?).into_response())
        }
        .await,
    )
}

// --- /v1/modpacks: the documented catalog ----------------------------------

pub async fn public_list(
    State(state): State<AppState>,
    _: Gate,
    Public(ctx): Public<{ MODS_READ }>,
    Query(q): Query<ListQuery>,
) -> Response {
    reply(
        async {
            let (docs, total) = service::browse(&state, &q).await?;
            let items: Vec<Value> =
                docs.iter().map(|p| obj(service::public_pack_card(service::Bases::of(&state), p))).collect();
            let n = items.len();
            let body = json!({"items": items, "count": n, "total": total, "limit": q.limit, "offset": q.offset});
            Ok((ctx.rate_limit, PydanticJson(body)).into_response())
        }
        .await,
    )
}

pub async fn public_for_mod(
    State(state): State<AppState>,
    _: Gate,
    Public(ctx): Public<{ MODS_READ }>,
    uri: Uri,
) -> Response {
    reply(
        async {
            let items = service::packs_for_mod(&state, &seg(&uri, 4), &seg(&uri, 5), true).await?;
            let n = items.len();
            Ok((ctx.rate_limit, PydanticJson(json!({"items": items, "count": n}))).into_response())
        }
        .await,
    )
}

pub async fn public_get(
    State(state): State<AppState>,
    _: Gate,
    Public(ctx): Public<{ MODS_READ }>,
    uri: Uri,
) -> Response {
    reply(
        async {
            let body = obj(service::public_detail(&state, &seg(&uri, 3), &seg(&uri, 4)).await?);
            Ok((ctx.rate_limit, PydanticJson(body)).into_response())
        }
        .await,
    )
}

pub async fn public_download(
    State(state): State<AppState>,
    _: Gate,
    _: Public<{ MODS_READ }>,
    Query(ApiDownload(q)): Query<ApiDownload>,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 3), &seg(&uri, 4), None).await?;
            download(&state, pack, q.variant.as_deref(), &q.format, false).await
        }
        .await,
    )
}

// --- /site/modpacks --------------------------------------------------------

/// The site router's `_resolve_feature_flags`: every site flag resolved (a
/// failed lookup is a 500), then the bare 404 while the Mods Hub is off.
pub struct SiteGate;

impl FromRequestParts<AppState> for SiteGate {
    type Rejection = Response;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, Response> {
        let mut hub_on = true;
        for flag in crate::activity::SITE_FLAGS {
            let on = features::is_enabled(state, flag).await.map_err(|e| {
                tracing::error!("site feature flags: {e:#}");
                ApiError::internal().into_response()
            })?;
            if flag == features::MODS_HUB_FLAG {
                hub_on = on;
            }
        }
        let path = parts.uri.path();
        if !hub_on && path.starts_with("/site/modpacks/") {
            return Err(error::http_exception(StatusCode::NOT_FOUND, None, &parts.method, path, &parts.headers));
        }
        Ok(SiteGate)
    }
}

fn site_json(body: Value, cache: &'static str) -> Response {
    ([(header::CACHE_CONTROL, HeaderValue::from_static(cache))], PyJson(body)).into_response()
}

pub async fn site_list(State(state): State<AppState>, _: SiteGate, Query(q): Query<ListQuery>) -> Response {
    reply(
        async {
            let (docs, total) = service::browse(&state, &q).await?;
            let items: Vec<Value> = docs.iter().map(|p| obj(service::pack_card(p))).collect();
            let n = items.len();
            Ok(site_json(json!({"items": items, "count": n, "total": total}), "public, max-age=30"))
        }
        .await,
    )
}

pub async fn site_mine(
    State(state): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
) -> Response {
    reply(
        async {
            let Some(user) = viewer else {
                return Err(ApiError::http(StatusCode::UNAUTHORIZED, Some("Sign in to view your modpacks.")));
            };
            Ok(site_json(json!({"items": service::list_owned(&state, &user).await?}), "no-cache"))
        }
        .await,
    )
}

pub async fn site_get(
    State(state): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    uri: Uri,
) -> Response {
    reply(
        async {
            let viewer = viewer.map(|u| u.id);
            let pack = service::get_for_view(&state, &seg(&uri, 4), &seg(&uri, 5), viewer).await?;
            Ok(site_json(obj(service::pack_detail(&state, &pack, viewer).await?), "no-cache"))
        }
        .await,
    )
}

pub async fn site_for_mod(State(state): State<AppState>, _: SiteGate, uri: Uri) -> Response {
    reply(
        async {
            let items = service::packs_for_mod(&state, &seg(&uri, 4), &seg(&uri, 5), false).await?;
            let n = items.len();
            Ok(site_json(json!({"items": items, "count": n}), "public, max-age=30"))
        }
        .await,
    )
}

pub async fn site_download(
    State(state): State<AppState>,
    _: SiteGate,
    OptionalSiteUser(viewer): OptionalSiteUser,
    Query(SiteDownload(q)): Query<SiteDownload>,
    uri: Uri,
) -> Response {
    reply(
        async {
            let pack = service::get_for_view(&state, &seg(&uri, 4), &seg(&uri, 5), viewer.map(|u| u.id)).await?;
            download(&state, pack, q.variant.as_deref(), &q.format, true).await
        }
        .await,
    )
}

// --- /admin/modpacks -------------------------------------------------------

pub async fn admin_list(State(state): State<AppState>, _: Superuser, Query(q): Query<AdminListQuery>) -> Response {
    reply(
        async {
            let (items, total) = service::master_list(&state, &q).await?;
            let n = items.len();
            Ok(PydanticJson(json!({"items": items, "count": n, "total": total})).into_response())
        }
        .await,
    )
}

pub async fn admin_delete(State(state): State<AppState>, _: Superuser, uri: Uri) -> Response {
    reply(
        async {
            service::master_delete(&state, &seg(&uri, 3)).await?;
            Ok(no_content())
        }
        .await,
    )
}

pub async fn admin_takedown(
    State(state): State<AppState>,
    uri: Uri,
    DepsThenBody(_, req): DepsThenBody<Superuser, TakedownBody>,
) -> Response {
    reply(
        async {
            let pack = service::take_down(&state, &seg(&uri, 3), &req.reason).await?;
            Ok(PydanticJson(json!({
                "slug": pack.slug, "handle": pack.owner_handle,
                "taken_down": pack.taken_down, "takedown_reason": pack.takedown_reason,
            }))
            .into_response())
        }
        .await,
    )
}

pub async fn admin_restore(State(state): State<AppState>, _: Superuser, uri: Uri) -> Response {
    reply(
        async {
            let pack = service::restore(&state, &seg(&uri, 3)).await?;
            Ok(PydanticJson(json!({"slug": pack.slug, "handle": pack.owner_handle, "taken_down": pack.taken_down}))
                .into_response())
        }
        .await,
    )
}
