//! `app/trove/leaderboards/pg_store.py` reads, SQL verbatim except the board
//! list and the chart's anchors, which return the same rows from index seeks:
//! the skip scans and orderings are what keep these under the statement
//! timeout on the cold tier.

use std::{collections::HashMap, future::Future};

use anyhow::Context;
use kiwi_core::lbread::{effective_reset_kind, is_player_board, trove_day_start};
use serde_json::{Map, Value, json};

use super::Live;

/// One `board` row, plus the anchor's contest type where the query joins it.
#[derive(Clone, Debug, PartialEq)]
pub struct BoardRow {
    pub uuid: i64,
    pub name_id: String,
    pub name: String,
    pub category_id: String,
    pub category: String,
    pub reset_kind_override: Option<String>,
    pub contest_type: Option<String>,
}

impl BoardRow {
    pub fn reset_kind(&self) -> &'static str {
        effective_reset_kind(self.reset_kind_override.as_deref(), self.uuid)
    }

    /// The `list_boards_at` dict (what Redis and `/site` carry).
    pub fn to_value(&self) -> Value {
        json!({
            "uuid": self.uuid, "name_id": self.name_id, "name": self.name,
            "category_id": self.category_id, "category": self.category,
            "contest_type": self.contest_type, "reset_kind": self.reset_kind(),
            "reset_kind_override": self.reset_kind_override, "player_board": is_player_board(self.uuid),
        })
    }

    /// `LeaderboardInfo`.
    pub fn info_v1(&self) -> Map<String, Value> {
        let v = json!({
            "uuid": self.uuid, "name_id": self.name_id, "name": self.name,
            "category_id": self.category_id, "category": self.category,
            "contest_type": self.contest_type, "reset_kind": self.reset_kind(),
            "player_board": is_player_board(self.uuid),
        });
        match v {
            Value::Object(m) => m,
            _ => Map::new(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Board {
    pub row: BoardRow,
    /// `(anchor, type)`, oldest first.
    pub contests: Vec<(i64, String)>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct EntryRow {
    pub player_name: String,
    pub rank: i64,
    pub score: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopEntry {
    pub player_name: String,
    pub score: f64,
    pub anchor: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct TopMeta {
    pub player_id: i64,
    pub player_name: String,
    pub rank: i64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct SeriesRow {
    pub player_id: i64,
    pub created_at: i64,
    pub rank: i64,
    pub score: f64,
}

/// `board_top_series`: anchors ascending, the latest anchor's top rows, and
/// those players' rows over the window (Postgres' order).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct TopSeries {
    pub anchors: Vec<i64>,
    pub top: Vec<TopMeta>,
    pub rows: Vec<SeriesRow>,
}

pub type Res<T> = anyhow::Result<T>;

/// The reads the leaderboards read routes and the warmer make.
pub trait Source: Sync {
    fn list_timestamps(&self, limit: i64, since: Option<i64>) -> impl Future<Output = Res<Vec<i64>>> + Send;
    fn list_days(&self, limit: i64) -> impl Future<Output = Res<Vec<i64>>> + Send;
    fn list_boards_at(&self, anchor: i64) -> impl Future<Output = Res<Vec<BoardRow>>> + Send;
    fn get_board(&self, uuid: i32) -> impl Future<Output = Res<Option<Board>>> + Send;
    fn latest_anchor_for_board(&self, uuid: i32) -> impl Future<Output = Res<Option<i64>>> + Send;
    fn top_entries_for_boards(
        &self,
        uuids: &[i32],
        anchor: i64,
    ) -> impl Future<Output = Res<HashMap<i64, TopEntry>>> + Send;
    fn list_entries(
        &self,
        uuid: i32,
        anchor: i64,
        limit: i64,
        offset: i64,
    ) -> impl Future<Output = Res<(Vec<EntryRow>, i64)>> + Send;
    fn previous_day_anchor(&self, uuid: i32, anchor: i64) -> impl Future<Output = Res<Option<i64>>> + Send;
    fn board_reset_kind(&self, uuid: i32) -> impl Future<Output = Res<&'static str>> + Send;
    /// `{name_lower: (rank, score)}`; a lowered name seen twice keeps the last row.
    fn prev_rows_for_players(
        &self,
        uuid: i32,
        prev_anchor: i64,
        names: &[String],
    ) -> impl Future<Output = Res<HashMap<String, (i64, f64)>>> + Send;
    fn board_top_series(&self, uuid: i32, window_start: i64, top: i64) -> impl Future<Output = Res<TopSeries>> + Send;
}

/// Python `str.lower()`.
pub fn py_lower(s: &str) -> String {
    s.to_lowercase()
}

const LIST_TIMESTAMPS: &str = "
                WITH RECURSIVE a AS (
                    SELECT (SELECT max(anchor) FROM entry) AS anchor
                    UNION ALL
                    SELECT (SELECT max(e.anchor) FROM entry e WHERE e.anchor < a.anchor)
                    FROM a WHERE a.anchor IS NOT NULL
                )
                SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1
                ";

const LIST_TIMESTAMPS_SINCE: &str = "
                WITH RECURSIVE a AS (
                    SELECT (SELECT max(anchor) FROM entry WHERE anchor >= $2) AS anchor
                    UNION ALL
                    SELECT (SELECT max(e.anchor) FROM entry e
                            WHERE e.anchor < a.anchor AND e.anchor >= $2)
                    FROM a WHERE a.anchor IS NOT NULL
                )
                SELECT anchor FROM a WHERE anchor IS NOT NULL LIMIT $1
                ";

const LIST_DAYS: &str = "
            WITH RECURSIVE d AS (
                SELECT (SELECT max(anchor) FROM entry) AS anchor
                UNION ALL
                SELECT (SELECT max(e.anchor) FROM entry e
                        WHERE e.anchor < ((d.anchor - $2) / 86400) * 86400 + $2)
                FROM d WHERE d.anchor IS NOT NULL
            )
            SELECT anchor FROM d WHERE anchor IS NOT NULL LIMIT $1
            ";

pub const LIST_BOARDS_AT: &str = "SELECT b.uuid, b.name_id, b.name, b.category_id, b.category, \
       b.reset_kind_override, bc.type AS contest_type \
FROM (SELECT DISTINCT board_uuid FROM entry WHERE anchor = $1) e \
JOIN board b ON b.uuid = e.board_uuid \
LEFT JOIN board_contest bc ON bc.board_uuid = b.uuid AND bc.anchor = $1 \
ORDER BY b.category, b.name";

/// `LIST_BOARDS_AT` with one index probe per board instead of a scan of the capture.
pub const LIST_BOARDS_AT_PROBE: &str = "SELECT b.uuid, b.name_id, b.name, b.category_id, b.category, \
       b.reset_kind_override, bc.type AS contest_type \
FROM board b \
CROSS JOIN LATERAL (SELECT 1 FROM entry e WHERE e.board_uuid = b.uuid AND e.anchor = $1 LIMIT 1) e \
LEFT JOIN board_contest bc ON bc.board_uuid = b.uuid AND bc.anchor = $1 \
ORDER BY b.category, b.name";

const GET_BOARD: &str =
    "SELECT uuid, name_id, name, category_id, category, reset_kind_override FROM board WHERE uuid = $1";
const BOARD_CONTESTS: &str = "SELECT anchor AS time, type FROM board_contest WHERE board_uuid = $1 ORDER BY anchor";
const LATEST_ANCHOR_FOR_BOARD: &str = "SELECT MAX(anchor) FROM entry WHERE board_uuid = $1";

const TOP_ENTRIES_FOR_BOARDS: &str = "SELECT b.uuid AS board_uuid, p.name AS player_name, t.score, t.anchor \
FROM unnest($1::int[]) AS b(uuid) \
CROSS JOIN LATERAL (\
    SELECT e.player_id, e.score, e.anchor FROM entry e \
    WHERE e.board_uuid = b.uuid AND e.anchor = $2 \
    ORDER BY e.rank, e.player_id LIMIT 1\
) t \
JOIN player p ON p.id = t.player_id";

const COUNT_ENTRIES: &str = "SELECT count(*) FROM entry WHERE board_uuid = $1 AND anchor = $2";
const LIST_ENTRIES: &str = "SELECT p.name AS player_name, e.rank, e.score \
FROM entry e JOIN player p ON p.id = e.player_id \
WHERE e.board_uuid = $1 AND e.anchor = $2 \
ORDER BY e.rank LIMIT $3 OFFSET $4";

const PREVIOUS_DAY_ANCHOR: &str = "SELECT MAX(anchor) FROM entry WHERE board_uuid = $1 AND anchor < $2";
const BOARD_RESET_KIND: &str = "SELECT reset_kind_override FROM board WHERE uuid = $1";

const PREV_ROWS_FOR_PLAYERS: &str = "SELECT p.name_lower, e.rank, e.score \
FROM entry e JOIN player p ON p.id = e.player_id \
WHERE e.board_uuid = $1 AND e.anchor = $2 AND p.name_lower = ANY($3)";

/// `SELECT DISTINCT anchor FROM entry WHERE board_uuid = $1 AND anchor >= $2
/// ORDER BY anchor` as a skip scan: one index seek per anchor rather than
/// reading every entry of the board in the window.
pub const SERIES_ANCHORS: &str = "
    WITH RECURSIVE a AS (
        SELECT (SELECT min(anchor) FROM entry WHERE board_uuid = $1 AND anchor >= $2) AS anchor
        UNION ALL
        SELECT (SELECT min(e.anchor) FROM entry e
                WHERE e.board_uuid = $1 AND e.anchor > a.anchor AND e.anchor >= $2)
        FROM a WHERE a.anchor IS NOT NULL
    )
    SELECT anchor FROM a WHERE anchor IS NOT NULL ORDER BY anchor";
pub const SERIES_TOP: &str = "SELECT e.player_id, p.name AS player_name, e.rank \
FROM entry e JOIN player p ON p.id = e.player_id \
WHERE e.board_uuid = $1 AND e.anchor = $2 \
ORDER BY e.rank LIMIT $3";
pub const SERIES_ROWS: &str = "SELECT e.player_id, e.anchor AS created_at, e.rank, e.score \
FROM entry e \
WHERE e.board_uuid = $1 AND e.anchor >= $2 AND e.player_id = ANY($3)";

impl Live<'_> {
    async fn client(&self) -> Res<deadpool_postgres::Object> {
        let pool = self.0.inner.db.pg.as_ref().context("POSTGRES_DSN not configured")?;
        pool.get().await.context("postgres pool")
    }

    async fn query(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Res<Vec<tokio_postgres::Row>> {
        let client = self.client().await?;
        let stmt = client.prepare_cached(sql).await?;
        Ok(client.query(&stmt, params).await?)
    }

    async fn scalar<T: for<'r> tokio_postgres::types::FromSql<'r>>(
        &self,
        sql: &str,
        params: &[&(dyn tokio_postgres::types::ToSql + Sync)],
    ) -> Res<Option<T>> {
        Ok(self.query(sql, params).await?.first().map(|r| r.try_get::<_, Option<T>>(0)).transpose()?.flatten())
    }
}

/// `list_boards_at`'s rows. Boards sharing a category and name come back in
/// whatever order the plan gives, so a tie reruns Python's query to keep it.
pub async fn board_rows_at(client: &deadpool_postgres::Client, anchor: i64) -> Res<Vec<tokio_postgres::Row>> {
    let stmt = client.prepare_cached(LIST_BOARDS_AT_PROBE).await?;
    let rows = client.query(&stmt, &[&anchor]).await?;
    let key = |r: &tokio_postgres::Row| -> Res<(String, String)> { Ok((r.try_get("category")?, r.try_get("name")?)) };
    let mut tied = false;
    for w in rows.windows(2) {
        tied |= key(&w[0])? == key(&w[1])?;
    }
    if !tied {
        return Ok(rows);
    }
    let stmt = client.prepare_cached(LIST_BOARDS_AT).await?;
    Ok(client.query(&stmt, &[&anchor]).await?)
}

fn board_row(r: &tokio_postgres::Row, contest: bool) -> Res<BoardRow> {
    Ok(BoardRow {
        uuid: i64::from(r.try_get::<_, i32>("uuid")?),
        name_id: r.try_get("name_id")?,
        name: r.try_get("name")?,
        category_id: r.try_get("category_id")?,
        category: r.try_get("category")?,
        reset_kind_override: r.try_get("reset_kind_override")?,
        contest_type: if contest { r.try_get("contest_type")? } else { None },
    })
}

impl Source for Live<'_> {
    async fn list_timestamps(&self, limit: i64, since: Option<i64>) -> Res<Vec<i64>> {
        let rows = match since {
            None => self.query(LIST_TIMESTAMPS, &[&limit]).await?,
            Some(since) => self.query(LIST_TIMESTAMPS_SINCE, &[&limit, &since]).await?,
        };
        rows.iter().map(|r| Ok(r.try_get::<_, i64>(0)?)).collect()
    }

    async fn list_days(&self, limit: i64) -> Res<Vec<i64>> {
        let offset = kiwi_core::lbread::TROVE_DAY_OFFSET;
        let rows = self.query(LIST_DAYS, &[&limit, &offset]).await?;
        rows.iter().map(|r| Ok(r.try_get::<_, i64>(0)?)).collect()
    }

    async fn list_boards_at(&self, anchor: i64) -> Res<Vec<BoardRow>> {
        board_rows_at(&self.client().await?, anchor).await?.iter().map(|r| board_row(r, true)).collect()
    }

    async fn get_board(&self, uuid: i32) -> Res<Option<Board>> {
        let client = self.client().await?;
        let stmt = client.prepare_cached(GET_BOARD).await?;
        let Some(b) = client.query_opt(&stmt, &[&uuid]).await? else { return Ok(None) };
        let stmt = client.prepare_cached(BOARD_CONTESTS).await?;
        let contests = client
            .query(&stmt, &[&uuid])
            .await?
            .iter()
            .map(|c| Ok((c.try_get::<_, i64>("time")?, c.try_get::<_, String>("type")?)))
            .collect::<Res<Vec<_>>>()?;
        Ok(Some(Board { row: board_row(&b, false)?, contests }))
    }

    async fn latest_anchor_for_board(&self, uuid: i32) -> Res<Option<i64>> {
        self.scalar(LATEST_ANCHOR_FOR_BOARD, &[&uuid]).await
    }

    async fn top_entries_for_boards(&self, uuids: &[i32], anchor: i64) -> Res<HashMap<i64, TopEntry>> {
        if uuids.is_empty() {
            return Ok(HashMap::new());
        }
        let uuids = uuids.to_vec();
        let rows = self.query(TOP_ENTRIES_FOR_BOARDS, &[&uuids, &anchor]).await?;
        rows.iter()
            .map(|r| {
                let entry = TopEntry {
                    player_name: r.try_get("player_name")?,
                    score: r.try_get("score")?,
                    anchor: r.try_get("anchor")?,
                };
                Ok((i64::from(r.try_get::<_, i32>("board_uuid")?), entry))
            })
            .collect()
    }

    async fn list_entries(&self, uuid: i32, anchor: i64, limit: i64, offset: i64) -> Res<(Vec<EntryRow>, i64)> {
        let client = self.client().await?;
        let stmt = client.prepare_cached(COUNT_ENTRIES).await?;
        let total: Option<i64> = client.query_one(&stmt, &[&uuid, &anchor]).await?.try_get(0)?;
        let stmt = client.prepare_cached(LIST_ENTRIES).await?;
        let items = client
            .query(&stmt, &[&uuid, &anchor, &limit, &offset])
            .await?
            .iter()
            .map(|r| {
                Ok(EntryRow {
                    player_name: r.try_get("player_name")?,
                    rank: i64::from(r.try_get::<_, i32>("rank")?),
                    score: r.try_get("score")?,
                })
            })
            .collect::<Res<Vec<_>>>()?;
        Ok((items, total.unwrap_or(0)))
    }

    async fn previous_day_anchor(&self, uuid: i32, anchor: i64) -> Res<Option<i64>> {
        self.scalar(PREVIOUS_DAY_ANCHOR, &[&uuid, &trove_day_start(anchor)]).await
    }

    async fn board_reset_kind(&self, uuid: i32) -> Res<&'static str> {
        let ov: Option<String> = self.scalar(BOARD_RESET_KIND, &[&uuid]).await?;
        Ok(effective_reset_kind(ov.as_deref(), i64::from(uuid)))
    }

    async fn prev_rows_for_players(
        &self,
        uuid: i32,
        prev_anchor: i64,
        names: &[String],
    ) -> Res<HashMap<String, (i64, f64)>> {
        if names.is_empty() {
            return Ok(HashMap::new());
        }
        let lowered: Vec<String> = names.iter().map(|n| py_lower(n)).collect();
        let rows = self.query(PREV_ROWS_FOR_PLAYERS, &[&uuid, &prev_anchor, &lowered]).await?;
        let mut out = HashMap::new();
        for r in &rows {
            let rank = i64::from(r.try_get::<_, i32>("rank")?);
            out.insert(r.try_get::<_, String>("name_lower")?, (rank, r.try_get::<_, f64>("score")?));
        }
        Ok(out)
    }

    async fn board_top_series(&self, uuid: i32, window_start: i64, top: i64) -> Res<TopSeries> {
        let client = self.client().await?;
        let stmt = client.prepare_cached(SERIES_ANCHORS).await?;
        let anchors = client
            .query(&stmt, &[&uuid, &window_start])
            .await?
            .iter()
            .map(|r| Ok(r.try_get::<_, i64>(0)?))
            .collect::<Res<Vec<i64>>>()?;
        let Some(&latest) = anchors.last() else { return Ok(TopSeries::default()) };
        let stmt = client.prepare_cached(SERIES_TOP).await?;
        let top = client
            .query(&stmt, &[&uuid, &latest, &top])
            .await?
            .iter()
            .map(|r| {
                Ok(TopMeta {
                    player_id: r.try_get("player_id")?,
                    player_name: r.try_get("player_name")?,
                    rank: i64::from(r.try_get::<_, i32>("rank")?),
                })
            })
            .collect::<Res<Vec<_>>>()?;
        if top.is_empty() {
            return Ok(TopSeries { anchors, top, rows: Vec::new() });
        }
        let ids: Vec<i64> = top.iter().map(|m| m.player_id).collect();
        let stmt = client.prepare_cached(SERIES_ROWS).await?;
        let rows = client
            .query(&stmt, &[&uuid, &window_start, &ids])
            .await?
            .iter()
            .map(|r| {
                Ok(SeriesRow {
                    player_id: r.try_get("player_id")?,
                    created_at: r.try_get("created_at")?,
                    rank: i64::from(r.try_get::<_, i32>("rank")?),
                    score: r.try_get("score")?,
                })
            })
            .collect::<Res<Vec<_>>>()?;
        Ok(TopSeries { anchors, top, rows })
    }
}
