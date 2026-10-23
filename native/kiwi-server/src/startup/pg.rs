//! `init_postgres`: the leaderboards, market and codexes `pg_schema.init`
//! scripts, each one simple-query batch like asyncpg's `execute(script)`.

pub const SCHEMAS: [(&str, &str); 3] = [
    ("leaderboards", include_str!("leaderboards.sql")),
    ("market", include_str!("market.sql")),
    ("codexes", include_str!("codexes.sql")),
];

pub async fn init(pool: &deadpool_postgres::Pool) {
    let client = match pool.get().await {
        Ok(c) => c,
        Err(e) => {
            tracing::error!("postgres: no connection for the schema: {e}");
            return;
        }
    };
    let mut ok = true;
    for (name, sql) in SCHEMAS {
        if let Err(e) = client.batch_execute(sql).await {
            ok = false;
            let detail = e.as_db_error().map_or_else(|| e.to_string(), |d| d.message().to_owned());
            tracing::error!("postgres: {name} schema failed: {detail}");
        }
    }
    if ok {
        tracing::info!("Postgres pool ready (leaderboards + market + codexes backends)");
    }
}
