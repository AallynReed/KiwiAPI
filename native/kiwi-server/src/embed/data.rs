//! The data the embeddable viewer fetches (`embed/service.py` and the
//! `/site/embed/*` routes): source resolution over a hub release, a held
//! upload, a game file, a game creature or a dressed character, then the
//! manifest, models, sounds and effects of that one source.

use std::collections::HashSet;
use std::sync::Arc;

use axum::{
    extract::{Request, State},
    http::{HeaderValue, header, request::Parts},
    response::{IntoResponse, Response},
};
use kiwi_core::updates::pkfx;
use serde_json::{Map, Value, json};

use crate::{
    auth::client::client_ip,
    blueprints::cache::{self, BuildError, Cached, Fmt},
    creature::{self, LIVE_BRANCH},
    dressing::service::{self as dress, Outfit, Selection},
    error::ApiError,
    features::{self, Embed},
    mod_releases::{
        access::release_with_project,
        artifact::{self, lower},
        read::{self as hub, blob},
        vfx::{self as hubvfx, Index, Resolver},
    },
    pyjson::PyJson,
    ratelimit::check_rate_limit,
    runtime_config,
    state::AppState,
    validation::Params,
};

const MED: &str = "public, max-age=300";

fn bad(msg: impl Into<String>) -> ApiError {
    ApiError::bad_request(msg)
}

fn missing(msg: impl Into<String>) -> ApiError {
    ApiError::not_found(msg)
}

fn internal(e: impl std::fmt::Display) -> ApiError {
    tracing::error!("embed: {e}");
    ApiError::internal()
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Release,
    Tmod,
    Game,
    Prefab,
    Dress,
}

impl Kind {
    fn as_str(self) -> &'static str {
        match self {
            Kind::Release => "release",
            Kind::Tmod => "tmod",
            Kind::Game => "game",
            Kind::Prefab => "prefab",
            Kind::Dress => "dress",
        }
    }
}

/// `service.Source`.
pub struct Source {
    pub kind: Kind,
    pub ident: String,
    pub title: String,
    pub tmod: Option<Vec<u8>>,
    pub game_path: Option<String>,
    pub prefab: Option<String>,
    pub outfit: Option<Outfit>,
    pub tmod_sha: Option<String>,
}

impl Source {
    fn new(kind: Kind, ident: String, title: String) -> Source {
        Source { kind, ident, title, tmod: None, game_path: None, prefab: None, outfit: None, tmod_sha: None }
    }

    pub fn cache_key(&self) -> String {
        format!("{}:{}", self.kind.as_str(), self.ident)
    }
}

/// `_source`'s query parameters.
pub struct SourceQ {
    release: Option<String>,
    tmod: Option<String>,
    game: Option<String>,
    prefab: Option<String>,
    dress: Option<String>,
}

fn read_source(p: &mut Params<'_>) -> SourceQ {
    SourceQ {
        release: p.query("release").str().max_len(64).optional(),
        tmod: p.query("tmod").str().max_len(64).optional(),
        game: p.query("game").str().max_len(400).optional(),
        prefab: p.query("prefab").str().max_len(400).optional(),
        dress: p.query("dress").str().max_len(700).optional(),
    }
}

fn given(v: &Option<String>) -> Option<&str> {
    v.as_deref().filter(|s| !s.is_empty())
}

/// `service.resolve(...)`.
pub async fn resolve(state: &AppState, q: &SourceQ) -> Result<Source, ApiError> {
    let all = [given(&q.release), given(&q.tmod), given(&q.game), given(&q.prefab), given(&q.dress)];
    if all.iter().flatten().count() != 1 {
        return Err(bad("Pass exactly one source: release, tmod, game, prefab or dress."));
    }
    if let Some(r) = given(&q.release) {
        return release_source(state, r).await;
    }
    if let Some(t) = given(&q.tmod) {
        return upload_source(state, t).await;
    }
    if let Some(p) = given(&q.prefab) {
        return prefab_source(state, p).await;
    }
    if let Some(d) = given(&q.dress) {
        return dress_source(state, d).await;
    }
    game_source(state, given(&q.game).unwrap_or("")).await
}

/// `parse_qs(token)` as `{key: first value}`, blanks dropped.
fn parse_qs(token: &str) -> Vec<(String, String)> {
    let dec = |s: &str| percent_encoding::percent_decode_str(&s.replace('+', " ")).decode_utf8_lossy().into_owned();
    let mut out: Vec<(String, String)> = Vec::new();
    for pair in token.split('&') {
        let Some((k, v)) = pair.split_once('=') else { continue };
        if v.is_empty() {
            continue;
        }
        let k = dec(k);
        if !out.iter().any(|(n, _)| *n == k) {
            out.push((k, dec(v)));
        }
    }
    out
}

async fn dress_source(state: &AppState, token: &str) -> Result<Source, ApiError> {
    let token = kiwi_core::pystr::strip(token);
    if token.is_empty() || token.chars().count() > 700 {
        return Err(bad("Missing or over-long outfit."));
    }
    let low = token.to_lowercase();
    let sel = if token.contains('=') {
        let q = parse_qs(&low);
        let get = |k: &str| q.iter().find(|(n, _)| n == k).map(|(_, v)| v.clone());
        Selection {
            class_key: get("class").unwrap_or_default(),
            costume: get("costume"),
            picks: ["hat", "face", "weapon", "head", "hair", "eyes"]
                .iter()
                .map(|k| ((*k).to_owned(), get(k)))
                .collect(),
            weapon_family: get("weapon_family"),
            race: get("race"),
            colors: ["hair_color", "eye_color"].iter().map(|k| ((*k).to_owned(), get(k))).collect(),
            hair_scale: None,
        }
    } else {
        let mut f: Vec<String> = low.split(':').map(str::to_owned).collect();
        f.resize(f.len().max(10), String::new());
        let s = |i: usize| Some(f[i].clone());
        Selection {
            class_key: f[0].clone(),
            costume: s(1),
            picks: vec![
                ("hat".into(), s(2)),
                ("face".into(), s(3)),
                ("weapon".into(), s(4)),
                ("head".into(), s(5)),
                ("hair".into(), s(6)),
                ("eyes".into(), s(8)),
            ],
            weapon_family: s(7),
            race: s(9),
            colors: Vec::new(),
            hair_scale: None,
        }
    };
    let cat = crate::dressing::catalogue::get(state).await.map_err(internal)?;
    let outfit = dress::resolve(state, &cat, &sel).await?.ok_or_else(|| missing("No such Trove class to dress."))?;
    let mut src = Source::new(Kind::Dress, outfit.ident(), format!("{} - {}", outfit.cls.name, outfit.costume.name));
    src.outfit = Some(outfit);
    Ok(src)
}

async fn rig_map(state: &AppState) -> Result<Arc<kiwi_core::rig::RigMap>, ApiError> {
    crate::creature::rig_map(state, &state.settings().trove_render_branch).await.map_err(internal)
}

async fn prefab_source(state: &AppState, name: &str) -> Result<Source, ApiError> {
    let name = kiwi_core::pystr::strip(name);
    if name.is_empty() || name.chars().count() > 400 {
        return Err(bad("Missing or over-long prefab path."));
    }
    let (path, candidates) = rig_map(state).await?.prefab_path(name);
    let Some(path) = path else {
        if let Some(first) = candidates.first() {
            return Err(bad(format!(
                "'{name}' is the filename of {} different creature prefabs. Pass the full path, e.g. '{first}'.",
                candidates.len()
            )));
        }
        return Err(missing(format!("No creature prefab '{name}' in the current game files.")));
    };
    let mut src = Source::new(Kind::Prefab, path.clone(), kiwi_core::rig::prefab_stem(&path));
    src.prefab = Some(path);
    Ok(src)
}

async fn release_source(state: &AppState, id: &str) -> Result<Source, ApiError> {
    let (rel, project) = release_with_project(state, id, None).await?;
    if rel.str("release_format") != "tmod" {
        return Err(missing("That release has nothing to preview."));
    }
    let sha = rel.str("tmod_sha").to_owned();
    let data = blob(state, &sha).await?.ok_or_else(|| missing("Release artifact not found."))?;
    let title = format!("{} {}", project.str("title"), rel.str("tag"));
    let mut src = Source::new(Kind::Release, id.to_owned(), kiwi_core::pystr::strip(&title).to_owned());
    src.tmod = Some(data);
    src.tmod_sha = Some(sha);
    Ok(src)
}

async fn upload_source(state: &AppState, token: &str) -> Result<Source, ApiError> {
    let data = super::load(state, token)
        .await
        .ok_or_else(|| missing("This preview has expired. Reload the page to start it again."))?;
    let title = artifact::read(&data, false)
        .ok()
        .and_then(|t| t.prop("title").map(|s| kiwi_core::pystr::strip(s).to_owned()))
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "Mod preview".into());
    let mut src = Source::new(Kind::Tmod, token.to_owned(), title);
    src.tmod = Some(data);
    src.tmod_sha = Some(token.to_owned());
    Ok(src)
}

async fn game_source(state: &AppState, path: &str) -> Result<Source, ApiError> {
    let path = kiwi_core::pystr::strip(path).replace('\\', "/");
    let path = path.trim_start_matches('/');
    if path.is_empty() || path.chars().count() > 400 {
        return Err(bad("Missing or over-long game file path."));
    }
    let low = path.to_lowercase();
    if ![".blueprint", ".pkfx", ".bnk"].iter().any(|e| low.ends_with(e)) {
        return Err(bad("Only .blueprint, .pkfx and .bnk game files can be previewed."));
    }
    let resolved = if file_sha(state, path).await?.is_some() {
        Some(path.to_owned())
    } else {
        let idx = dress::game_paths(state, LIVE_BRANCH).await?;
        idx.map.get(&pkfx::basename(path).to_lowercase()).cloned()
    };
    let resolved = resolved.ok_or_else(|| missing(format!("No '{path}' in the current game files.")))?;
    let title = resolved.rsplit('/').next().unwrap_or(&resolved).to_owned();
    let mut src = Source::new(Kind::Game, resolved.clone(), title);
    src.game_path = Some(resolved);
    Ok(src)
}

/// `updates_read.get_file_meta(LIVE_BRANCH, path)["content_sha256"]`.
async fn file_sha(state: &AppState, path: &str) -> Result<Option<String>, ApiError> {
    #[cfg(test)]
    if let Some(a) = dress::TEST_ARCHIVE.get() {
        use sha2::Digest;
        return Ok(a.files.get(path).map(|d| hex::encode(sha2::Sha256::digest(d))));
    }
    crate::media::file_sha(state, LIVE_BRANCH, path).await
}

async fn read_blob(state: &AppState, path: &str, sha: &str) -> Result<Option<Vec<u8>>, ApiError> {
    #[cfg(test)]
    if let Some(a) = dress::TEST_ARCHIVE.get() {
        let _ = sha;
        return Ok(a.files.get(path).cloned());
    }
    let _ = path;
    crate::media::read_blob(crate::media::update_store(state), sha.to_owned()).await
}

// --- the request prelude ------------------------------------------------------------------

/// `_throttle`.
async fn throttle(state: &AppState, parts: &Parts) -> Result<(), Response> {
    #[cfg(test)]
    if dress::TEST_ARCHIVE.get().is_some() {
        return Ok(());
    }
    let (max, window) = runtime_config::get_rate_limit(state, "embed_rate_limit")
        .await
        .map_err(|e| internal(format!("embed_rate_limit: {e:#}")).into_response())?;
    let ip = client_ip(parts).filter(|s| !s.is_empty()).unwrap_or_else(|| "unknown".into());
    check_rate_limit(state, &format!("embed:{ip}"), max, window).await.map_err(IntoResponse::into_response)?;
    Ok(())
}

/// The gate, `_source`, `_throttle`, then the endpoint's own parameters.
async fn begin<T>(
    state: &AppState,
    parts: &Parts,
    own: impl FnOnce(&mut Params<'_>) -> T,
) -> Result<(Source, T), Response> {
    features::check::<Embed>(state).await.map_err(IntoResponse::into_response)?;
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let q = read_source(&mut p);
    let src = if p.v.is_ok() { Some(resolve(state, &q).await.map_err(IntoResponse::into_response)?) } else { None };
    throttle(state, parts).await?;
    let t = own(&mut p);
    p.v.finish(()).map_err(IntoResponse::into_response)?;
    Ok((src.expect("validated source"), t))
}

fn json_reply(cache: &'static str, r: Result<Value, ApiError>) -> Response {
    match r {
        Ok(v) => {
            let mut res = PyJson(v).into_response();
            res.headers_mut().insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            res
        }
        Err(e) => e.into_response(),
    }
}

macro_rules! tri {
    ($e:expr) => {
        match $e {
            Ok(v) => v,
            Err(r) => return r,
        }
    };
}

fn fmt_of(s: &str) -> Fmt {
    if s == "bin" { Fmt::Bin } else { Fmt::Json }
}

// --- manifest ---------------------------------------------------------------------------------

fn animations(rig: &str) -> Vec<String> {
    crate::rigs::rig(rig).map(|r| r.animation_names()).unwrap_or_default()
}

fn manifest_value(source: &str, title: &str, blueprints: Value, vfx: Value, audio: Value) -> Value {
    json!({"source": source, "title": title, "blueprints": blueprints, "vfx": {"items": vfx}, "audio": {"items": audio}})
}

async fn manifest(state: &AppState, src: &Source) -> Result<Value, ApiError> {
    match src.kind {
        Kind::Game => game_manifest(state, src).await,
        Kind::Prefab => prefab_manifest(state, src).await,
        Kind::Dress => {
            let rig = src.outfit.as_ref().map(|o| o.cls.skeleton.clone()).unwrap_or_default();
            let bp = json!({"items": [{"path": src.ident, "size": null, "assembled": true}], "rig": rig, "animations": animations(&rig)});
            Ok(manifest_value("dress", &src.title, bp, json!([]), json!([])))
        }
        Kind::Release | Kind::Tmod => tmod_manifest(state, src).await,
    }
}

async fn game_manifest(state: &AppState, src: &Source) -> Result<Value, ApiError> {
    let path = src.game_path.clone().unwrap_or_default();
    let low = path.to_lowercase();
    let none = || json!({"items": [], "rig": null, "animations": []});
    if low.ends_with(".pkfx") {
        return Ok(manifest_value("game", &src.title, none(), json!([{"path": path, "size": null}]), json!([])));
    }
    if low.ends_with(".bnk") {
        return Ok(manifest_value("game", &src.title, none(), json!([]), json!([{"path": path, "size": null}])));
    }
    let base = pkfx::basename(&path).to_lowercase();
    let stem: String = base.chars().take(base.chars().count().saturating_sub(10)).collect();
    let rig = rig_map(state).await?.creature_for(&stem).map(|(s, _, _)| s).filter(|s| crate::rigs::has_baked_rig(s));
    let bp = json!({
        "items": [{"path": path, "size": null, "assembled": rig.is_some()}],
        "rig": rig, "animations": rig.as_deref().map(animations).unwrap_or_default(),
    });
    Ok(manifest_value("game", &src.title, bp, json!([]), json!([])))
}

async fn prefab_manifest(state: &AppState, src: &Source) -> Result<Value, ApiError> {
    let prefab = src.prefab.clone().unwrap_or_default();
    let (skeleton, parts) = rig_map(state).await?.creature_by_prefab(&prefab);
    let paths = dress::game_paths(state, LIVE_BRANCH).await?;
    let mut names: Vec<&String> = parts.iter().map(|(b, _)| b).collect();
    names.sort();
    let found: Vec<String> = names
        .into_iter()
        .filter_map(|b| {
            let cands = paths.all.get(&format!("{b}.blueprint")).cloned().unwrap_or_default();
            kiwi_core::workshop::nearest_path(&cands, &prefab).cloned()
        })
        .collect();
    let rig = skeleton.filter(|s| !s.is_empty() && !found.is_empty() && crate::rigs::has_baked_rig(s));
    let items: Vec<Value> =
        found.iter().map(|gp| json!({"path": gp, "size": null, "assembled": rig.is_some()})).collect();
    let bp = json!({"items": items, "rig": rig, "animations": rig.as_deref().map(animations).unwrap_or_default()});
    Ok(manifest_value("prefab", &src.title, bp, json!([]), json!([])))
}

fn unreadable() -> ApiError {
    bad("That file isn't a readable .tmod.")
}

async fn tmod_manifest(state: &AppState, src: &Source) -> Result<Value, ApiError> {
    let data = src.tmod.clone().unwrap_or_default();
    let read = tokio::task::spawn_blocking(move || -> Result<_, ApiError> {
        let t = artifact::read(&data, true).map_err(|_| unreadable())?;
        let (items, fns) = hub::blueprint_parts(&t);
        let (pkfx_items, _) = hubvfx::pkfx_and_index(&data).map_err(|_| unreadable())?;
        let banks = hub::by_ext(&t, ".bnk");
        Ok((items, fns, pkfx_items, banks))
    });
    let (items, fns, pkfx_items, banks) = read.await.map_err(internal)??;
    let (rig, anims, parts) = crate::mod_releases::rig::resolve_release(state, &fns).await?;
    let bp = hub::blueprints_value(items, &fns, rig, anims, &parts);
    let vfx = hubvfx::list_value(pkfx_items)["items"].take();
    let audio = if src.kind == Kind::Release { banks } else { json!([]) };
    Ok(manifest_value(src.kind.as_str(), &src.title, bp, vfx, audio))
}

pub async fn manifest_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, ()) = tri!(begin(&s, &parts, |_| ()).await);
    json_reply("public, max-age=60", manifest(&s, &src).await)
}

// --- blueprint / assembled -------------------------------------------------------------------------

async fn decode_game_path(state: &AppState, target: &str, fmt: Fmt) -> Result<Cached, BuildError> {
    let sha = file_sha(state, target).await?.ok_or_else(|| missing("Game file not found."))?;
    let (st, t, s2) = (state.clone(), target.to_owned(), sha.clone());
    let build = || async move {
        let raw = read_blob(&st, &t, &s2).await?.ok_or_else(|| missing("Game file not found."))?;
        Ok(tokio::task::spawn_blocking(move || hub::pack_blueprint(&raw, &t)).await.map_err(internal)??)
    };
    cache::get_or_build(state, &cache::key_for_file(&sha, target), fmt, build).await
}

async fn blueprint(state: &AppState, src: &Source, path: &str, fmt: Fmt) -> Result<Cached, BuildError> {
    match src.kind {
        Kind::Game => {
            let target = src.game_path.clone().unwrap_or_default();
            let want = kiwi_core::pystr::strip(path).to_lowercase();
            if !path.is_empty() && want != target.to_lowercase() && want != pkfx::basename(&target).to_lowercase() {
                return Err(missing("That blueprint isn't part of this preview.").into());
            }
            decode_game_path(state, &target, fmt).await
        }
        Kind::Prefab => {
            let prefab = src.prefab.clone().unwrap_or_default();
            let (_, parts) = rig_map(state).await?.creature_by_prefab(&prefab);
            let base = pkfx::basename(path).to_lowercase();
            let want = base.strip_suffix(".blueprint").unwrap_or(&base).to_owned();
            if want.is_empty() || !parts.iter().any(|(b, _)| *b == want) {
                return Err(missing("That blueprint isn't part of this creature.").into());
            }
            let paths = dress::game_paths(state, LIVE_BRANCH).await?;
            let cands = paths.all.get(&format!("{want}.blueprint")).cloned().unwrap_or_default();
            let target =
                kiwi_core::workshop::nearest_path(&cands, &prefab).ok_or_else(|| missing("Game file not found."))?;
            decode_game_path(state, target, fmt).await
        }
        Kind::Dress => Err(missing("A dressed character has no separate blueprint to show.").into()),
        Kind::Release | Kind::Tmod => {
            let data = src.tmod.clone().unwrap_or_default();
            let want = lower(kiwi_core::pystr::strip(path));
            let build = move || async move {
                Ok(tokio::task::spawn_blocking(move || {
                    let t = artifact::read(&data, true).map_err(|e| internal(format!("TmodError: {e}")))?;
                    let target = t.files.into_iter().find(|f| lower(&f.path) == want && want.ends_with(".blueprint"));
                    let target = target.ok_or_else(|| missing("No such blueprint in this release"))?;
                    hub::pack_blueprint(&target.data, &target.path)
                })
                .await
                .map_err(internal)??)
            };
            match src.tmod_sha.as_deref().filter(|s| !s.is_empty() && src.kind == Kind::Release) {
                Some(sha) => cache::get_or_build(state, &cache::key_for_tmod(sha, path), fmt, build).await,
                None => cache::build_uncached(fmt, build()).await,
            }
        }
    }
}

pub async fn blueprint_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, (path, fmt)) = tri!(
        begin(&s, &parts, |p| {
            let path = p.query("path").str().max_len(400).default(String::new());
            let fmt = p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into());
            (path, fmt)
        })
        .await
    );
    match blueprint(&s, &src, &path, fmt_of(&fmt)).await {
        Ok(c) => cache::respond(&parts.headers, &c, 300),
        Err(e) => e.into_response(),
    }
}

async fn assembled(state: &AppState, src: &Source, fmt: Fmt) -> Result<Option<Cached>, Response> {
    match src.kind {
        Kind::Game => {
            let path = src.game_path.clone().unwrap_or_default();
            if !path.to_lowercase().ends_with(".blueprint") {
                return Ok(None);
            }
            let base = pkfx::basename(&path).to_lowercase();
            let stem: String = base.chars().take(base.chars().count().saturating_sub(10)).collect();
            let found = rig_map(state).await.map_err(IntoResponse::into_response)?.creature_for(&stem);
            match found {
                Some((skel, prefab, parts)) if !skel.is_empty() && !prefab.is_empty() && !parts.is_empty() => {
                    creature::game_creature(state, &prefab, &skel, parts, fmt).await
                }
                _ => Ok(None),
            }
        }
        Kind::Prefab => {
            let prefab = src.prefab.clone().unwrap_or_default();
            let (skel, parts) = rig_map(state).await.map_err(IntoResponse::into_response)?.creature_by_prefab(&prefab);
            match skel {
                Some(skel) if !skel.is_empty() && !parts.is_empty() => {
                    creature::game_creature(state, &prefab, &skel, parts, fmt).await
                }
                _ => Ok(None),
            }
        }
        Kind::Dress => match &src.outfit {
            Some(o) => dress::model(state, o, fmt).await,
            None => Ok(None),
        },
        Kind::Release | Kind::Tmod => {
            let data = src.tmod.clone().unwrap_or_default();
            let st = state.clone();
            let build = || async move { creature::build_tmod(&st, data).await };
            let sig = if src.kind == Kind::Tmod { None } else { creature::signature(state).await? };
            let sha = src.tmod_sha.clone().filter(|s| !s.is_empty());
            let sig = sig.filter(|_| sha.is_some());
            creature::cached(state, sig.as_deref(), &format!("tmod:{}", sha.unwrap_or_default()), fmt, build).await
        }
    }
}

pub async fn assembled_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, fmt) =
        tri!(begin(&s, &parts, |p| p.query("fmt").str().one_of_pattern(&["json", "bin"]).default("json".into())).await);
    match assembled(&s, &src, fmt_of(&fmt)).await {
        Ok(Some(c)) => cache::respond(&parts.headers, &c, 300),
        Ok(None) => ApiError::not_found("No assemblable creature here.").into_response(),
        Err(r) => r,
    }
}

// --- audio -------------------------------------------------------------------------------------------

async fn bank_index(state: &AppState, src: &Source, path: &str) -> Result<Value, ApiError> {
    if src.kind == Kind::Release
        && let Some(data) = src.tmod.clone()
    {
        return hub::bank_index(state, data, path).await;
    }
    if src.kind == Kind::Tmod {
        return Err(missing(
            "Sounds can't be previewed from an uploaded mod. Publish it to the Mods Hub and embed the release instead.",
        ));
    }
    let no_sounds = || missing("This preview has no sounds to play.");
    if src.kind != Kind::Game {
        return Err(no_sounds());
    }
    let target = src.game_path.clone().unwrap_or_default();
    if !target.to_lowercase().ends_with(".bnk") {
        return Err(no_sounds());
    }
    if !path.is_empty() && pkfx::basename(path).to_lowercase() != pkfx::basename(&target).to_lowercase() {
        return Err(missing("That sound bank isn't part of this preview."));
    }
    let sha = file_sha(state, &target).await?.ok_or_else(|| missing("Game file not found."))?;
    let raw = read_blob(state, &target, &sha).await?.ok_or_else(|| missing("Game file not found."))?;
    let stem: String = target.chars().take(target.chars().count().saturating_sub(4)).collect();
    let side = format!("{stem}.txt");
    let (mut sidecar, mut sidecar_sha) = (None, None);
    if let Some(ssha) = file_sha(state, &side).await?
        && let Some(blob) = read_blob(state, &side, &ssha).await?
    {
        sidecar = Some(String::from_utf8_lossy(&blob).into_owned());
        sidecar_sha = Some(ssha);
    }
    crate::media::audio::manifest(state, raw, &sha, sidecar, sidecar_sha.as_deref()).await
}

pub async fn audio_bank_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, path) = tri!(begin(&s, &parts, |p| p.query("path").str().max_len(400).default(String::new())).await);
    let r = async {
        let payload = bank_index(&s, &src, &path).await?;
        let get = |k: &str| payload.get(k).cloned().unwrap_or(Value::Null);
        let sounds: Vec<Value> = payload
            .get("sounds")
            .and_then(Value::as_array)
            .map(|a| {
                a.iter()
                    .map(|v| match v {
                        Value::Object(m) => Value::Object(
                            m.iter()
                                .filter(|(k, _)| *k != "sha")
                                .map(|(k, v)| (k.clone(), v.clone()))
                                .collect::<Map<_, _>>(),
                        ),
                        other => other.clone(),
                    })
                    .collect()
            })
            .unwrap_or_default();
        Ok(json!({
            "path": path, "bank": get("bank"), "sounds": sounds, "count": get("count"),
            "playable": get("playable"), "total_duration": get("total_duration"),
        }))
    };
    json_reply(MED, r.await)
}

pub async fn audio_sound_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, (path, id, raw)) = tri!(
        begin(&s, &parts, |p| {
            let path = p.query("path").str().max_len(400).default(String::new());
            let id = p.query("id").int().ge(0).le(0xFFFF_FFFFi64).required();
            let raw = p.query("raw").bool().default(false);
            (path, id, raw)
        })
        .await
    );
    let r = async {
        let payload = bank_index(&s, &src, &path).await?;
        hub::sound_from_index(&s, &payload, id, raw).await
    };
    hub::sound_reply(&parts.headers, r.await)
}

// --- VFX ----------------------------------------------------------------------------------------------

async fn vfx_index_and_text(state: &AppState, src: &Source, path: Option<&str>) -> Result<(Index, String), ApiError> {
    if src.kind == Kind::Game {
        let target = src.game_path.clone().unwrap_or_default();
        let raw = match file_sha(state, &target).await? {
            Some(sha) => read_blob(state, &target, &sha).await?,
            None => None,
        };
        let raw = raw.ok_or_else(|| missing("Effect not found in the game files."))?;
        let text = String::from_utf8_lossy(&raw).into_owned();
        return Ok((vec![(pkfx::basename(&target).to_lowercase(), raw)], text));
    }
    let Some(data) = src.tmod.clone() else {
        return Err(missing("This preview has no effects to show."));
    };
    let (_, index) = tokio::task::spawn_blocking(move || hubvfx::pkfx_and_index(&data))
        .await
        .map_err(internal)?
        .map_err(|_| unreadable())?;
    let Some(path) = path else { return Ok((index, String::new())) };
    let raw = hubvfx::index_get(&index, &pkfx::basename(path).to_lowercase())
        .ok_or_else(|| missing("Effect not found in this mod."))?;
    let text = String::from_utf8_lossy(raw).into_owned();
    Ok((index, text))
}

async fn depset(state: &AppState, index: &Index, res: &Resolver) -> Result<HashSet<String>, ApiError> {
    hubvfx::depset(index, |bn| res.lookup(bn), |w| res.read(state, w)).await
}

pub async fn vfx_manifest_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, path) = tri!(begin(&s, &parts, |p| p.query("path").str().min_len(1).max_len(400).required()).await);
    let r = async {
        let (index, text) = vfx_index_and_text(&s, &src, Some(&path)).await?;
        let res = Resolver::new(&s).await?;
        let (deps, missing) = hubvfx::manifest_deps(&index, &text, |bn| res.lookup(bn).is_some());
        let set = depset(&s, &index, &res).await?;
        hubvfx::remember(&src.cache_key(), Arc::new(set));
        Ok(json!({"path": path, "pkfx": text, "deps": deps, "missing": missing, "game_available": res.available()}))
    };
    json_reply(MED, r.await)
}

pub async fn vfx_asset_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    let (src, rf) = tri!(begin(&s, &parts, |p| p.query("path").str().min_len(1).max_len(400).required()).await);
    let r = async {
        let (index, _) = vfx_index_and_text(&s, &src, None).await?;
        let res = Resolver::new(&s).await?;
        let key = src.cache_key();
        let set = match hubvfx::cached(&key) {
            Some(set) => set,
            None => {
                let set = Arc::new(depset(&s, &index, &res).await?);
                hubvfx::remember(&key, set.clone());
                set
            }
        };
        let bn = pkfx::basename(&rf).to_lowercase();
        if !set.contains(&bn) && hubvfx::index_get(&index, &bn).is_none() {
            return Err(missing("Asset not referenced by this effect."));
        }
        let raw = match hubvfx::index_get(&index, &bn) {
            Some(d) => Some(d.clone()),
            None => match res.lookup(&bn) {
                Some(w) => res.read(&s, w).await?,
                None => None,
            },
        };
        let raw = raw.ok_or_else(|| missing("Asset not found (not bundled and not in the game files)."))?;
        Ok((raw, pkfx::media_type_for(&rf)))
    };
    match r.await {
        Ok((data, media_type)) => (
            [
                (header::CONTENT_TYPE, HeaderValue::from_static(media_type)),
                (header::CACHE_CONTROL, HeaderValue::from_static("public, max-age=3600")),
            ],
            data,
        )
            .into_response(),
        Err(e) => e.into_response(),
    }
}

// --- backdrop ---------------------------------------------------------------------------------------

pub async fn backdrop_route(State(s): State<AppState>, req: Request) -> Response {
    let (parts, _) = req.into_parts();
    if let Err(e) = features::check::<Embed>(&s).await {
        return e.into_response();
    }
    tri!(throttle(&s, &parts).await);
    let mut p = Params::new(Vec::new(), parts.uri.query(), &parts.headers);
    let kind = p.query("kind").str().one_of_pattern(&super::backdrop::KINDS).required();
    if let Err(e) = p.v.finish(()) {
        return e.into_response();
    }
    match super::backdrop::backdrop(&s, &kind).await {
        Ok(Some(v)) => json_reply("public, max-age=3600", Ok(v)),
        Ok(None) => ApiError::not_found("No backdrop for this kind.").into_response(),
        Err(r) => r,
    }
}
