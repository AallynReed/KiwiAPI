//! PTS is its own branch, so when it catches up to Live its next capture
//! repeats changes Live already shipped. Those rows get `from_live: true` and
//! the version `from_live_{added,modified,removed}`; nothing is deleted.

use std::collections::HashMap;

use futures::TryStreamExt;
use mongodb::{
    Database,
    bson::{Bson, Document, doc},
};

use super::{
    SyncError,
    repo::{CHANGES, Op, VERSIONS, bulk_write},
};
use crate::auth::pyutil::bson_int;

pub const SOURCE: &str = "live-us";
const CHUNK: usize = 1000;

/// One change row of a path's history, `at` being its version's `captured_at` (ms).
#[derive(Debug, Clone, PartialEq)]
pub struct Hist {
    pub at: i64,
    pub kind: String,
    pub sha: Option<String>,
}

/// Live reached exactly this result before the capture at `cutoff`, and after
/// the branch's own last change to the path (else it is a real revert).
pub fn is_from_live(kind: &str, sha: Option<&str>, cutoff: i64, own_prev: Option<i64>, live: &[Hist]) -> bool {
    let Some(last) = live.iter().filter(|h| h.at < cutoff).max_by_key(|h| h.at) else {
        return false;
    };
    if own_prev.is_some_and(|t| last.at <= t) {
        return false;
    }
    match kind {
        "removed" => last.kind == "removed",
        _ => last.kind != "removed" && last.sha.as_deref() == sha,
    }
}

fn s(d: &Document, k: &str) -> Option<String> {
    d.get_str(k).ok().map(str::to_owned)
}

fn i(d: &Document, k: &str) -> Option<i64> {
    match d.get(k) {
        Some(Bson::Int32(n)) => Some(i64::from(*n)),
        Some(Bson::Int64(n)) => Some(*n),
        _ => None,
    }
}

async fn find(db: &Database, coll: &str, filter: Document, proj: Document) -> Result<Vec<Document>, SyncError> {
    let cur = db.collection::<Document>(coll).find(filter).projection(proj).await.map_err(SyncError::db)?;
    cur.try_collect().await.map_err(SyncError::db)
}

/// `{ordinal: captured_at ms}` of a branch's versions.
async fn captured(db: &Database, branch: &str) -> Result<HashMap<i64, i64>, SyncError> {
    let docs = find(db, VERSIONS, doc! {"branch": branch}, doc! {"ordinal": 1, "captured_at": 1}).await?;
    Ok(docs
        .iter()
        .filter_map(|d| Some((i(d, "ordinal")?, d.get_datetime("captured_at").ok()?.timestamp_millis())))
        .collect())
}

/// Tag one version's rows; returns `(added, modified, removed)` from Live.
async fn tag_version(
    db: &Database,
    branch: &str,
    ordinal: i64,
    own_at: &HashMap<i64, i64>,
    live_at: &HashMap<i64, i64>,
) -> Result<[i64; 3], SyncError> {
    let mut counts = [0; 3];
    let first = own_at.keys().min().copied();
    let Some(&cutoff) = own_at.get(&ordinal).filter(|_| first != Some(ordinal)) else {
        return Ok(counts);
    };
    let proj = doc! {"path": 1, "type": 1, "content_sha256": 1, "ordinal": 1};
    let rows = find(db, CHANGES, doc! {"branch": branch, "ordinal": bson_int(ordinal)}, proj.clone()).await?;
    let mut ops = Vec::new();
    for chunk in rows.chunks(CHUNK) {
        let paths: Vec<String> = chunk.iter().filter_map(|d| s(d, "path")).collect();
        let mut live: HashMap<String, Vec<Hist>> = HashMap::new();
        for d in find(db, CHANGES, doc! {"branch": SOURCE, "path": {"$in": &paths}}, proj.clone()).await? {
            let (Some(p), Some(at)) = (s(&d, "path"), i(&d, "ordinal").and_then(|o| live_at.get(&o))) else {
                continue;
            };
            let h = Hist { at: *at, kind: s(&d, "type").unwrap_or_default(), sha: s(&d, "content_sha256") };
            live.entry(p).or_default().push(h);
        }
        let mut own_prev: HashMap<String, i64> = HashMap::new();
        let filter = doc! {"branch": branch, "path": {"$in": &paths}, "ordinal": {"$lt": bson_int(ordinal)}};
        for d in find(db, CHANGES, filter, proj.clone()).await? {
            if let (Some(p), Some(&at)) = (s(&d, "path"), i(&d, "ordinal").and_then(|o| own_at.get(&o))) {
                let e = own_prev.entry(p).or_insert(at);
                *e = (*e).max(at);
            }
        }
        for d in chunk {
            let (Some(path), Some(kind)) = (s(d, "path"), s(d, "type")) else { continue };
            let sha = s(d, "content_sha256");
            let hist = live.get(&path).map_or(&[][..], Vec::as_slice);
            if !is_from_live(&kind, sha.as_deref(), cutoff, own_prev.get(&path).copied(), hist) {
                continue;
            }
            counts[match kind.as_str() {
                "added" => 0,
                "modified" => 1,
                _ => 2,
            }] += 1;
            ops.push(Op::Update {
                filter: doc! {"branch": branch, "ordinal": bson_int(ordinal), "path": path},
                set: doc! {"from_live": true},
                upsert: false,
            });
        }
    }
    let changes = db.collection::<Document>(CHANGES);
    let all = doc! {"branch": branch, "ordinal": bson_int(ordinal), "from_live": {"$exists": true}};
    changes.update_many(all, doc! {"$unset": {"from_live": ""}}).await.map_err(SyncError::db)?;
    for batch in ops.chunks(CHUNK) {
        bulk_write(db, CHANGES, batch).await?;
    }
    Ok(counts)
}

/// Tag every complete non-Live version that has not been checked yet.
pub async fn tag_pending(db: &Database, branch: &str) -> Result<(), SyncError> {
    if branch == SOURCE {
        return Ok(());
    }
    let filter = doc! {"branch": branch, "status": "complete", "from_live_added": {"$exists": false}};
    let pending = find(db, VERSIONS, filter, doc! {"ordinal": 1}).await?;
    if pending.is_empty() {
        return Ok(());
    }
    let (own_at, live_at) = (captured(db, branch).await?, captured(db, SOURCE).await?);
    for o in pending.iter().filter_map(|d| i(d, "ordinal")) {
        let [a, m, r] = tag_version(db, branch, o, &own_at, &live_at).await?;
        let set = doc! {"from_live_added": a, "from_live_modified": m, "from_live_removed": r};
        db.collection::<Document>(VERSIONS)
            .update_one(doc! {"branch": branch, "ordinal": bson_int(o)}, doc! {"$set": set})
            .await
            .map_err(SyncError::db)?;
        if a + m + r > 0 {
            tracing::info!("updates[{branch}]: ordinal={o} {} change(s) already on {SOURCE}", a + m + r);
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn h(at: i64, kind: &str, sha: Option<&str>) -> Hist {
        Hist { at, kind: kind.into(), sha: sha.map(Into::into) }
    }

    #[test]
    fn catch_up_matches_live() {
        let live = [h(10, "added", Some("a")), h(20, "modified", Some("b"))];
        assert!(is_from_live("modified", Some("b"), 30, Some(5), &live));
        assert!(is_from_live("added", Some("b"), 30, None, &live));
        assert!(!is_from_live("modified", Some("c"), 30, Some(5), &live));
    }

    #[test]
    fn live_after_the_capture_does_not_count() {
        let live = [h(10, "added", Some("a")), h(40, "modified", Some("b"))];
        assert!(!is_from_live("modified", Some("b"), 30, Some(5), &live));
    }

    #[test]
    fn revert_of_a_pts_test_change_is_kept() {
        let live = [h(10, "added", Some("a"))];
        assert!(!is_from_live("modified", Some("a"), 30, Some(20), &live));
    }

    #[test]
    fn removals() {
        let live = [h(10, "added", Some("a")), h(20, "removed", None)];
        assert!(is_from_live("removed", None, 30, Some(5), &live));
        assert!(!is_from_live("removed", None, 30, Some(5), &live[..1]));
        assert!(!is_from_live("removed", None, 30, Some(5), &[]));
    }
}
