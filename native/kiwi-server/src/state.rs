//! Shared, cheaply clonable application state.

use std::sync::Arc;

use axum::body::Body;
use hyper_util::client::legacy::{connect::HttpConnector, Client};
use hyper_util::rt::TokioExecutor;

use crate::{config::Settings, db::Db};

pub type HttpClient = Client<HttpConnector, Body>;

#[derive(Clone)]
pub struct AppState {
    pub inner: Arc<Inner>,
}

pub struct Inner {
    pub settings: Settings,
    pub db: Db,
    /// Pooled HTTP/1.1 client for the Python upstream.
    pub upstream: HttpClient,
}

impl AppState {
    pub async fn new(settings: Settings) -> anyhow::Result<Self> {
        let mut connector = HttpConnector::new();
        connector.set_nodelay(true);
        let upstream = Client::builder(TokioExecutor::new())
            .pool_max_idle_per_host(256)
            .build(connector);
        let db = Db::connect().await?;
        Ok(AppState { inner: Arc::new(Inner { settings, db, upstream }) })
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
