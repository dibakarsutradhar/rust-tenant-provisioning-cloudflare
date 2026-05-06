use axum::{
    extract::{Request, State},
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};

use crate::services::jwt;
use crate::state::AppState;

pub async fn require_auth(
    State(state): State<AppState>,
    mut req: Request,
    next: Next,
) -> Result<Response, StatusCode> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = jwt::verify(&state.config, token).map_err(|e| {
        tracing::warn!("JWT verify failed: {e}");
        StatusCode::UNAUTHORIZED
    })?;

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
