//! `app/trove/updates/ingest.py` `sync_branch`: pointer -> manifest -> diff
//! against the sidecar; changed loose files are downloaded whole, touched
//! archive directories are TFI-diffed and only the `archiveN.tfa` holding
//! changed entries is fetched and sliced. Blobs land in the CAS, rows go
//! through the repo. Downloads run `download_concurrency` at a time.

use std::{
    collections::{BTreeMap, BTreeSet},
    future::Future,
    ops::Range,
    pin::Pin,
    sync::{Arc, Mutex},
};

use bytes::Bytes;
use futures::StreamExt;
use kiwi_core::update_archive::{
    self as ua, DirWork, Ordered, Plan, Sidecar, TfiEntry, archive_size, diff_logical, join_logical, parent_dir,
    slice_range,
};

use super::{Counts, Summary, SyncError, cas::ContentStore, cdn::Cdn, repo::Repo};

type Task<'a> = Pin<Box<dyn Future<Output = Result<(), SyncError>> + Send + 'a>>;

/// Run `f` over `items`, at most `limit` at once. Like `asyncio.gather` with a
/// semaphore, a failure does not stop the others; the first one is returned
/// once they are all done.
async fn gather_bounded(limit: usize, tasks: Vec<Task<'_>>) -> Result<(), SyncError> {
    let mut first = None;
    let mut s = futures::stream::iter(tasks).buffer_unordered(limit.max(1));
    while let Some(r) = s.next().await {
        if let Err(e) = r {
            first.get_or_insert(e);
        }
    }
    first.map_or(Ok(()), Err)
}

enum Blob {
    Whole(Bytes),
    Slice(Arc<Vec<u8>>, Range<usize>),
}

impl Blob {
    fn as_slice(&self) -> &[u8] {
        match self {
            Blob::Whole(b) => b,
            Blob::Slice(v, r) => &v[r.clone()],
        }
    }
}

struct Ctx<'a, C, R> {
    branch: &'a str,
    content_path: &'a str,
    ordinal: i64,
    cdn: &'a C,
    store: &'a ContentStore,
    repo: &'a R,
    plan: &'a Plan,
    old_manifest: &'a Ordered<Sidecar>,
    concurrency: usize,
    counts: Mutex<Counts>,
}

impl<C: Cdn, R: Repo> Ctx<'_, C, R> {
    async fn store_blob(
        &self,
        data: Blob,
        change_type: &'static str,
        path: &str,
        fnv: Option<i64>,
        archive_dir: Option<&str>,
        archive_index: Option<i64>,
    ) -> Result<(), SyncError> {
        let store = self.store.clone();
        let (sha, created, len) = tokio::task::spawn_blocking(move || {
            let bytes = data.as_slice();
            store.put(bytes).map(|(sha, created)| (sha, created, bytes.len() as i64))
        })
        .await
        .map_err(|e| SyncError::Io(e.to_string()))?
        .map_err(SyncError::io)?;
        let (b, o) = (self.branch, self.ordinal);
        self.repo.record_change(b, o, path, change_type, Some(&sha), fnv, len).await?;
        self.repo.upsert_state(b, path, &sha, fnv, len, archive_dir, archive_index, o).await?;
        let (total, bytes) = {
            let mut c = self.counts.lock().expect("counts");
            match change_type {
                "added" => c.added += 1,
                _ => c.modified += 1,
            }
            if created {
                c.bytes_added += len;
            }
            (c.added + c.modified, c.bytes_added)
        };
        if total % 5000 == 0 {
            tracing::info!("updates[{b}]: {total} files, {} MiB stored so far…", bytes / (1024 * 1024));
        }
        Ok(())
    }

    async fn record_removed(&self, path: &str) -> Result<(), SyncError> {
        self.repo.record_change(self.branch, self.ordinal, path, "removed", None, None, 0).await?;
        self.repo.remove_state(self.branch, path).await?;
        self.counts.lock().expect("counts").removed += 1;
        Ok(())
    }

    async fn do_loose(&self, path: &str) -> Result<(), SyncError> {
        let e = self.plan.new_map.get(path).expect("changed paths come from the manifest");
        let data = self.cdn.download_file(self.content_path, path, &e.sha1, Some(e.size)).await?;
        let change_type = if self.repo.state_exists(self.branch, path).await? { "modified" } else { "added" };
        self.store_blob(Blob::Whole(data), change_type, path, None, None, None).await?;
        self.repo.set_manifest_entry(self.branch, path, &e.sha1, e.size).await
    }

    async fn sync_directory(&self, work: &DirWork) -> Result<(), SyncError> {
        let (branch, directory) = (self.branch, work.directory.as_str());
        let Some(tfi_path) = &work.tfi_path else {
            for path in self.repo.get_archive_state(branch, directory).await?.keys() {
                self.record_removed(path).await?;
            }
            for p in self.old_manifest.keys().filter(|p| parent_dir(p) == directory) {
                self.repo.remove_manifest_entry(branch, p).await?;
            }
            return Ok(());
        };
        let tfi_e = self.plan.new_map.get(tfi_path).expect("tfi_path comes from the manifest");
        let tfi_bytes = self.cdn.download_file(self.content_path, tfi_path, &tfi_e.sha1, Some(tfi_e.size)).await?;
        let new_full: Ordered<TfiEntry> = ua::parse_tfi(&tfi_bytes)
            .map_err(SyncError::Archive)?
            .into_iter()
            .map(|e| (join_logical(directory, &e.name), e))
            .collect();
        let old_fnv = self.repo.get_archive_state(branch, directory).await?;
        let new_fnv: Ordered<u32> = new_full.iter().map(|(p, e)| (p.clone(), e.fnv_hash)).collect();
        let ld = diff_logical(&old_fnv, &new_fnv);

        let mut changed_by_archive: BTreeMap<i64, Vec<&str>> = BTreeMap::new();
        for path in ld.added.iter().chain(&ld.modified) {
            let ai = new_full.get(path).expect("diffed paths come from the index").archive_index;
            changed_by_archive.entry(i64::from(ai)).or_default().push(path);
        }
        let archives: BTreeSet<i64> =
            work.changed_archives.iter().copied().chain(changed_by_archive.keys().copied()).collect();

        let new_full = &new_full;
        let old_fnv = &old_fnv;
        let changed_by_archive = &changed_by_archive;
        let process = |ai: i64| -> Task<'_> {
            Box::pin(async move {
                let tfa_path = join_logical(directory, &format!("archive{ai}.tfa"));
                let tfa_e = self.plan.new_map.get(&tfa_path);
                let names = changed_by_archive.get(&ai).map_or(&[][..], Vec::as_slice);
                if let (false, Some(tfa_e)) = (names.is_empty(), tfa_e) {
                    let raw =
                        self.cdn.download_file(self.content_path, &tfa_path, &tfa_e.sha1, Some(tfa_e.size)).await?;
                    let hint = archive_size(new_full.values(), ai as u32);
                    let content = tokio::task::spawn_blocking(move || ua::decompress_tfa(&raw, hint))
                        .await
                        .map_err(|e| SyncError::Io(e.to_string()))?
                        .map_err(SyncError::Archive)?;
                    let content = Arc::new(content);
                    for path in names {
                        let e = new_full.get(path).expect("diffed paths come from the index");
                        let change_type = if old_fnv.contains_key(path) { "modified" } else { "added" };
                        let range = slice_range(content.len(), e.offset, e.size);
                        let blob = Blob::Slice(content.clone(), range);
                        let (fnv, idx) = (Some(i64::from(e.fnv_hash)), Some(i64::from(e.archive_index)));
                        self.store_blob(blob, change_type, path, fnv, Some(directory), idx).await?;
                    }
                }
                if let Some(tfa_e) = tfa_e {
                    self.repo.set_manifest_entry(branch, &tfa_path, &tfa_e.sha1, tfa_e.size).await?;
                }
                Ok(())
            })
        };
        gather_bounded(self.concurrency, archives.into_iter().map(process).collect()).await?;

        for path in &ld.removed {
            self.record_removed(path).await?;
        }
        self.repo.set_manifest_entry(branch, tfi_path, &tfi_e.sha1, tfi_e.size).await
    }
}

/// One probe/sync of `branch` (bootstrap pointer `pointer_file`).
pub async fn sync_branch<C: Cdn, R: Repo>(
    branch: &str,
    pointer_file: &str,
    cdn: &C,
    store: &ContentStore,
    repo: &R,
    download_concurrency: usize,
) -> Result<Summary, SyncError> {
    let pointer = cdn.fetch_pointer(pointer_file).await?;
    let content_path = pointer.content_path.as_str();
    let (version, entries) = cdn.fetch_manifest(content_path, &pointer.version).await?;
    let old_manifest = repo.get_manifest_sidecar(branch).await?;
    let plan = ua::classify_manifest_diff(&old_manifest, &entries);
    if !plan.any() {
        repo.touch_probe(branch, content_path, &version).await?;
        return Ok(Summary {
            branch: branch.to_owned(),
            version,
            changed: false,
            ordinal: None,
            counts: Counts::default(),
        });
    }

    let (ordinal, _resumed) = repo.begin_version(branch, &version, &pointer).await?;
    let ctx = Ctx {
        branch,
        content_path,
        ordinal,
        cdn,
        store,
        repo,
        plan: &plan,
        old_manifest: &old_manifest,
        concurrency: download_concurrency,
        counts: Mutex::new(Counts::default()),
    };
    let loose = plan.changed_loose.iter().map(|p| -> Task<'_> { Box::pin(ctx.do_loose(p)) }).collect();
    gather_bounded(download_concurrency, loose).await?;
    for path in &plan.removed_loose {
        if repo.state_exists(branch, path).await? {
            ctx.record_removed(path).await?;
        }
        repo.remove_manifest_entry(branch, path).await?;
    }
    for work in &plan.dir_work {
        ctx.sync_directory(work).await?;
    }
    let counts = ctx.counts.into_inner().expect("counts");
    repo.finish_version(branch, ordinal, &version, &pointer, &counts).await?;
    Ok(Summary { branch: branch.to_owned(), version, changed: true, ordinal: Some(ordinal), counts })
}
