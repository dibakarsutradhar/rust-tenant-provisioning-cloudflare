use crate::{error::AppError, state::AppState};
use axum::{Extension, Json};
use serde_json::{Value, json};

pub async fn health(Extension(state): Extension<AppState>) -> Result<Json<Value>, AppError> {
    sqlx::query("SELECT 1").execute(&state.db).await?;
    Ok(Json(json!({ "status": "ok", "db": "reachable" })))
}
