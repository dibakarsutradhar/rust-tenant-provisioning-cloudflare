use axum::{
    Extension, Json,
    extract::Request,
    http::StatusCode,
    response::{Html, IntoResponse, Response},
};
use serde_json::{Value, json};

use crate::{error::AppError, middleware::tenant::TenantContext, state::AppState};

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
pub async fn root(req: Request) -> Response {
    let host = req
        .headers()
        .get("host")
        .and_then(|h| h.to_str().ok())
        .unwrap_or("");

    let subdomain = host.split('.').next().unwrap_or("");

    match subdomain {
        "app" | "" => {
            // signup page
            let html = tokio::fs::read_to_string("static/index.html")
                .await
                .unwrap_or_else(|_| "<h1>404</h1>".to_string());
            Html(html).into_response()
        }
        _ => {
            // tenant subdomain — serve login page
            let html = tokio::fs::read_to_string("static/login.html")
                .await
                .unwrap_or_else(|_| "<h1>404</h1>".to_string());
            Html(html).into_response()
        }
    }
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
