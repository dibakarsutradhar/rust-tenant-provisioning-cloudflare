use axum::{Extension, Json};
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
