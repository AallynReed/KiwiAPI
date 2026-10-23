//! The Dressing Room (`app/trove/dressing`): `/v1/dressing/*` and the
//! `/site/dressing/*` copies the page and partners call.

pub mod catalogue;
pub mod service;

#[cfg(test)]
mod tests;

use axum::{
    Router,
    extract::{FromRequestParts, Request, State},
    http::{HeaderMap, HeaderValue, StatusCode, header, request::Parts},
    response::{IntoResponse, Response},
    routing::get,
};
use kiwi_core::dressing::{self as kd, Catalogue};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

use self::service::{Outfit, Selection};
use crate::{
    auth::{Public, scopes},
    blueprints::cache::{self, Fmt},
    creature,
    error::{self, ApiError, ErrorCode, method_not_allowed},
    features::{self, DressingRoom, RequireFeature},
    pyjson::{PyJson, PydanticJson},
    state::AppState,
    validation::{FromParams, Params, Query},
};

/// `palette.HAIR`, as Trove's picker lays it out.
pub const HAIR: [&str; 18] = [
    "#7a4e24", "#e01b0c", "#d6197a", "#3fd016", "#2a46b8", "#1e2224", "#b5762a", "#f08010", "#f221c7", "#2fd9a8",
    "#7a18e0", "#8a9096", "#c2b24a", "#f2e06a", "#f9a8d8", "#3fe0f0", "#6e8cf0", "#ffffff",
];

/// `palette.EYE`.
pub const EYE: [&str; 18] = [
    "#7a1f1f", "#6b4a1f", "#1f5a33", "#1f5a9e", "#4a2a7a", "#1e2224", "#e01b0c", "#f08010", "#2fcf4a", "#5a93e8",
    "#8560c8", "#8a9096", "#f221c7", "#f2e020", "#3fe0a8", "#3fd8f0", "#a020f0", "#ffffff",
];

pub const COLUMNS: i64 = 6;
const MAX_KEY: usize = 220;
const STILL_MAX_AGE: &str = "public, max-age=31536000, immutable";
const OPTION_SLOTS: [&str; 7] = ["costume", "hat", "face", "weapon", "head", "hair", "eyes"];

pub fn router() -> Router<AppState> {
    let get_only = || async { method_not_allowed("GET") };
    Router::new()
        .route("/v1/dressing/palette", get(palette).fallback(get_only))
        .route("/v1/dressing/races", get(v1).fallback(get_only))
        .route("/v1/dressing/classes", get(v1).fallback(get_only))
        .route("/v1/dressing/options", get(v1).fallback(get_only))
        .route("/v1/dressing/outfit", get(v1).fallback(get_only))
        .route("/v1/dressing/image", get(v1).fallback(get_only))
        .route("/v1/dressing/model", get(v1).fallback(get_only))
        .route("/site/dressing/classes", get(site_classes).fallback(get_only))
        .route("/site/dressing/palette", get(site_palette).fallback(get_only))
        .route("/site/dressing/races", get(site_races).fallback(get_only))
        .route("/site/dressing/options", get(site_options).fallback(get_only))
        .route("/site/dressing/model", get(site_model).fallback(get_only))
        .route("/site/dressing/image", get(site_image).fallback(get_only))
        .route("/site/dressing/render", get(site_render).fallback(get_only))
}

pub fn palette_body() -> Value {
    json!({"columns": COLUMNS, "hair": HAIR, "eyes": EYE})
}

async fn palette(_: RequireFeature<DressingRoom>, Public(ctx): Public<{ scopes::CODEXES_READ }>) -> Response {
    (ctx.rate_limit, PydanticJson(palette_body())).into_response()
}

// --- the read models -------------------------------------------------------------------

fn empty() -> ApiError {
    ApiError::new(
        StatusCode::SERVICE_UNAVAILABLE,
        ErrorCode::NotFound,
        "The dressing room catalogue isn't built yet - the game archive hasn't been indexed on this instance.",
    )
}

async fn catalogue(state: &AppState) -> Result<std::sync::Arc<Catalogue>, ApiError> {
    catalogue::get(state).await.map_err(|e| {
        tracing::error!("dressing catalogue: {e:#}");
        ApiError::internal()
    })
}

async fn built(state: &AppState) -> Result<std::sync::Arc<Catalogue>, ApiError> {
    let cat = catalogue(state).await?;
    if cat.is_empty() { Err(empty()) } else { Ok(cat) }
}

pub fn races_value(cat: &Catalogue) -> Value {
    let items: Vec<Value> = cat
        .races
        .iter()
        .map(|r| json!({"key": r.key, "name": r.name, "heads": r.pieces_of("head").len(), "eyes": r.pieces_of("eyes").len()}))
        .collect();
    json!({"items": items})
}

pub fn classes_value(cat: &Catalogue) -> Value {
    let mut classes: Vec<&kd::DressClass> = cat.classes.iter().collect();
    classes.sort_by_cached_key(|c| c.name.to_lowercase());
    let items: Vec<Value> = classes
        .into_iter()
        .map(|c| {
            let sockets: Vec<Value> =
                c.sockets.iter().map(|s| json!({"ap": s.ap, "slot": s.slot, "family": s.family})).collect();
            let slots: Vec<&str> = ["costume", "head", "hair", "eyes", "hat", "face", "weapon"]
                .into_iter()
                .filter(|s| !service::is_direct(s) || service::attach_point(s, &c.skeleton).is_some())
                .collect();
            json!({
                "key": c.key, "name": c.name, "skeleton": c.skeleton, "weapons": c.weapons,
                "sockets": sockets, "costumes": cat.costumes_of(&c.key).len(), "slots": slots,
            })
        })
        .collect();
    json!({"items": items})
}

pub struct OptionsQ {
    pub slot: String,
    pub class_key: Option<String>,
    pub race: Option<String>,
    pub q: Option<String>,
    pub offset: i64,
    pub limit: i64,
}

impl FromParams for OptionsQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        OptionsQ {
            slot: p.query("slot").str().one_of_pattern(&OPTION_SLOTS).required(),
            class_key: p.query("class").str().optional(),
            race: p.query("race").str().optional(),
            q: p.query("q").str().max_len(80).optional(),
            offset: p.query("offset").int().ge(0).default(0),
            limit: p.query("limit").int().ge(1).le(500).default(100),
        }
    }
}

pub fn options_value(cat: &Catalogue, q: &OptionsQ) -> Result<Value, ApiError> {
    let cls = q.class_key.as_deref().filter(|k| !k.is_empty()).and_then(|k| cat.class(&k.to_lowercase()));
    let mut items: Vec<&std::sync::Arc<kd::Opt>> = if q.slot == "costume" {
        let cls = cls.ok_or_else(|| ApiError::bad_request("`class` is required for costumes."))?;
        cat.costumes_of(&cls.key).iter().collect()
    } else {
        let mut items: Vec<_> = cat.styles_of(&q.slot).iter().collect();
        if q.slot == "weapon"
            && let Some(cls) = cls
        {
            items.retain(|o| cls.sockets.iter().any(|s| s.slot == o.slot_id));
        }
        if let Some(chosen) = cat.race(&q.race.as_deref().unwrap_or("").to_lowercase())
            && (q.slot == "head" || q.slot == "eyes")
        {
            let mine: Vec<String> = chosen.pieces_of(&q.slot).iter().map(|b| b.to_lowercase()).collect();
            items.retain(|o| mine.contains(&o.key));
        }
        items
    };
    if let Some(needle) = q.q.as_deref().filter(|s| !s.is_empty()) {
        let needle = kiwi_core::pystr::strip(needle).to_lowercase();
        items.retain(|o| o.name.to_lowercase().contains(&needle) || o.key.contains(&needle));
    }
    let total = items.len();
    let start = usize::try_from(q.offset).unwrap_or(usize::MAX).min(total);
    let end = start.saturating_add(usize::try_from(q.limit).unwrap_or(usize::MAX)).min(total);
    let page: Vec<Value> = items[start..end]
        .iter()
        .map(|o| {
            json!({
                "key": o.key, "name": o.name, "slot": o.slot, "family": o.family, "blueprint": o.blueprint,
                "prefab": o.prefab, "credit": o.credit, "covers_head": o.covers_head,
            })
        })
        .collect();
    Ok(json!({"items": page, "total": total, "offset": q.offset, "limit": q.limit}))
}

fn v1_json(rl: crate::ratelimit::RateLimitInfo, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => (rl, PydanticJson(v)).into_response(),
        Err(e) => e.into_response(),
    }
}

fn site_json(max_age: u32, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => {
            let mut res = PyJson(v).into_response();
            if let Ok(h) = HeaderValue::from_str(&format!("public, max-age={max_age}")) {
                res.headers_mut().insert(header::CACHE_CONTROL, h);
            }
            res
        }
        Err(e) => e.into_response(),
    }
}

// --- an outfit ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq)]
enum Surface {
    V1Outfit,
    V1Image,
    V1Model,
    SiteModel,
    SiteImage,
}

struct OutfitQ {
    sel: Selection,
    size: i64,
    az: f64,
    el: f64,
    fmt: Fmt,
}

fn read_outfit(p: &mut Params<'_>, surface: Surface) -> OutfitQ {
    let class_key = p.query("class").str().required();
    let mut opt = |name: &str| (name.to_owned(), p.query(name).str().optional());
    let mut picks = vec![
        opt("costume"),
        opt("hat"),
        opt("face"),
        opt("weapon"),
        opt("head"),
        opt("hair"),
        opt("eyes"),
        opt("race"),
    ];
    let site = matches!(surface, Surface::SiteModel | Surface::SiteImage);
    let (hair_color, eye_color, hair_scale, weapon_family) = if site {
        let weapon_family = p.query("weapon_family").str().optional();
        let hair_color = p.query("hair_color").str().max_len(7).optional();
        let eye_color = p.query("eye_color").str().max_len(7).optional();
        let hair_scale = p.query("hair_scale").float().ge(0.05).le(1.0).optional();
        (hair_color, eye_color, hair_scale, weapon_family)
    } else {
        let hair_color = p.query("hair_color").str().optional();
        let hair_scale = p.query("hair_scale").float().ge(0.05).le(1.0).optional();
        let eye_color = p.query("eye_color").str().optional();
        let weapon_family = p.query("weapon_family").str().optional();
        (hair_color, eye_color, hair_scale, weapon_family)
    };
    let (mut size, mut az, mut el, mut fmt) = (256, 30.0, 8.0, Fmt::Json);
    match surface {
        Surface::V1Image | Surface::SiteImage => {
            size = p.query("size").int().ge(32).le(512).default(256);
            az = p.query("az").float().ge(-360.0).le(360.0).default(30.0);
            el = p.query("el").float().ge(-89.0).le(89.0).default(8.0);
        }
        Surface::V1Model | Surface::SiteModel => {
            let f = p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into());
            fmt = if f == "bin" { Fmt::Bin } else { Fmt::Json };
        }
        Surface::V1Outfit => {}
    }
    let race = picks.pop().and_then(|(_, v)| v);
    let costume = picks.remove(0).1;
    OutfitQ {
        sel: Selection {
            class_key,
            costume,
            picks,
            weapon_family,
            race,
            colors: vec![("hair_color".into(), hair_color), ("eye_color".into(), eye_color)],
            hair_scale,
        },
        size,
        az,
        el,
        fmt,
    }
}

/// `_key(value)`.
fn key(v: Option<&str>) -> Result<Option<String>, ApiError> {
    let Some(v) = v else { return Ok(None) };
    let v = kiwi_core::pystr::strip(v).to_lowercase();
    if v.chars().count() > MAX_KEY {
        return Err(ApiError::bad_request("Selection value is too long."));
    }
    Ok(Some(v).filter(|s| !s.is_empty()))
}

/// `resolve_query(...)`: the outfit, or the 404 an unknown class is.
async fn resolve_query(state: &AppState, raw: &Selection) -> Result<Outfit, ApiError> {
    let mut sel = raw.clone();
    sel.class_key = key(Some(&raw.class_key))?.unwrap_or_default();
    sel.costume = key(raw.costume.as_deref())?;
    for (_, v) in sel.picks.iter_mut() {
        *v = key(v.as_deref())?;
    }
    sel.race = key(raw.race.as_deref())?;
    let cat = catalogue(state).await?;
    service::resolve(state, &cat, &sel)
        .await?
        .ok_or_else(|| ApiError::not_found("Unknown class, or no costumes for it."))
}

fn with_issues(mut res: Response, o: &Outfit) -> Response {
    if !o.issues.is_empty()
        && let Ok(h) = HeaderValue::from_str(&o.issue_header())
    {
        res.headers_mut().insert("x-dressing-dropped", h);
    }
    res
}

fn latin1(v: &HeaderValue) -> String {
    v.as_bytes().iter().map(|&b| char::from(b)).collect()
}

/// `still_response(request, png, outfit)`.
pub fn still_response(headers: &HeaderMap, png: Vec<u8>, o: &Outfit) -> Response {
    let etag = format!("\"{}\"", &hex::encode(Sha256::digest(&png))[..32]);
    let inm = headers.get(header::IF_NONE_MATCH).map(latin1);
    let mut res = if cache::etag_matches(inm.as_deref(), &etag) {
        let mut r = Response::new(axum::body::Body::empty());
        *r.status_mut() = StatusCode::NOT_MODIFIED;
        r
    } else {
        let mut r = (StatusCode::OK, png).into_response();
        r.headers_mut().insert(header::CONTENT_TYPE, HeaderValue::from_static("image/png"));
        r
    };
    let h = res.headers_mut();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(STILL_MAX_AGE));
    if let Ok(v) = HeaderValue::from_str(&etag) {
        h.insert(header::ETAG, v);
    }
    with_issues(res, o)
}

fn parse_outfit(parts: &Parts, surface: Surface) -> Result<OutfitQ, crate::validation::ValidationError> {
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let q = read_outfit(&mut p, surface);
    p.v.finish(q)
}

type V1 = Public<{ scopes::CODEXES_READ }>;

async fn v1(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    match v1_prelude(&s, &mut parts).await {
        Ok(ctx) => v1_route(&s, &parts, ctx.rate_limit).await,
        Err(r) => r,
    }
}

/// A `/v1/dressing/*` read once the gate and the public scope have passed.
pub(crate) async fn v1_route(s: &AppState, parts: &Parts, rl: crate::ratelimit::RateLimitInfo) -> Response {
    let gone = || ApiError::not_found("That outfit has nothing to draw on this instance.").into_response();
    match parts.uri.path() {
        "/v1/dressing/races" => v1_json(rl, built(s).await.map(|c| races_value(&c))),
        "/v1/dressing/classes" => v1_json(rl, built(s).await.map(|c| classes_value(&c))),
        "/v1/dressing/options" => {
            let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
            let q = OptionsQ::from_params(&mut p);
            let q = match p.v.finish(q) {
                Ok(q) => q,
                Err(e) => return e.into_response(),
            };
            let r = async {
                let cat = built(s).await?;
                options_value(&cat, &q)
            };
            v1_json(rl, r.await)
        }
        "/v1/dressing/outfit" => match parse_outfit(parts, Surface::V1Outfit) {
            Ok(q) => v1_json(rl, resolve_query(s, &q.sel).await.map(|o| o.outfit_value())),
            Err(e) => e.into_response(),
        },
        "/v1/dressing/image" => match parse_outfit(parts, Surface::V1Image) {
            Ok(q) => image_reply(s, &parts.headers, &q, gone).await,
            Err(e) => e.into_response(),
        },
        _ => match parse_outfit(parts, Surface::V1Model) {
            Ok(q) => model_reply(s, &parts.headers, &q, gone).await,
            Err(e) => e.into_response(),
        },
    }
}

async fn v1_prelude(s: &AppState, parts: &mut Parts) -> Result<crate::auth::AccessContext, Response> {
    features::check::<DressingRoom>(s).await.map_err(IntoResponse::into_response)?;
    let Public(ctx) = V1::from_request_parts(parts, s).await.map_err(IntoResponse::into_response)?;
    Ok(ctx)
}

async fn image_reply(s: &AppState, headers: &HeaderMap, q: &OutfitQ, missing: impl FnOnce() -> Response) -> Response {
    let o = match resolve_query(s, &q.sel).await {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    match service::still(s, &o, q.size, q.az, q.el).await {
        Ok(Some(png)) if !png.is_empty() => still_response(headers, png, &o),
        Ok(_) => missing(),
        Err(r) => r,
    }
}

async fn model_reply(s: &AppState, headers: &HeaderMap, q: &OutfitQ, missing: impl FnOnce() -> Response) -> Response {
    let o = match resolve_query(s, &q.sel).await {
        Ok(o) => o,
        Err(e) => return e.into_response(),
    };
    match service::model(s, &o, q.fmt).await {
        Ok(Some(c)) => with_issues(cache::respond(headers, &c, 300), &o),
        Ok(None) => missing(),
        Err(r) => r,
    }
}

// --- /site ----------------------------------------------------------------------------------

async fn site_gate(s: &AppState, parts: &Parts) -> Result<(), Response> {
    crate::workshop::site_gate(s, parts, Some(features::DRESSING_ROOM_FLAG)).await
}

fn http(parts: &Parts, status: StatusCode, detail: &str) -> Response {
    error::http_exception(status, Some(detail), &parts.method, parts.uri.path(), &parts.headers)
}

async fn site_classes(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    site_json(300, built(&s).await.map(|c| classes_value(&c)))
}

async fn site_palette(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    site_json(3600, Ok(palette_body()))
}

async fn site_races(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    site_json(300, built(&s).await.map(|c| races_value(&c)))
}

async fn site_options(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    let q = match Query::<OptionsQ>::from_request_parts(&mut parts, &s).await {
        Ok(Query(q)) => q,
        Err(r) => return r.into_response(),
    };
    let r = async {
        let cat = built(&s).await?;
        options_value(&cat, &q)
    };
    site_json(300, r.await)
}

async fn site_model(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    let q = match parse_outfit(&parts, Surface::SiteModel) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    model_reply(&s, &parts.headers, &q, || http(&parts, StatusCode::NOT_FOUND, "That outfit has nothing to draw."))
        .await
}

async fn site_image(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    let q = match parse_outfit(&parts, Surface::SiteImage) {
        Ok(q) => q,
        Err(e) => return e.into_response(),
    };
    image_reply(&s, &parts.headers, &q, || http(&parts, StatusCode::NOT_FOUND, "That outfit has nothing to draw."))
        .await
}

struct RenderQ {
    blueprint: Option<String>,
    prefab: Option<String>,
    dim: i64,
}

impl FromParams for RenderQ {
    fn from_params(p: &mut Params<'_>) -> Self {
        RenderQ {
            blueprint: p.query("blueprint").str().min_len(1).max_len(200).optional(),
            prefab: p.query("prefab").str().min_len(1).max_len(400).optional(),
            dim: p.query("dim").int().ge(32).le(256).default(96),
        }
    }
}

async fn site_render(State(s): State<AppState>, req: Request) -> Response {
    let (mut parts, _) = req.into_parts();
    if let Err(r) = site_gate(&s, &parts).await {
        return r;
    }
    let q = match Query::<RenderQ>::from_request_parts(&mut parts, &s).await {
        Ok(Query(q)) => q,
        Err(r) => return r.into_response(),
    };
    let branch = creature::LIVE_BRANCH;
    let Some(blueprint) = q.blueprint.filter(|b| !b.is_empty()) else {
        let Some(prefab) = q.prefab.filter(|p| !p.is_empty()) else {
            return http(&parts, StatusCode::NOT_FOUND, "Nothing to render.");
        };
        return match creature::creature_png(&s, &prefab, q.dim, branch).await {
            Ok(Some(png)) if !png.is_empty() => creature::png_response(png),
            Ok(_) => http(&parts, StatusCode::NOT_FOUND, "Nothing to assemble for that prefab."),
            Err(r) => r,
        };
    };
    let resolved =
        match service::blueprint_path(&s, &blueprint.to_lowercase(), q.prefab.as_deref().unwrap_or(""), branch, "")
            .await
        {
            Ok(r) => r,
            Err(e) => return e.into_response(),
        };
    use crate::blueprints::render::{RenderError, blueprint_png};
    match blueprint_png(&s, resolved.as_deref().unwrap_or(&blueprint), q.dim, branch).await {
        Ok(Some(png)) => creature::png_response(png),
        Ok(None) => http(&parts, StatusCode::NOT_FOUND, &format!("No blueprint '{blueprint}'")),
        Err(RenderError::Blueprint(m)) => {
            http(&parts, StatusCode::UNPROCESSABLE_ENTITY, &format!("Not renderable: {m}"))
        }
        Err(RenderError::Unhandled) => ApiError::internal().into_response(),
    }
}
