//! Possible cheaters and alt clusters (`app/trove/leaderboards/detection.py`):
//! `GET /v1/leaderboards/cheaters`, `GET /site/leaderboards/cheaters`,
//! `POST /v1/leaderboards/views/recompute` and `POST .../cheaters/recompute`.
//! The warmer calls [`warm`]; serving never computes, it reads what the
//! warmer persisted.

pub mod store;
pub mod views;
pub mod wire;

#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

use std::{
    collections::{BTreeSet, HashMap},
    sync::{Arc, LazyLock, Mutex},
};

use axum::{
    Router,
    extract::State,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures::{StreamExt, stream};
use kiwi_core::cheaters::{
    Board, Checks, ClusterParams, CmParams, Detector, Entry, Head, Series, Signals, detect_clusters, empty, fuse,
    history_signals, parse_excluded, reset_boundary_before,
};
use serde_json::{Value, json};

use self::store::{Live, Store, cheaters_key, py_float, py_int, py_str, ready_anchor, retention_days, rget, rset};
use crate::{
    auth::{MasterIngest, Public, scopes::LEADERBOARDS_READ},
    error::ApiError,
    features::{self, Leaderboards, RequireFeature, py_truthy},
    pyjson::{PyJson, PydanticJson},
    state::AppState,
};

pub const WARMER: &str = "leaderboards_warmer";

/// Every knob a pass reads, in Python's order.
#[derive(Clone, Debug)]
pub struct Knobs {
    pub cheater_enabled: bool,
    pub clusters_enabled: bool,
    pub checks: Checks,
    pub cache_ttl: f64,
    pub cluster: ClusterParams,
    pub cluster_excluded: BTreeSet<i64>,
    pub cm: CmParams,
}

impl Knobs {
    /// `_key(anchor)`: everything that changes the answer.
    fn key(&self, anchor: i64) -> String {
        let (c, cl, cm) = (&self.checks, &self.cluster, &self.cm);
        format!(
            "{anchor}|{:?}|{:?}|{}|{:?}|{}|{:?}|{}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}|{:?}",
            c.z_threshold,
            c.velocity_multiplier,
            c.min_board_size,
            c.cohort_pct,
            cl.min_size,
            cl.band_pct,
            cl.max_edit,
            cl.size_full,
            c.excluded,
            self.cluster_excluded,
            self.cheater_enabled,
            self.clusters_enabled,
            cm.candidate_top_n,
            cm.min_hourly_gain,
            cm.gain_percentile,
            cm.gain_tolerance,
            cm.min_matching_hours,
            cm.min_match_ratio,
            cm.min_density,
            cm.min_group_size,
            cm.max_cell_accounts,
            cm.schedule_min_active_hours,
            cm.schedule_min_similarity,
            cm.fusion_corroboration_bonus,
            cm.footprint_min_jaccard,
            cm.weekly_uptime_fraction,
        )
    }

    fn head(&self, anchor: Option<i64>, cheater: bool, clusters: bool, computed_at: f64) -> Head {
        Head {
            anchor,
            z_threshold: self.checks.z_threshold,
            velocity_multiplier: self.checks.velocity_multiplier,
            min_board_size: self.checks.min_board_size,
            cheater_detection_enabled: cheater,
            alt_clusters_enabled: clusters,
            computed_at: computed_at as i64,
        }
    }
}

async fn float<S: Store>(s: &S, key: &str) -> anyhow::Result<f64> {
    let v = s.setting(key).await?;
    py_float(&v).ok_or_else(|| anyhow::anyhow!("{key}: float({v})"))
}

async fn int<S: Store>(s: &S, key: &str) -> anyhow::Result<i64> {
    let v = s.setting(key).await?;
    py_int(&v).ok_or_else(|| anyhow::anyhow!("{key}: int({v})"))
}

/// The settings `detect_possible_cheaters` reads, in its order.
pub async fn knobs<S: Store>(s: &S) -> anyhow::Result<Knobs> {
    let cheater_enabled = py_truthy(&s.setting(features::CHEATER_DETECTION_FLAG).await?);
    let clusters_enabled = py_truthy(&s.setting(features::ALT_CLUSTERS_FLAG).await?);
    let z_threshold = float(s, "cheaters_z_threshold").await?;
    let velocity_multiplier = float(s, "cheaters_velocity_multiplier").await?;
    let min_board_size = int(s, "cheaters_min_board_size").await?;
    let cache_ttl = float(s, "cheaters_cache_ttl_seconds").await?;
    let cohort_pct = float(s, "cheaters_elite_cohort_pct").await?;
    let cluster = ClusterParams {
        min_size: int(s, "cheaters_cluster_min_size").await?,
        band_pct: float(s, "cheaters_cluster_score_band_pct").await?,
        max_edit: int(s, "cheaters_cluster_max_edit_distance").await?,
        size_full: int(s, "cheaters_cluster_size_full").await?,
    };
    let excluded = parse_excluded(&py_str(&s.setting("cheaters_excluded_board_uuids").await?));
    let cluster_excluded = parse_excluded(&py_str(&s.setting("cheaters_cluster_excluded_board_uuids").await?));
    let cm = CmParams {
        candidate_top_n: int(s, "cheaters_comovement_candidate_top_n").await?,
        min_hourly_gain: float(s, "cheaters_comovement_min_hourly_gain").await?,
        gain_percentile: float(s, "cheaters_comovement_gain_percentile").await?,
        gain_tolerance: float(s, "cheaters_comovement_gain_tolerance_pct").await?,
        min_matching_hours: int(s, "cheaters_comovement_min_matching_hours").await?,
        min_match_ratio: float(s, "cheaters_comovement_min_match_ratio").await?,
        min_density: float(s, "cheaters_comovement_min_density").await?,
        min_group_size: int(s, "cheaters_comovement_min_group_size").await?,
        max_cell_accounts: int(s, "cheaters_comovement_max_cell_accounts").await?,
        recompute_seconds: int(s, "cheaters_comovement_recompute_seconds").await?,
        schedule_min_active_hours: int(s, "cheaters_schedule_min_active_hours").await?,
        schedule_min_similarity: float(s, "cheaters_schedule_min_similarity").await?,
        fusion_corroboration_bonus: float(s, "cheaters_fusion_corroboration_bonus").await?,
        footprint_min_jaccard: float(s, "cheaters_footprint_min_jaccard").await?,
        weekly_uptime_fraction: float(s, "cheaters_weekly_uptime_fraction").await?,
    };
    let checks = Checks {
        z_threshold,
        z_repr: crate::pyfloat::repr(z_threshold),
        velocity_multiplier,
        min_board_size,
        cohort_pct,
        excluded,
    };
    Ok(Knobs { cheater_enabled, clusters_enabled, checks, cache_ttl, cluster, cluster_excluded, cm })
}

// ---------------------------------------------------------------------------
// In-process state (`_CACHE`, `_LAST_GOOD`, `_COMOVEMENT_CACHE`)

#[derive(Default)]
struct Caches {
    served: HashMap<String, (f64, Arc<Value>)>,
    last_good: Option<Arc<Value>>,
    comovement: Option<(String, f64, Arc<Signals>)>,
}

static CACHES: LazyLock<Mutex<Caches>> = LazyLock::new(Default::default);

fn caches() -> std::sync::MutexGuard<'static, Caches> {
    CACHES.lock().unwrap_or_else(|e| e.into_inner())
}

/// `invalidate_cache`: drop the served results (a knob or board cadence changed).
pub fn invalidate_cache() {
    caches().served.clear();
}

/// `reset`: drop every in-process result, the last-good one included.
pub fn reset() {
    *caches() = Caches::default();
}

fn remember(key: String, now: f64, payload: &Arc<Value>) {
    let mut c = caches();
    c.served.insert(key, (now, payload.clone()));
    c.last_good = Some(payload.clone());
}

async fn latest<S: Store>(s: &S) -> anyhow::Result<Option<i64>> {
    Ok(s.list_timestamps(1).await?.first().copied())
}

async fn get_cheaters<S: Store>(s: &S, anchor: i64) -> Option<Value> {
    rget(s, &cheaters_key(anchor)).await
}

/// `detect_possible_cheaters(force=...)`: serving (`false`) answers from the
/// published anchor's cached or persisted pass; warming (`true`) computes the
/// latest anchor (or adopts a fresh persisted pass) and persists it.
pub async fn detect<S: Store>(s: &S, force: bool) -> anyhow::Result<Arc<Value>> {
    let k = knobs(s).await?;
    if !k.cheater_enabled && !k.clusters_enabled {
        let anchor = match if force { None } else { ready_anchor(s).await } {
            Some(a) => Some(a),
            None => latest(s).await?,
        };
        return Ok(Arc::new(empty(&k.head(anchor, false, false, s.now()))));
    }
    if !force {
        let serve_anchor = match ready_anchor(s).await {
            Some(a) => Some(a),
            None => latest(s).await?,
        };
        let Some(anchor) = serve_anchor else {
            return Ok(Arc::new(empty(&k.head(None, true, true, s.now()))));
        };
        let key = k.key(anchor);
        let now = s.now();
        {
            let mut c = caches();
            if let Some((at, hit)) = c.served.get(&key).cloned()
                && now - at < k.cache_ttl
            {
                c.last_good = Some(hit.clone());
                return Ok(hit);
            }
        }
        if let Some(persisted) = get_cheaters(s, anchor).await {
            let p = Arc::new(persisted);
            remember(key, now, &p);
            return Ok(p);
        }
        s.trigger_warmer();
        if let Some(good) = caches().last_good.clone() {
            return Ok(good);
        }
        return Ok(Arc::new(empty(&k.head(Some(anchor), true, true, s.now()))));
    }

    let Some(anchor) = latest(s).await? else {
        return Ok(Arc::new(empty(&k.head(None, true, true, s.now()))));
    };
    let key = k.key(anchor);
    let now = s.now();
    if let Some(persisted) = get_cheaters(s, anchor).await {
        let computed_at = match &persisted {
            Value::Object(m) => match m.get("computed_at") {
                None => 0.0,
                Some(v) => v.as_f64().ok_or_else(|| anyhow::anyhow!("persisted computed_at is {v}"))?,
            },
            other => anyhow::bail!("persisted cheaters payload is {other}"),
        };
        if now - computed_at < k.cache_ttl {
            let p = Arc::new(persisted);
            remember(key, now, &p);
            return Ok(p);
        }
    }
    let result = Arc::new(compute(s, anchor, &k).await?);
    remember(key, now, &result);
    let ttl = retention_days(s).await * 86400;
    rset(s, &cheaters_key(anchor), &result, ttl).await;
    caches().served.retain(|_, (t, _)| now - *t <= k.cache_ttl * 2.0);
    Ok(result)
}

/// The warmer's detection pass ([`detect`] with `force`).
pub async fn warm(state: &AppState) -> anyhow::Result<Arc<Value>> {
    detect(&Live(state), true).await
}

/// What the page and profile read ([`detect`] serving).
pub async fn serve(state: &AppState) -> anyhow::Result<Arc<Value>> {
    detect(&Live(state), false).await
}

/// `_compute`: one full pass over `anchor`.
pub async fn compute<S: Store>(s: &S, anchor: i64, k: &Knobs) -> anyhow::Result<Value> {
    let boards: Arc<Vec<Board>> = Arc::new(s.list_boards_at(anchor).await?.into_iter().map(|r| r.board).collect());
    let uuids: Vec<i64> = boards.iter().map(|b| b.uuid).collect();
    let by_board = Arc::new(s.entries_by_board_at(anchor, &uuids).await?);
    let none: Vec<Entry> = Vec::new();
    let mut det = Detector::new();
    if k.cheater_enabled {
        for board in boards.iter() {
            let entries = by_board.get(&board.uuid).unwrap_or(&none);
            let Some(plan) = det.scan(board, entries, &k.checks) else { continue };
            let cycle_start = reset_boundary_before(anchor, &board.reset_kind).max(s.hot_window_start().await);
            let prev = s.previous_captures_bulk(&plan.need, board.uuid, anchor, cycle_start).await?;
            det.velocity(board, entries, anchor, k.checks.velocity_multiplier, &plan, &prev);
        }
    }
    let (clusters, scanned) = if k.clusters_enabled {
        let (b, e, x, p) = (boards.clone(), by_board.clone(), k.cluster_excluded.clone(), k.cluster.clone());
        let (name_clusters, scanned) = tokio::task::spawn_blocking(move || detect_clusters(&b, &e, &x, &p)).await?;
        let signals = comovement(s, anchor, &by_board, &boards, k).await?;
        if k.cheater_enabled {
            det.sustained(&signals.player_flags, &boards, &by_board);
        }
        (fuse(name_clusters, &signals, &k.cm), scanned)
    } else {
        (Vec::new(), 0)
    };
    let head = k.head(Some(anchor), k.cheater_enabled, k.clusters_enabled, s.now());
    Ok(det.finish(&head, clusters, scanned))
}

/// `_comovement_clusters`: the week's history signals, throttled and cached
/// by week start and knobs.
async fn comovement<S: Store>(
    s: &S,
    anchor: i64,
    by_board: &HashMap<i64, Vec<Entry>>,
    boards: &[Board],
    k: &Knobs,
) -> anyhow::Result<Arc<Signals>> {
    let cm = &k.cm;
    if cm.candidate_top_n <= 0 {
        return Ok(Arc::new(Signals::default()));
    }
    let week_start = reset_boundary_before(anchor, "weekly");
    let key = format!(
        "{week_start}|{}|{:?}|{:?}|{:?}|{}|{:?}|{:?}|{}|{}|{}|{:?}|{:?}|{:?}",
        cm.candidate_top_n,
        cm.min_hourly_gain,
        cm.gain_percentile,
        cm.gain_tolerance,
        cm.min_matching_hours,
        cm.min_match_ratio,
        cm.min_density,
        cm.min_group_size,
        cm.max_cell_accounts,
        cm.schedule_min_active_hours,
        cm.schedule_min_similarity,
        cm.weekly_uptime_fraction,
        k.cluster_excluded,
    );
    let now = s.now();
    if let Some((ck, at, sig)) = &caches().comovement
        && *ck == key
        && now - at < cm.recompute_seconds as f64
    {
        return Ok(sig.clone());
    }
    let mut anchors: Vec<i64> =
        s.list_timestamps(400).await?.into_iter().filter(|a| week_start <= *a && *a <= anchor).collect();
    anchors.sort_unstable();
    if anchors.len() as i64 <= cm.min_matching_hours {
        let sig = Arc::new(Signals::default());
        caches().comovement = Some((key, now, sig.clone()));
        return Ok(sig);
    }
    let none: Vec<Entry> = Vec::new();
    let loads: Vec<anyhow::Result<Option<(i64, Board, Series)>>> = stream::iter(0..boards.len())
        .map(|i| {
            let board = &boards[i];
            let entries = by_board.get(&board.uuid).unwrap_or(&none);
            async move {
                if k.cluster_excluded.contains(&board.uuid) || (entries.len() as i64) < cm.min_group_size {
                    return Ok(None);
                }
                let mut ranked: Vec<&Entry> = entries.iter().collect();
                ranked.sort_by_key(|e| e.rank);
                let candidates: Vec<String> =
                    ranked.iter().take(cm.candidate_top_n.max(0) as usize).map(|e| e.player_name.clone()).collect();
                let series = s.comovement_series(board.uuid, &candidates, week_start, anchor).await?;
                Ok((!series.is_empty()).then(|| (board.uuid, board.clone(), series)))
            }
        })
        .buffered(8)
        .collect()
        .await;
    let mut series_by_board = Vec::new();
    let mut meta = HashMap::new();
    for load in loads {
        if let Some((uuid, board, series)) = load? {
            series_by_board.push((uuid, series));
            meta.insert(uuid, board);
        }
    }
    let params = cm.clone();
    let signals = Arc::new(
        tokio::task::spawn_blocking(move || history_signals(&series_by_board, &meta, &anchors, &params)).await?,
    );
    caches().comovement = Some((key, now, signals.clone()));
    Ok(signals)
}

// ---------------------------------------------------------------------------
// Routes

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/v1/leaderboards/cheaters", get(v1_cheaters))
        .route("/site/leaderboards/cheaters", get(site_cheaters))
        .route("/v1/leaderboards/views/recompute", post(views_recompute))
        .route("/v1/leaderboards/cheaters/recompute", post(recompute))
}

fn internal(what: &str, e: impl std::fmt::Display) -> Response {
    tracing::error!("{what}: {e:#}");
    ApiError::internal().into_response()
}

/// `public, max-age={int(cheaters_cache_ttl_seconds)}`.
async fn cache_control<S: Store>(s: &S) -> anyhow::Result<HeaderValue> {
    let ttl = int(s, "cheaters_cache_ttl_seconds").await?;
    Ok(HeaderValue::from_str(&format!("public, max-age={ttl}"))?)
}

async fn v1_cheaters(
    _on: RequireFeature<Leaderboards>,
    Public(ctx): Public<{ LEADERBOARDS_READ }>,
    State(state): State<AppState>,
) -> Response {
    let s = Live(&state);
    let payload = match detect(&s, false).await {
        Ok(p) => p,
        Err(e) => return internal("cheaters", e),
    };
    let cc = match cache_control(&s).await {
        Ok(cc) => cc,
        Err(e) => return internal("cheaters ttl", e),
    };
    match wire::v1_body(&payload) {
        Ok(body) => (ctx.rate_limit, [(header::CACHE_CONTROL, cc)], PydanticJson(body)).into_response(),
        Err(e) => internal("CheatersResponse", e),
    }
}

/// `site_lb_cheaters`: the raw payload, behind the site's leaderboards gate.
async fn site_cheaters(State(state): State<AppState>) -> Response {
    match features::is_enabled(&state, features::LEADERBOARDS_FLAG).await {
        Ok(true) => {}
        Ok(false) => return ApiError::http(StatusCode::NOT_FOUND, None).into_response(),
        Err(e) => return internal("site feature flags", e),
    }
    let s = Live(&state);
    let payload = match detect(&s, false).await {
        Ok(p) => p,
        Err(e) => return internal("cheaters", e),
    };
    match cache_control(&s).await {
        Ok(cc) => ([(header::CACHE_CONTROL, cc)], PyJson((*payload).clone())).into_response(),
        Err(e) => internal("cheaters ttl", e),
    }
}

async fn recompute(
    _on: RequireFeature<Leaderboards>,
    MasterIngest(auth): MasterIngest,
    State(state): State<AppState>,
) -> Response {
    let s = Live(&state);
    let cleared = views::invalidate_cheaters(&s).await;
    reset();
    s.trigger_warmer();
    let body = json!({"recomputing": true, "redis_snapshots_cleared": cleared});
    (StatusCode::ACCEPTED, auth.rate_limit, PydanticJson(body)).into_response()
}

async fn views_recompute(
    _on: RequireFeature<Leaderboards>,
    MasterIngest(auth): MasterIngest,
    State(state): State<AppState>,
) -> Response {
    match views::recompute(&Live(&state)).await {
        Ok(body) => (auth.rate_limit, PydanticJson(body)).into_response(),
        Err(e) => internal("views recompute", e),
    }
}
