use crate::{db, error::AppError, state::AppState};
use axum::{Extension, Json};
use serde::{Deserialize, Serialize};

#[derive(Deserialize)]
pub struct RegisterRequest {
    pub subdomain: String,
    pub company: String,
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct RegisterResponse {
    pub tenant_id: String,
    pub message: String,
}

pub async fn register(
    Extension(state): Extension<AppState>,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, AppError> {
    // basic validation
    let subdomain = body.subdomain.trim().to_lowercase();
    if subdomain.is_empty() || subdomain.contains('.') {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }

    // hash password
    let hash = bcrypt::hash(&body.password, bcrypt::DEFAULT_COST)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    // insert tenant (status: pending)
    let tenant_id = db::insert_tenant(&state.db, &subdomain, &body.company)
        .await
        .map_err(|e| match e {
            sqlx::Error::Database(db_err)
                if db_err.constraint() == Some("tenants_subdomain_key") =>
            {
                AppError::BadRequest("subdomain already taken".into())
            }
            e => AppError::Database(e),
        })?;

    // insert owner user
    db::insert_user(&state.db, tenant_id, &body.email, &hash).await?;

    // spawn provisioning task — fire and forget
    let db_clone = state.db.clone();
    let sub_clone = subdomain.clone();
    tokio::spawn(async move {
        crate::services::provisioning::run(db_clone, tenant_id, sub_clone).await;
    });

    Ok(Json(RegisterResponse {
        tenant_id: tenant_id.to_string(),
        message: format!("provisioning started — poll /api/provisioning/status/{tenant_id}"),
    }))
}
