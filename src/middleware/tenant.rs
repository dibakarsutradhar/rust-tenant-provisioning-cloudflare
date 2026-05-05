use axum::{
    extract::{Request, State},
    http::StatusCode,
    middleware::Next,
    response::Response,
};
use uuid::Uuid;

use crate::{db, state::AppState};

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
    // Extract and clone host immediately to end the immutable borrow of `req`
    // before any mutable borrow (extensions_mut) is needed later.
    let host: String = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("")
        .to_owned();

    let subdomain = extract_subdomain(&host, &state.base_domain);

    // path resolution flow ->
    // host = ggdhaka.thegarageos.com  → path 1 → tenants table
    // host = app.thegarageos.com      → path 1 → skip (app)
    // host = app.ggdhaka.com          → path 1 misses → path 2 → custom_domains table
    // host = thegarageos.com          → no subdomain → skip

    // ── path 1: known subdomain of thegarageos.com ──────────────────────────
    if let Some(ref sub) = subdomain {
        // app subdomain is not a tenant
        if sub == "app" {
            return Ok(next.run(req).await);
        }

        // check in-process cache
        if let Some(tenant_id) = state.subdomain_cache.get(sub) {
            req.extensions_mut().insert(TenantContext {
                tenant_id: *tenant_id,
                subdomain: sub.clone(),
            });
            return Ok(next.run(req).await);
        }

        // query tenants table
        match db::get_tenant_id_by_subdomain(&state.db, sub).await {
            Ok(Some(tenant_id)) => {
                state.subdomain_cache.insert(sub.clone(), tenant_id);
                req.extensions_mut().insert(TenantContext {
                    tenant_id,
                    subdomain: sub.clone(),
                });
                return Ok(next.run(req).await);
            }
            Ok(None) => {
                // not a known tenant subdomain — fall through to custom domain check
            }
            Err(e) => {
                tracing::error!("DB error resolving subdomain: {e}");
                return Err(StatusCode::INTERNAL_SERVER_ERROR);
            }
        }
    }

    // ── path 2: no subdomain match — try as custom domain ───────────────────
    // e.g. host = "app.ggdhaka.com" which has no .thegarageos.com suffix
    let cache_key = format!("custom:{}", host);

    if let Some(tenant_id) = state.subdomain_cache.get(&cache_key) {
        req.extensions_mut().insert(TenantContext {
            tenant_id: *tenant_id,
            subdomain: host.to_string(),
        });
        return Ok(next.run(req).await);
    }

    match db::get_tenant_id_by_custom_domain(&state.db, &host).await {
        Ok(Some(tenant_id)) => {
            state.subdomain_cache.insert(cache_key, tenant_id);
            req.extensions_mut().insert(TenantContext {
                tenant_id,
                subdomain: host.to_string(),
            });
            Ok(next.run(req).await)
        }
        Ok(None) => {
            tracing::warn!("Unknown host: {host}");
            Err(StatusCode::NOT_FOUND)
        }
        Err(e) => {
            tracing::error!("DB error resolving custom domain: {e}");
            Err(StatusCode::INTERNAL_SERVER_ERROR)
        }
    }
}

fn extract_subdomain(host: &str, base_domain: &str) -> Option<String> {
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
