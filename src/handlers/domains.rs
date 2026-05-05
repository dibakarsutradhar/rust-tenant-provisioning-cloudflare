use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{error::AppError, middleware::tenant::TenantContext, services, state::AppState};

#[derive(Deserialize)]
pub struct AddDomainRequest {
    pub domain: String,
}

#[derive(Serialize)]
pub struct AddDomainResponse {
    pub id: Uuid,
    pub domain: String,
    pub cname_target: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct DomainStatus {
    pub id: Uuid,
    pub domain: String,
    pub status: String,
}

pub async fn add_domain(
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Json(body): Json<AddDomainRequest>,
) -> Result<Json<AddDomainResponse>, AppError> {
    let domain = body.domain.to_lowercase().trim().to_string();

    if domain.is_empty() || !domain.contains('.') {
        return Err(AppError::BadRequest("invalid domain".into()));
    }

    let tunnel_id = std::env::var("CLOUDFLARE_TUNNEL_ID")
        .map_err(|_| AppError::BadRequest("tunnel not configured".into()))?;

    let id = services::custom_domain::add_custom_domain(&state.db, ctx.tenant_id, domain.clone())
        .await
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(AddDomainResponse {
        id,
        domain,
        cname_target: format!("{tunnel_id}.cfargotunnel.com"),
        message: "Add the CNAME record then we'll detect it automatically".into(),
    }))
}

pub async fn list_domains(
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<Vec<DomainStatus>>, AppError> {
    let domains = sqlx::query_as!(
        DomainStatus,
        "SELECT id, domain, status FROM custom_domains
         WHERE tenant_id = $1
         ORDER BY created_at DESC",
        ctx.tenant_id
    )
    .fetch_all(&state.db)
    .await?;

    Ok(Json(domains))
}

// SSE stream for domain provisioning status
pub async fn domain_stream(
    axum::extract::Path(domain_id): axum::extract::Path<Uuid>,
    Extension(state): Extension<AppState>,
) -> Result<impl axum::response::IntoResponse, AppError> {
    use axum::response::sse::{Event, KeepAlive, Sse};
    use futures::StreamExt;
    use sqlx::postgres::PgListener;
    use std::{convert::Infallible, pin::Pin};
    use tokio_stream::wrappers::ReceiverStream;

    type SseStream = Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>>;

    // check current status first
    let current = sqlx::query_scalar!("SELECT status FROM custom_domains WHERE id = $1", domain_id)
        .fetch_optional(&state.db)
        .await?;

    match current.as_deref() {
        None => return Err(AppError::NotFound("domain not found".into())),
        Some("active") => {
            let domain =
                sqlx::query_scalar!("SELECT domain FROM custom_domains WHERE id = $1", domain_id)
                    .fetch_one(&state.db)
                    .await?;
            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("done")
                .data(domain))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        Some("failed") => {
            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("failed")
                .data("provisioning failed"))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        _ => {}
    }

    let channel = format!("domain_{}", domain_id.to_string().replace('-', "_"));
    let mut listener = PgListener::connect_with(&state.db).await?;
    listener.listen(&channel).await?;

    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(8);

    tokio::spawn(async move {
        loop {
            match listener.recv().await {
                Ok(notification) => {
                    let payload = notification.payload().to_string();
                    tracing::info!("Domain NOTIFY: {payload}");

                    let event = if payload.starts_with("done:") {
                        Event::default()
                            .event("done")
                            .data(payload.trim_start_matches("done:"))
                    } else if payload.starts_with("failed:") {
                        Event::default()
                            .event("failed")
                            .data(payload.trim_start_matches("failed:"))
                    } else {
                        Event::default().event("status").data(&payload)
                    };

                    let is_terminal =
                        payload.starts_with("done:") || payload.starts_with("failed:");
                    if tx.send(event).await.is_err() {
                        break;
                    }
                    if is_terminal {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("Domain listener error: {e}");
                    break;
                }
            }
        }
    });

    let stream: SseStream = Box::pin(ReceiverStream::new(rx).map(Ok::<Event, Infallible>));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

pub async fn acme_challenge(
    axum::extract::Path(token): axum::extract::Path<String>,
    Extension(state): Extension<AppState>,
) -> Result<String, AppError> {
    // look up the challenge response from DB
    let response = sqlx::query_scalar!(
        "SELECT response FROM acme_challenges WHERE token = $1",
        token
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("challenge not found".into()))?;

    Ok(response)
}
