//! Shared, cheaply clonable application state.

use std::sync::Arc;

use crate::{config::Settings, db::Db};

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub settings: Settings,
    pub db: Db,
}

impl AppState {
    pub async fn new(settings: Settings) -> anyhow::Result<Self> {
        let db = Db::connect().await?;
        Ok(AppState { inner: Arc::new(Inner { settings, db }) })
    }

    pub fn settings(&self) -> &Settings {
        &self.inner.settings
    }

    /// Mongo database. Panics only if a ported route is served without
    /// `MONGO_URI`, which is a deployment error.
    pub fn mongo(&self) -> &mongodb::Database {
        self.inner.db.mongo.as_ref().expect("MONGO_URI not configured")
    }

    pub fn redis(&self) -> redis::aio::ConnectionManager {
        self.inner.db.redis.clone().expect("REDIS_URL not configured")
    }

    pub fn pg(&self) -> &deadpool_postgres::Pool {
        self.inner.db.pg.as_ref().expect("POSTGRES_DSN not configured")
    }
}
