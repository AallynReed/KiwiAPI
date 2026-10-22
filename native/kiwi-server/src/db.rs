//! Connections to the same stores the Python app uses: Mongo (`MONGO_URI` /
//! `MONGO_DB`), Redis (`REDIS_URL`) and Postgres (`POSTGRES_DSN`). Each is
//! optional at startup so a pure-proxy deploy needs none of them.

use anyhow::Context;

#[derive(Clone)]
pub struct Db {
    pub mongo: Option<mongodb::Database>,
    pub redis: Option<redis::aio::ConnectionManager>,
    pub pg: Option<deadpool_postgres::Pool>,
}

fn env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|v| !v.is_empty())
}

impl Db {
    pub async fn connect() -> anyhow::Result<Self> {
        let mongo = match env("MONGO_URI") {
            Some(uri) => {
                let client = mongodb::Client::with_uri_str(&uri).await.context("MONGO_URI")?;
                Some(client.database(&env("MONGO_DB").unwrap_or_else(|| "kiwi".into())))
            }
            None => None,
        };
        let redis = match env("REDIS_URL") {
            Some(url) => {
                let client = redis::Client::open(url).context("REDIS_URL")?;
                Some(redis::aio::ConnectionManager::new(client).await.context("redis connect")?)
            }
            None => None,
        };
        let pg = match env("POSTGRES_DSN") {
            Some(dsn) => {
                let cfg: tokio_postgres::Config = dsn.parse().context("POSTGRES_DSN")?;
                let max = env("POSTGRES_POOL_MAX").and_then(|v| v.parse().ok()).unwrap_or(10);
                let mgr = deadpool_postgres::Manager::new(cfg, tokio_postgres::NoTls);
                Some(deadpool_postgres::Pool::builder(mgr).max_size(max).build().context("pg pool")?)
            }
            None => None,
        };
        Ok(Db { mongo, redis, pg })
    }
}
