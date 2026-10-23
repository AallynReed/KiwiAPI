//! The two `pg_store.py` deletes behind `/delete-anchor`, SQL verbatim.

use deadpool_postgres::Pool;

use crate::lb_ingest::store::{PgResult, exec};

/// `delete_anchor`: the rows removed.
pub async fn delete_anchor(pool: &Pool, anchor: i64) -> PgResult<i64> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    let deleted = exec(&**con, "DELETE FROM entry WHERE anchor = $1", &[anchor]).await;
    crate::lbread::store::entries_written();
    Ok(deleted? as i64)
}

/// `delete_estimate`.
pub async fn delete_estimate(pool: &Pool, window_end: i64) -> PgResult<()> {
    let con = pool.get().await.map_err(|e| e.to_string())?;
    exec(&**con, "DELETE FROM activity_estimate WHERE window_end = $1", &[window_end]).await.map(drop)
}
