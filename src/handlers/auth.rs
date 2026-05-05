use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{db, error::AppError, services, state::AppState};

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

// ── login ──────────────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct LoginRequest {
    pub email: String,
    pub password: String,
}

#[derive(Serialize)]
pub struct LoginResponse {
    pub token: String,
    pub tenant_id: Uuid,
    pub role: String,
}

pub async fn login(
    Extension(state): Extension<AppState>,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    // find user by email — we need subdomain from Host header ideally,
    // but for MVP we find by email globally (emails are unique per tenant)
    let user = sqlx::query!(
        "SELECT u.id, u.tenant_id, u.password_hash, u.role
         FROM users u
         JOIN tenants t ON t.id = u.tenant_id
         WHERE u.email = $1 AND t.status = 'active'",
        body.email,
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::BadRequest("invalid email or password".into()))?;

    // verify password
    let valid = bcrypt::verify(&body.password, &user.password_hash)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    if !valid {
        return Err(AppError::BadRequest("invalid email or password".into()));
    }

    // issue JWT
    let token = services::jwt::issue(user.id, user.tenant_id, &user.role)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(LoginResponse {
        token,
        tenant_id: user.tenant_id,
        role: user.role,
    }))
}
