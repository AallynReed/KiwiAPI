//! `app/trove/leaderboards/pg_store.py` (and `pg_schema.ensure_partition`) for
//! the write side: the capture snapshot, the completeness-guard reads, the full
//! reset, the admin board list, the player-aggregate jobs and the cold tier.
//! The SQL is Python's, verbatim. Errors are the message `str(exc)` gives for
//! the asyncpg exception.

use std::collections::HashMap;

use bytes::{BufMut, Bytes, BytesMut};
use deadpool_postgres::Pool;
use futures::{SinkExt, StreamExt, pin_mut};
use kiwi_core::{agg::Agg, lbparse::Board};
use tokio_postgres::{
    CopyInSink, CopyOutStream, GenericClient,
    types::{ToSql, Type},
};

use kiwi_core::lbmodels::effective_reset_kind;

pub type PgResult<T> = Result<T, String>;

/// `str(exc)` of what asyncpg raises.
pub fn pg_err(e: tokio_postgres::Error) -> String {
    match e.as_db_error() {
        Some(db) => db.message().to_owned(),
        None => e.to_string(),
    }
}

/// `acquire()` without a pool.
pub const NO_POOL: &str = "Postgres pool not initialized (POSTGRES_DSN unset?)";

pub fn pool(state: &crate::state::AppState) -> PgResult<&Pool> {
    state.inner.db.pg.as_ref().ok_or_else(|| NO_POOL.to_owned())
}

/// A Python int bound as the type Postgres inferred for the parameter.
fn int_arg(ty: &Type, i: usize, v: i64) -> PgResult<Box<dyn ToSql + Sync + Send>> {
    let range = |_| format!("invalid input for query argument ${}: {v} (value out of int32 range)", i + 1);
    Ok(match *ty {
        Type::INT2 => Box::new(
            i16::try_from(v)
                .map_err(|_| format!("invalid input for query argument ${}: {v} (value out of int16 range)", i + 1))?,
        ),
        Type::INT4 => Box::new(i32::try_from(v).map_err(range)?),
        Type::FLOAT8 => Box::new(v as f64),
        _ => Box::new(v),
    })
}

async fn ints<C: GenericClient>(
    c: &C,
    sql: &str,
    args: &[i64],
) -> PgResult<(tokio_postgres::Statement, Vec<Box<dyn ToSql + Sync + Send>>)> {
    let stmt = c.prepare(sql).await.map_err(pg_err)?;
    let params = stmt
        .params()
        .iter()
        .zip(args)
        .enumerate()
        .map(|(i, (ty, &v))| int_arg(ty, i, v))
        .collect::<PgResult<Vec<_>>>()?;
    Ok((stmt, params))
}

fn refs(params: &[Box<dyn ToSql + Sync + Send>]) -> Vec<&(dyn ToSql + Sync)> {
    params.iter().map(|p| p.as_ref() as &(dyn ToSql + Sync)).collect()
}

/// `con.execute(sql, *ints)` -> rows affected.
pub async fn exec<C: GenericClient>(c: &C, sql: &str, args: &[i64]) -> PgResult<u64> {
    let (stmt, params) = ints(c, sql, args).await?;
    c.execute(&stmt, &refs(&params)).await.map_err(pg_err)
}

/// `con.fetch(sql, *ints)`.
pub async fn fetch<C: GenericClient>(c: &C, sql: &str, args: &[i64]) -> PgResult<Vec<tokio_postgres::Row>> {
    let (stmt, params) = ints(c, sql, args).await?;
    c.query(&stmt, &refs(&params)).await.map_err(pg_err)
}

/// `con.fetchval(sql, *ints)` of a nullable bigint.
pub async fn fetch_i64<C: GenericClient>(c: &C, sql: &str, args: &[i64]) -> PgResult<Option<i64>> {
    let rows = fetch(c, sql, args).await?;
    let Some(row) = rows.first() else { return Ok(None) };
    let ty = row.columns()[0].type_().clone();
    Ok(match ty {
        Type::INT4 => row.try_get::<_, Option<i32>>(0).map_err(pg_err)?.map(i64::from),
        Type::INT2 => row.try_get::<_, Option<i16>>(0).map_err(pg_err)?.map(i64::from),
        Type::BOOL => row.try_get::<_, Option<bool>>(0).map_err(pg_err)?.map(i64::from),
        _ => row.try_get::<_, Option<i64>>(0).map_err(pg_err)?,
    })
}

// --- binary COPY ---------------------------------------------------------------

const COPY_HEADER: &[u8] = b"PGCOPY\n\xff\r\n\0\0\0\0\0\0\0\0\0";
const CHUNK: usize = 1 << 20;

/// A binary COPY body, sent in ~1 MiB chunks.
pub struct CopyBuf {
    buf: BytesMut,
}

impl CopyBuf {
    fn new() -> Self {
        let mut buf = BytesMut::with_capacity(CHUNK + 4096);
        buf.put_slice(COPY_HEADER);
        CopyBuf { buf }
    }
    fn row(&mut self, fields: i16) {
        self.buf.put_i16(fields);
    }
    fn i32(&mut self, v: i32) {
        self.buf.put_i32(4);
        self.buf.put_i32(v);
    }
    fn i64(&mut self, v: i64) {
        self.buf.put_i32(8);
        self.buf.put_i64(v);
    }
    fn f64(&mut self, v: f64) {
        self.buf.put_i32(8);
        self.buf.put_u64(v.to_bits());
    }
    fn text(&mut self, v: &str) {
        self.buf.put_i32(v.len() as i32);
        self.buf.put_slice(v.as_bytes());
    }
    fn opt_i64(&mut self, v: Option<i64>) {
        match v {
            Some(v) => self.i64(v),
            None => self.buf.put_i32(-1),
        }
    }
    fn full(&self) -> bool {
        self.buf.len() >= CHUNK
    }
    fn take(&mut self) -> Bytes {
        self.buf.split().freeze()
    }
}

/// Feeds a `COPY ... FROM STDIN BINARY` the rows `fill` writes; `fill`
/// returns false once it has nothing more.
async fn copy_in<F>(sink: CopyInSink<Bytes>, mut fill: F) -> PgResult<u64>
where
    F: FnMut(&mut CopyBuf) -> PgResult<bool>,
{
    pin_mut!(sink);
    let mut buf = CopyBuf::new();
    loop {
        let more = fill(&mut buf)?;
        if !more {
            buf.buf.put_i16(-1);
        }
        if buf.full() || !more {
            sink.send(buf.take()).await.map_err(pg_err)?;
        }
        if !more {
            break;
        }
    }
    sink.finish().await.map_err(pg_err)
}

async fn read_copy(stream: CopyOutStream) -> PgResult<Vec<u8>> {
    pin_mut!(stream);
    let mut out = Vec::new();
    while let Some(chunk) = stream.next().await {
        out.extend_from_slice(&chunk.map_err(pg_err)?);
    }
    Ok(out)
}

fn int4(v: i64, what: &str) -> PgResult<i32> {
    i32::try_from(v).map_err(|_| format!("invalid input for {what}: {v} (value out of int32 range)"))
}

/// A COPY record field: asyncpg raises the codec's own `OverflowError`.
fn copy_int4(v: i64) -> PgResult<i32> {
    i32::try_from(v).map_err(|_| "value out of int32 range".to_owned())
}

// --- write_snapshot --------------------------------------------------------------

/// `write_snapshot`'s summary (`cleared_before_insert` from the DELETE tag).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Written {
    pub boards: usize,
    pub entries: usize,
    pub cleared: u64,
}

const SNAPSHOT_TIMEOUT: &str = "SET LOCAL statement_timeout = 120000";

/// `pg_schema.ensure_partition`.
pub fn partition_ddl(anchor: i64) -> String {
    let lo = kiwi_core::lbparse::trove_day_start(anchor);
    format!("CREATE TABLE IF NOT EXISTS entry_p_{lo} PARTITION OF entry FOR VALUES FROM ({lo}) TO ({})", lo + 86400)
}

const BOARD_UPSERT: &str = "INSERT INTO board (uuid, name_id, name, category_id, category) \
    VALUES ($1, $2, $3, $4, $5) \
    ON CONFLICT (uuid) DO UPDATE SET \
    name_id = EXCLUDED.name_id, name = EXCLUDED.name, \
    category_id = EXCLUDED.category_id, category = EXCLUDED.category";
const CONTEST_INSERT: &str =
    "INSERT INTO board_contest (board_uuid, anchor, type) VALUES ($1, $2, $3) ON CONFLICT DO NOTHING";
const STAGING: &str =
    "CREATE TEMP TABLE _stg (board_uuid int, anchor bigint, rank int, score float8, name text) ON COMMIT DROP";
const STAGING_COPY: &str = "COPY _stg (board_uuid, anchor, rank, score, name) FROM STDIN BINARY";
const PLAYER_UPSERT: &str = "INSERT INTO player (name, name_lower) \
    SELECT min(name COLLATE \"C\"), lower(name) COLLATE \"C\" FROM _stg \
    GROUP BY lower(name) COLLATE \"C\" \
    ON CONFLICT (name_lower) DO UPDATE SET name = EXCLUDED.name \
    WHERE player.name IS DISTINCT FROM EXCLUDED.name";
const ENTRY_INSERT: &str = "INSERT INTO entry (board_uuid, anchor, rank, score, player_id) \
    SELECT s.board_uuid, s.anchor, s.rank, s.score, p.id \
    FROM _stg s JOIN player p ON p.name_lower = lower(s.name)";
const CASE_COLLISIONS: &str = "INSERT INTO player_duplicate (name_lower, name, kind, verdict, boards, \
    max_occurrences, spellings, first_anchor, last_anchor, evidence, \
    method_version, updated_at) \
    SELECT lower(s.name), max(s.name), 'case', 'case_only', \
           count(DISTINCT s.board_uuid)::int, count(DISTINCT s.name)::int, \
           to_jsonb(array_agg(DISTINCT s.name)), $1, $1, '{}'::jsonb, 1, $1 \
    FROM _stg s WHERE lower(s.name) COLLATE \"C\" IN (\
      SELECT lower(name) COLLATE \"C\" FROM _stg GROUP BY 1 \
      HAVING min(name COLLATE \"C\") <> max(name COLLATE \"C\")) \
    GROUP BY lower(s.name) HAVING count(DISTINCT s.name) > 1 \
    ON CONFLICT (name_lower) DO UPDATE SET \
      name = EXCLUDED.name, \
      kind = CASE WHEN player_duplicate.kind IN ('same_name', 'both') \
                  THEN 'both' ELSE 'case' END, \
      verdict = CASE WHEN player_duplicate.kind IN ('same_name', 'both') \
                     THEN player_duplicate.verdict ELSE EXCLUDED.verdict END, \
      boards = CASE WHEN player_duplicate.kind IN ('same_name', 'both') \
                    THEN player_duplicate.boards ELSE EXCLUDED.boards END, \
      max_occurrences = GREATEST(player_duplicate.max_occurrences, \
                                 EXCLUDED.max_occurrences), \
      spellings = EXCLUDED.spellings, \
      first_anchor = LEAST(player_duplicate.first_anchor, EXCLUDED.first_anchor), \
      last_anchor = GREATEST(player_duplicate.last_anchor, EXCLUDED.last_anchor), \
      updated_at = EXCLUDED.updated_at";

/// `_LAST_RISE_RULE`.
const LAST_RISE_RULE: &str = "CASE WHEN EXCLUDED.last_seen > a.last_seen AND EXCLUDED.latest_score > a.latest_score \
    THEN EXCLUDED.last_seen ELSE a.last_rise END";

/// `_fold_anchor_into_agg`'s statement for `table`.
pub fn fold_sql(table: &str) -> String {
    format!(
        "INSERT INTO {table} AS a \
           (player_id, board_uuid, best_rank, appearances, first_seen, last_seen, \
            latest_rank, latest_score, last_folded_anchor) \
         SELECT DISTINCT ON (e.player_id, e.board_uuid) \
                e.player_id, e.board_uuid, e.rank, 1, e.anchor, e.anchor, \
                e.rank, e.score, e.anchor \
         FROM entry e WHERE e.anchor = $1 \
         ORDER BY e.player_id, e.board_uuid, e.rank ASC, e.score DESC \
         ON CONFLICT (player_id, board_uuid) DO UPDATE SET \
           best_rank    = LEAST(a.best_rank, EXCLUDED.best_rank), \
           first_seen   = LEAST(a.first_seen, EXCLUDED.first_seen), \
           last_seen    = GREATEST(a.last_seen, EXCLUDED.last_seen), \
           appearances  = a.appearances + \
                          (CASE WHEN EXCLUDED.last_folded_anchor > a.last_folded_anchor \
                                THEN 1 ELSE 0 END), \
           latest_rank  = CASE WHEN EXCLUDED.last_seen >= a.last_seen \
                               THEN EXCLUDED.latest_rank ELSE a.latest_rank END, \
           latest_score = CASE WHEN EXCLUDED.last_seen >= a.last_seen \
                               THEN EXCLUDED.latest_score ELSE a.latest_score END, \
           last_rise    = {LAST_RISE_RULE}, \
           last_folded_anchor = GREATEST(a.last_folded_anchor, EXCLUDED.last_folded_anchor)"
    )
}

/// `write_snapshot`: one transaction, idempotent on the anchor.
pub async fn write_snapshot(pool: &Pool, boards: &[Board<'_>], anchor: i64) -> PgResult<Written> {
    let entries: usize = boards.iter().map(|b| b.entries.len()).sum();
    let mut con = pool.get().await.map_err(|e| e.to_string())?;
    let tx = con.transaction().await.map_err(pg_err)?;
    tx.batch_execute(SNAPSHOT_TIMEOUT).await.map_err(pg_err)?;
    tx.batch_execute(&partition_ddl(anchor)).await.map_err(pg_err)?;
    let cleared = exec(&*tx, "DELETE FROM entry WHERE anchor = $1", &[anchor]).await?;

    // executemany: one statement, every row pipelined in order.
    if !boards.is_empty() {
        let stmt = tx.prepare(BOARD_UPSERT).await.map_err(pg_err)?;
        let uuids = boards
            .iter()
            .enumerate()
            .map(|(i, b)| int4(b.uuid, &format!("query argument $1 in element #{i} of executemany() sequence")))
            .collect::<PgResult<Vec<i32>>>()?;
        let mut pending = Vec::with_capacity(boards.len());
        for (b, uuid) in boards.iter().zip(&uuids) {
            let row: [&(dyn ToSql + Sync); 5] = [uuid, &b.name_id, &b.name, &b.category_id, &b.category];
            pending.push(tx.execute_raw(&stmt, row));
        }
        futures::future::try_join_all(pending).await.map_err(pg_err)?;
    }
    let contests: Vec<(i32, &str)> = boards.iter().filter_map(|b| Some((b.uuid as i32, b.contest?))).collect();
    if !contests.is_empty() {
        let stmt = tx.prepare(CONTEST_INSERT).await.map_err(pg_err)?;
        let a = int_arg(&stmt.params()[1], 1, anchor)?;
        let mut pending = Vec::with_capacity(contests.len());
        for (uuid, kind) in &contests {
            let row: [&(dyn ToSql + Sync); 3] = [uuid, a.as_ref(), kind];
            pending.push(tx.execute_raw(&stmt, row));
        }
        futures::future::try_join_all(pending).await.map_err(pg_err)?;
    }

    tx.batch_execute(STAGING).await.map_err(pg_err)?;
    let (mut bi, mut ei) = (0, 0);
    copy_in(tx.copy_in(STAGING_COPY).await.map_err(pg_err)?, |buf| {
        while let Some(b) = boards.get(bi) {
            let Some(&(rank, name, score)) = b.entries.get(ei) else {
                (bi, ei) = (bi + 1, 0);
                continue;
            };
            ei += 1;
            buf.row(5);
            buf.i32(copy_int4(b.uuid)?);
            buf.i64(anchor);
            buf.i32(copy_int4(rank)?);
            buf.f64(score);
            buf.text(name);
            if buf.full() {
                return Ok(true);
            }
        }
        Ok(false)
    })
    .await?;
    tx.batch_execute("ANALYZE _stg").await.map_err(pg_err)?;
    tx.batch_execute(PLAYER_UPSERT).await.map_err(pg_err)?;
    tx.batch_execute(ENTRY_INSERT).await.map_err(pg_err)?;
    exec(&*tx, CASE_COLLISIONS, &[anchor]).await?;
    exec(&*tx, &fold_sql("player_board_agg"), &[anchor]).await?;
    tx.commit().await.map_err(pg_err)?;
    Ok(Written { boards: boards.len(), entries, cleared })
}

// --- completeness-guard reads ------------------------------------------------------

/// `previous_anchor_before`.
pub async fn previous_anchor_before(pool: &Pool, anchor: i64) -> PgResult<Option<i64>> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    fetch_i64(&**con, "SELECT MAX(anchor) FROM entry WHERE anchor < $1", &[anchor]).await
}

/// `board_counts_at`.
pub async fn board_counts_at(pool: &Pool, anchor: i64) -> PgResult<HashMap<i64, i64>> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let rows =
        fetch(&**con, "SELECT board_uuid, count(*) AS n FROM entry WHERE anchor = $1 GROUP BY board_uuid", &[anchor])
            .await?;
    rows.iter()
        .map(|r| Ok((i64::from(r.try_get::<_, i32>(0).map_err(pg_err)?), r.try_get::<_, i64>(1).map_err(pg_err)?)))
        .collect()
}

/// `board_kinds`.
pub async fn board_kinds(pool: &Pool, uuids: &[i64]) -> PgResult<HashMap<i64, String>> {
    if uuids.is_empty() {
        return Ok(HashMap::new());
    }
    let ids: Vec<i32> = uuids.iter().map(|&u| int4(u, "query argument $1")).collect::<PgResult<_>>()?;
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let rows =
        con.query("SELECT uuid, reset_kind_override FROM board WHERE uuid = ANY($1)", &[&ids]).await.map_err(pg_err)?;
    rows.iter()
        .map(|r| {
            let uuid = i64::from(r.try_get::<_, i32>(0).map_err(pg_err)?);
            let ov: Option<String> = r.try_get(1).map_err(pg_err)?;
            Ok((uuid, effective_reset_kind(ov.as_deref(), uuid).to_owned()))
        })
        .collect()
}

// --- reset ---------------------------------------------------------------------------

/// `reset_all`'s Postgres tallies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResetTally {
    pub entries: i64,
    pub estimates: i64,
    pub boards: i64,
}

pub async fn reset_all(pool: &Pool, drop_boards: bool) -> PgResult<ResetTally> {
    let mut con = pool.get().await.map_err(|e| e.to_string())?;
    let entries = fetch_i64(
        &**con,
        "SELECT COALESCE(SUM(c.reltuples)::bigint, 0) FROM pg_inherits i \
         JOIN pg_class c ON c.oid = i.inhrelid WHERE i.inhparent = 'entry'::regclass",
        &[],
    )
    .await?
    .unwrap_or(0);
    let estimates = fetch_i64(&**con, "SELECT count(*) FROM activity_estimate", &[]).await?.unwrap_or(0);
    let tx = con.transaction().await.map_err(pg_err)?;
    tx.batch_execute(
        "TRUNCATE entry, player, activity_estimate, activity_active, \
         class_activity_estimate, class_activity_active, player_rename, \
         player_duplicate, \
         player_board_agg RESTART IDENTITY",
    )
    .await
    .map_err(pg_err)?;
    let mut boards = 0;
    if drop_boards {
        boards = fetch_i64(&*tx, "SELECT count(*) FROM board", &[]).await?.unwrap_or(0);
        tx.batch_execute("TRUNCATE board, board_contest").await.map_err(pg_err)?;
    }
    tx.commit().await.map_err(pg_err)?;
    Ok(ResetTally { entries, estimates, boards })
}

// --- admin board cadence -------------------------------------------------------------

/// A `board` row as the admin endpoints read it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BoardRow {
    pub uuid: i64,
    pub name: String,
    pub name_id: String,
    pub category: String,
    pub category_id: String,
    pub reset_kind_override: Option<String>,
}

fn board_row(r: &tokio_postgres::Row) -> PgResult<BoardRow> {
    Ok(BoardRow {
        uuid: i64::from(r.try_get::<_, i32>(0).map_err(pg_err)?),
        name: r.try_get(1).map_err(pg_err)?,
        name_id: r.try_get(2).map_err(pg_err)?,
        category: r.try_get(3).map_err(pg_err)?,
        category_id: r.try_get(4).map_err(pg_err)?,
        reset_kind_override: r.try_get(5).map_err(pg_err)?,
    })
}

/// `admin_list_boards`.
pub async fn admin_list_boards(pool: &Pool) -> PgResult<Vec<BoardRow>> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let rows = con
        .query("SELECT uuid, name, name_id, category, category_id, reset_kind_override FROM board", &[])
        .await
        .map_err(pg_err)?;
    rows.iter().map(board_row).collect()
}

/// `set_reset_kind_override`.
pub async fn set_reset_kind_override(pool: &Pool, uuid: i64, value: Option<&str>) -> PgResult<Option<BoardRow>> {
    let id = i32::try_from(uuid)
        .map_err(|_| format!("invalid input for query argument $1: {uuid} (value out of int32 range)"))?;
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let row = con
        .query_opt(
            "UPDATE board SET reset_kind_override = $2 WHERE uuid = $1 \
             RETURNING uuid, name, name_id, category, category_id, reset_kind_override",
            &[&id, &value],
        )
        .await
        .map_err(pg_err)?;
    row.as_ref().map(board_row).transpose()
}

// --- player_board_agg jobs -------------------------------------------------------------

/// `_AGG_JOB_LOCK` ("LAST").
pub const AGG_JOB_LOCK: i64 = 0x4C41_5354;
const AGG_STAGING: &str = "player_board_agg_rebuild";
const COMMAND_TIMEOUT_MS: i64 = 120_000;

/// One dedicated session for a whole-history job: each statement gets the
/// timeout asyncpg would give it (120s, or the call's own 600/1800s).
pub struct JobSession {
    client: deadpool_postgres::ClientWrapper,
    timeout_ms: i64,
}

impl JobSession {
    async fn open(pool: &Pool) -> PgResult<JobSession> {
        let obj = pool.get().await.map_err(|e| e.to_string())?;
        // Detached from the pool: its session settings and locks die with it.
        Ok(JobSession { client: deadpool_postgres::Object::take(obj), timeout_ms: 0 })
    }

    async fn timeout(&mut self, ms: i64) -> PgResult<()> {
        if self.timeout_ms != ms {
            self.client.batch_execute(&format!("SET statement_timeout = {ms}")).await.map_err(pg_err)?;
            self.timeout_ms = ms;
        }
        Ok(())
    }

    /// One statement; the row count only matters (and is only read) with args
    /// or for the final UPDATE, which goes through `exec`.
    async fn run(&mut self, sql: &str, args: &[i64], ms: i64) -> PgResult<u64> {
        self.timeout(ms).await?;
        if args.is_empty() && !sql.starts_with("UPDATE") {
            return self.client.batch_execute(sql).await.map(|()| 0).map_err(pg_err);
        }
        exec(&*self.client, sql, args).await
    }

    async fn val(&mut self, sql: &str, args: &[i64]) -> PgResult<Option<i64>> {
        self.timeout(COMMAND_TIMEOUT_MS).await?;
        fetch_i64(&*self.client, sql, args).await
    }

    /// `_anchors_after`'s probe.
    async fn next_anchor(&mut self, after: i64) -> PgResult<Option<i64>> {
        self.val("SELECT min(anchor) FROM entry WHERE anchor > $1", &[after]).await
    }

    async fn capture(&mut self, anchor: i64) -> PgResult<Vec<u8>> {
        self.timeout(600_000).await?;
        let sql = format!(
            "COPY (SELECT board_uuid, player_id, rank, score FROM entry WHERE anchor = {anchor}) \
             TO STDOUT (FORMAT binary)"
        );
        read_copy(self.client.copy_out(&sql).await.map_err(pg_err)?).await
    }

    /// Every capture folded in memory, oldest first (the native replay).
    async fn replay(&mut self, progress: &(dyn Fn(u64, i64, i64) + Sync)) -> PgResult<(Agg, Option<i64>)> {
        let mut agg = Agg::default();
        let (mut first, mut last, mut done) = (None, None, 0u64);
        let mut next = self.next_anchor(-1).await?;
        while let Some(anchor) = next {
            let first = *first.get_or_insert(anchor);
            let data = self.capture(anchor).await?;
            if agg.fold_copy(anchor, &data).is_none() {
                return Err(format!("capture {anchor} could not be read"));
            }
            last = Some(anchor);
            done += 1;
            progress(done, first, anchor);
            next = self.next_anchor(anchor).await?;
        }
        Ok((agg, last))
    }
}

/// `_agg_job`: refuses to run beside another aggregate job in any process.
async fn agg_job<F, Fut>(pool: &Pool, job: F) -> PgResult<i64>
where
    F: FnOnce(JobSession) -> Fut,
    Fut: Future<Output = (JobSession, PgResult<i64>)>,
{
    let mut s = JobSession::open(pool).await?;
    let got = s.val("SELECT pg_try_advisory_lock($1)::int", &[AGG_JOB_LOCK]).await?;
    if got != Some(1) {
        return Err("Another player-aggregate job is already running.".into());
    }
    let (mut s, res) = job(s).await;
    let _ = s.run("SELECT pg_advisory_unlock($1)", &[AGG_JOB_LOCK], COMMAND_TIMEOUT_MS).await;
    res
}

/// `rebuild_player_board_agg` (the kiwi_native replay, then a short swap).
pub async fn rebuild_player_board_agg(pool: &Pool, progress: &(dyn Fn(u64, i64, i64) + Sync)) -> PgResult<i64> {
    agg_job(pool, |mut s| async move {
        let res = rebuild(&mut s, progress).await;
        (s, res)
    })
    .await
}

async fn rebuild(s: &mut JobSession, progress: &(dyn Fn(u64, i64, i64) + Sync)) -> PgResult<i64> {
    s.run(&format!("DROP TABLE IF EXISTS {AGG_STAGING}"), &[], COMMAND_TIMEOUT_MS).await?;
    let swapped = async {
        let (agg, last) = s.replay(progress).await?;
        s.run(&format!("CREATE TABLE {AGG_STAGING} (LIKE player_board_agg INCLUDING ALL)"), &[], COMMAND_TIMEOUT_MS)
            .await?;
        s.timeout(1_800_000).await?;
        let mut rows = agg.rows.iter();
        let sql = format!(
            "COPY {AGG_STAGING} (player_id, board_uuid, best_rank, appearances, first_seen, last_seen, \
             latest_rank, latest_score, last_folded_anchor, last_rise) FROM STDIN BINARY"
        );
        copy_in(s.client.copy_in(&sql).await.map_err(pg_err)?, |buf| {
            for (&(player, board), r) in rows.by_ref() {
                buf.row(10);
                buf.i64(player);
                buf.i32(board);
                buf.i32(r.best_rank);
                buf.i64(r.appearances);
                buf.i64(r.first_seen);
                buf.i64(r.last_seen);
                buf.i32(r.latest_rank);
                buf.f64(r.latest_score);
                buf.i64(r.last_folded);
                buf.opt_i64(r.last_rise);
                if buf.full() {
                    return Ok(true);
                }
            }
            Ok(false)
        })
        .await?;
        drop(agg);
        s.run("BEGIN", &[], COMMAND_TIMEOUT_MS).await?;
        let swap = async {
            s.run("LOCK TABLE player_board_agg IN ACCESS EXCLUSIVE MODE", &[], 600_000).await?;
            let fold = fold_sql(AGG_STAGING);
            let mut next = s.next_anchor(last.unwrap_or(-1)).await?;
            while let Some(anchor) = next {
                s.run(&fold, &[anchor], 600_000).await?;
                next = s.next_anchor(anchor).await?;
            }
            s.run("DROP TABLE player_board_agg", &[], COMMAND_TIMEOUT_MS).await?;
            s.run(&format!("ALTER TABLE {AGG_STAGING} RENAME TO player_board_agg"), &[], COMMAND_TIMEOUT_MS).await?;
            s.run(&format!("ALTER INDEX {AGG_STAGING}_pkey RENAME TO player_board_agg_pkey"), &[], COMMAND_TIMEOUT_MS)
                .await?;
            Ok::<(), String>(())
        }
        .await;
        match swap {
            Ok(()) => s.run("COMMIT", &[], COMMAND_TIMEOUT_MS).await.map(|_| ()),
            Err(e) => {
                let _ = s.client.batch_execute("ROLLBACK").await;
                // The rollback also undid any SET inside the transaction.
                s.timeout_ms = 0;
                Err(e)
            }
        }
    }
    .await;
    let dropped = s.run(&format!("DROP TABLE IF EXISTS {AGG_STAGING}"), &[], COMMAND_TIMEOUT_MS).await;
    swapped?;
    dropped?;
    s.run("ANALYZE player_board_agg", &[], COMMAND_TIMEOUT_MS).await?;
    Ok(s.val("SELECT count(*) FROM player_board_agg", &[]).await?.unwrap_or(0))
}

/// `backfill_last_rise` (the kiwi_native replay into a scratch table, merged
/// with GREATEST): the number of live rows whose `last_rise` changed.
pub async fn backfill_last_rise(pool: &Pool, progress: &(dyn Fn(u64, i64, i64) + Sync)) -> PgResult<i64> {
    agg_job(pool, |mut s| async move {
        let res = last_rise(&mut s, progress).await;
        (s, res)
    })
    .await
}

async fn last_rise(s: &mut JobSession, progress: &(dyn Fn(u64, i64, i64) + Sync)) -> PgResult<i64> {
    s.run("DROP TABLE IF EXISTS player_last_rise_scratch", &[], COMMAND_TIMEOUT_MS).await?;
    s.run(
        "CREATE UNLOGGED TABLE player_last_rise_scratch (\
           player_id BIGINT NOT NULL, board_uuid INTEGER NOT NULL,\
           last_seen BIGINT NOT NULL, latest_score DOUBLE PRECISION NOT NULL,\
           last_rise BIGINT, PRIMARY KEY (player_id, board_uuid)\
         ) WITH (fillfactor = 50)",
        &[],
        COMMAND_TIMEOUT_MS,
    )
    .await?;
    let merged = async {
        let (agg, _) = s.replay(progress).await?;
        s.timeout(1_800_000).await?;
        let mut rows = agg.rows.iter();
        let sink = s
            .client
            .copy_in("COPY player_last_rise_scratch (player_id, board_uuid, last_seen, latest_score, last_rise) FROM STDIN BINARY")
            .await
            .map_err(pg_err)?;
        copy_in(
            sink,
            |buf| {
                for (&(player, board), r) in rows.by_ref() {
                    buf.row(5);
                    buf.i64(player);
                    buf.i32(board);
                    buf.i64(r.last_seen);
                    buf.f64(r.latest_score);
                    buf.opt_i64(r.last_rise);
                    if buf.full() {
                        return Ok(true);
                    }
                }
                Ok(false)
            },
        )
        .await?;
        drop(agg);
        s.run(
            "UPDATE player_board_agg a SET last_rise = s.last_rise \
             FROM player_last_rise_scratch s \
             WHERE s.player_id = a.player_id AND s.board_uuid = a.board_uuid \
               AND s.last_rise IS NOT NULL \
               AND (a.last_rise IS NULL OR s.last_rise > a.last_rise)",
            &[],
            1_800_000,
        )
        .await
    }
    .await;
    let dropped = s.run("DROP TABLE IF EXISTS player_last_rise_scratch", &[], COMMAND_TIMEOUT_MS).await;
    let n = merged?;
    dropped?;
    Ok(n as i64)
}

// --- cold tier ---------------------------------------------------------------------

pub const COLD_TABLESPACE: &str = "cold";

/// An `entry` partition (`_entry_partitions`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Partition {
    pub oid: u32,
    pub name: String,
    pub lo: i64,
    pub tablespace: String,
    pub bytes: i64,
}

pub async fn cold_tablespace_exists(pool: &Pool) -> PgResult<bool> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let row =
        con.query_opt("SELECT 1 FROM pg_tablespace WHERE spcname = $1", &[&COLD_TABLESPACE]).await.map_err(pg_err)?;
    Ok(row.is_some())
}

/// `_partition_lo`: `int()` of what follows `entry_p_`.
pub fn partition_lo(relname: &str) -> Option<i64> {
    kiwi_core::pynum::int(relname.strip_prefix("entry_p_")?)
}

pub async fn entry_partitions<C: GenericClient>(con: &C) -> PgResult<Vec<Partition>> {
    let rows = con
        .query(
            "SELECT c.oid, c.relname, \
                    COALESCE(t.spcname, 'pg_default') AS tablespace, \
                    pg_total_relation_size(c.oid) AS bytes \
             FROM pg_inherits i \
             JOIN pg_class c ON c.oid = i.inhrelid \
             LEFT JOIN pg_tablespace t ON t.oid = c.reltablespace \
             WHERE i.inhparent = 'entry'::regclass",
            &[],
        )
        .await
        .map_err(pg_err)?;
    let mut out = Vec::new();
    for r in &rows {
        let name: String = r.try_get(1).map_err(pg_err)?;
        let Some(lo) = partition_lo(&name) else { continue };
        out.push(Partition {
            oid: r.try_get(0).map_err(pg_err)?,
            name,
            lo,
            tablespace: r.try_get(2).map_err(pg_err)?,
            bytes: r.try_get(3).map_err(pg_err)?,
        });
    }
    Ok(out)
}

/// `tier_cold_partitions(after_days, now, limit)`: the moved names and bytes,
/// or None without the `cold` tablespace.
pub async fn tier_cold_partitions(
    pool: &Pool,
    after_days: i64,
    now: i64,
    limit: Option<usize>,
) -> PgResult<Option<(Vec<String>, i64)>> {
    if !cold_tablespace_exists(pool).await? {
        return Ok(None);
    }
    let keep_from = kiwi_core::lbmodels::cold_keep_from(now, after_days);
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let mut eligible: Vec<Partition> = entry_partitions(&**con)
        .await?
        .into_iter()
        .filter(|p| p.tablespace == "pg_default" && p.lo < keep_from)
        .collect();
    eligible.sort_by_key(|p| p.lo);
    if let Some(n) = limit {
        eligible.truncate(n);
    }
    let (mut moved, mut moved_bytes) = (Vec::new(), 0);
    for p in eligible {
        let idx = con
            .query("SELECT indexrelid::regclass::text AS name FROM pg_index WHERE indrelid = $1", &[&p.oid])
            .await
            .map_err(pg_err)?;
        con.batch_execute(&format!("ALTER TABLE {} SET TABLESPACE {COLD_TABLESPACE}", p.name)).await.map_err(pg_err)?;
        for r in &idx {
            let name: String = r.try_get(0).map_err(pg_err)?;
            con.batch_execute(&format!("ALTER INDEX {name} SET TABLESPACE {COLD_TABLESPACE}")).await.map_err(pg_err)?;
        }
        moved.push(p.name);
        moved_bytes += p.bytes;
    }
    Ok(Some((moved, moved_bytes)))
}

/// `tier_status`'s partition lists (None without the `cold` tablespace).
pub async fn tier_partitions(pool: &Pool) -> PgResult<Option<Vec<Partition>>> {
    if !cold_tablespace_exists(pool).await? {
        return Ok(None);
    }
    let con = pool.get().await.map_err(|e| e.to_string())?;
    Ok(Some(entry_partitions(&**con).await?))
}
