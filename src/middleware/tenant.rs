use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

use crate::{db, state::AppState};

// this gets injected into every request that passes through the middleware
#[derive(Clone, Debug)]
pub struct TenantContext {
    pub tenant_id: Uuid,
    pub subdomain: String,
}

pub async fn resolve_tenant(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    // strip port if present e.g. localhost:3000 -> localhost
    let host = host.split(':').next().unwrap_or("");

    let subdomain = extract_subdomain(host, &state.base_domain);

    let Some(subdomain) = subdomain else {
        // no subdomain (e.g. bare thegarageos.com) — skip tenant resolution
        return Ok(next.run(req).await);
    };

    // app subdomain is not a tenant — skip resolution
    if subdomain == "app" {
        return Ok(next.run(req).await);
    }

    // 1. check in-process cache first
    if let Some(tenant_id) = state.subdomain_cache.get(&subdomain) {
        req.extensions_mut().insert(TenantContext {
            tenant_id: *tenant_id,
            subdomain: subdomain.clone(),
        });
        return Ok(next.run(req).await);
    }

    // 2. miss — query Postgres
    match db::get_tenant_id_by_subdomain(&state.db, &subdomain).await {
        Ok(Some(tenant_id)) => {
            // warm the cache
            state.subdomain_cache.insert(subdomain.clone(), tenant_id);

            req.extensions_mut().insert(TenantContext {
                tenant_id,
                subdomain,
            });
            Ok(next.run(req).await)
        }
        Ok(None) => {
            tracing::warn!("Unknown subdomain: {subdomain}");
            Err(StatusCode::NOT_FOUND)
        }
        Err(e) => {
            tracing::error!("DB error resolving tenant: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn extract_subdomain(host: &str, base_domain: &str) -> Option<String> {
    // host: "ggdhaka.thegarageos.com"
    // base:         "thegarageos.com"
    // returns: Some("ggdhaka")
    let suffix = format!(".{base_domain}");
    if host.ends_with(&suffix) {
        let sub = &host[..host.len() - suffix.len()];
        if sub.is_empty() {
            None
        } else {
            Some(sub.to_string())
        }
    } else {
        None
    }
}
