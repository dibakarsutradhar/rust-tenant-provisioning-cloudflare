use axum::{
    extract::Request,
    http::{StatusCode, header},
    middleware::Next,
    response::Response,
};

use crate::services::jwt;

pub async fn require_auth(mut req: Request, next: Next) -> Result<Response, StatusCode> {
    let token = req
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .ok_or(StatusCode::UNAUTHORIZED)?;

    let claims = jwt::verify(token).map_err(|e| {
        tracing::warn!("JWT verify failed: {e}");
        StatusCode::UNAUTHORIZED
    })?;

    req.extensions_mut().insert(claims);
    Ok(next.run(req).await)
}
