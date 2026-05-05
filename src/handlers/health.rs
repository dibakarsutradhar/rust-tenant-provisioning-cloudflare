use axum::{
    Extension, Json,
    extract::Request,
    http::StatusCode,
    response::{Html, IntoResponse, Redirect, Response},
};
use serde_json::{Value, json};

use crate::{db, error::AppError, middleware::tenant::TenantContext, state::AppState};

pub async fn health(Extension(state): Extension<AppState>) -> Result<Json<Value>, AppError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(json!({ "status": "ok", "db": "reachable" })))
}

pub async fn tenant_home(Extension(ctx): Extension<TenantContext>) -> Json<Value> {
    Json(json!({
        "tenant_id": ctx.tenant_id,
        "subdomain": ctx.subdomain,
    }))
}

// serves different index based on subdomain
pub async fn root(Extension(state): Extension<AppState>, req: Request) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    let suffix = format!(".{}", state.base_domain);

    if host.ends_with(&suffix) {
        let sub = &host[..host.len() - suffix.len()];

        if sub == "app" {
            // app.thegarageos.com → signup
            let html = tokio::fs::read_to_string("static/index.html")
                .await
                .unwrap_or_default();
            return Html(html).into_response();
        }

        // tenant subdomain e.g. ggdhaka.thegarageos.com
        if !sub.is_empty() {
            if let Ok(Some(tenant_id)) = db::get_tenant_id_by_subdomain(&state.db, sub).await {
                let primary = sqlx::query_scalar!(
                    "SELECT primary_domain FROM tenants WHERE id = $1",
                    tenant_id
                )
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .flatten();

                if let Some(primary) = primary {
                    return Redirect::temporary(&format!("https://{primary}/login.html"))
                        .into_response();
                }
            }

            // no custom domain — serve login directly
            let html = tokio::fs::read_to_string("static/login.html")
                .await
                .unwrap_or_default();
            return Html(html).into_response();
        }
    }

    // custom domain e.g. demo-gos.dibakar.me → login
    let html = tokio::fs::read_to_string("static/login.html")
        .await
        .unwrap_or_default();
    Html(html).into_response()
}

// blocks signup page on tenant subdomains
pub async fn block_signup(req: Request) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let subdomain = host.split('.').next().unwrap_or("");

    if subdomain == "app" {
        let html = tokio::fs::read_to_string("static/index.html")
            .await
            .unwrap_or_else(|_| "<h1>404</h1>".to_string());
        Html(html).into_response()
    } else {
        // redirect tenant subdomains away from signup
        axum::response::Redirect::to("/login.html").into_response()
    }
}

// checks for primary domain before serving
pub async fn serve_html(Extension(state): Extension<AppState>, req: Request) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("")
        .split(':')
        .next()
        .unwrap_or("");

    let path = req.uri().path().to_string();
    let suffix = format!(".{}", state.base_domain);

    if host.ends_with(&suffix) {
        let sub = &host[..host.len() - suffix.len()];
        if sub != "app" && !sub.is_empty() {
            if let Ok(Some(tenant_id)) = db::get_tenant_id_by_subdomain(&state.db, sub).await {
                let primary = sqlx::query_scalar!(
                    "SELECT primary_domain FROM tenants WHERE id = $1",
                    tenant_id
                )
                .fetch_optional(&state.db)
                .await
                .ok()
                .flatten()
                .flatten();

                if let Some(primary) = primary {
                    let target = format!("https://{primary}{path}");
                    tracing::info!("Redirecting {host}{path} → {target}");
                    return Redirect::temporary(&target).into_response();
                }
            }
        }
    }

    let file_path = format!("static{path}");
    match tokio::fs::read_to_string(&file_path).await {
        Ok(content) => Html(content).into_response(),
        Err(_) => StatusCode::NOT_FOUND.into_response(),
    }
}
