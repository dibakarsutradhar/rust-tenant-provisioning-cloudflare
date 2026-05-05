use dashmap::DashMap;
use sqlx::PgPool;
use uuid::Uuid;

#[derive(Clone)]
pub struct AppState {
    pub db: PgPool,
    pub subdomain_cache: std::sync::Arc<DashMap<String, Uuid>>, // subdomain → tenant_id
    pub base_domain: String,
}

impl AppState {
    pub fn new(db: PgPool) -> Self {
        let base_domain =
            std::env::var("BASE_DOMAIN").unwrap_or_else(|_| "thegarageos.com".to_string());

        Self {
            db,
            subdomain_cache: std::sync::Arc::new(DashMap::new()),
            base_domain,
        }
    }
}
