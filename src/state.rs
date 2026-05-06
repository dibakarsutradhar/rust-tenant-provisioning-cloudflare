use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

use crate::config::Config;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub config: Config,
    pub subdomain_cache: std::sync::Arc<DashMap<String, Uuid>>,
    pub primary_domain_cache: std::sync::Arc<DashMap<Uuid, Option<String>>>,
}

impl AppState {
    pub fn new(db: PgPool, config: Config) -> Self {
        Self {
            db,
            config,
            subdomain_cache: std::sync::Arc::new(DashMap::new()),
            primary_domain_cache: std::sync::Arc::new(DashMap::new()),
        }
    }
}
