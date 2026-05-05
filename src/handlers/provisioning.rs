// src/handlers/provisioning.rs
use axum::{
    Extension,
    extract::Path,
    response::sse::{Event, KeepAlive, Sse},
};
use futures::StreamExt;
use sqlx::postgres::PgListener;
use std::{convert::Infallible, pin::Pin};
use tokio_stream::wrappers::ReceiverStream;
use uuid::Uuid;

use crate::{error::AppError, state::AppState};

// box the stream so all arms have the same type
type SseStream = Pin<Box<dyn futures::Stream<Item = Result<Event, Infallible>> + Send>>;

pub async fn status_stream(
    Path(tenant_id): Path<Uuid>,
    Extension(state): Extension<AppState>,
) -> Result<Sse<SseStream>, AppError> {
    // check current status first — maybe provisioning already finished
    // before the browser connected
    let current = sqlx::query_scalar!("SELECT status FROM tenants WHERE id = $1", tenant_id)
        .fetch_optional(&state.db)
        .await?;

    match current.as_deref() {
        None => return Err(AppError::NotFound("tenant not found".into())),
        Some("active") => {
            // already done — return a single immediate event
            let subdomain =
                sqlx::query_scalar!("SELECT subdomain FROM tenants WHERE id = $1", tenant_id)
                    .fetch_one(&state.db)
                    .await?;

            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("done")
                .data(subdomain))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        Some("failed") => {
            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("failed")
                .data("provisioning failed"))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        _ => {} // pending — fall through to LISTEN
    }

    // subscribe to postgres NOTIFY on this tenant's channel
    // channel name must be a valid identifier — replace hyphens in UUID
    let channel = format!("tenant_{}", tenant_id.to_string().replace('-', "_"));

    let mut listener = PgListener::connect_with(&state.db).await?;
    listener.listen(&channel).await?;

    // bridge PgListener into a tokio channel so we can make it a Stream
    let (tx, rx) = tokio::sync::mpsc::channel::<Event>(4);

    tokio::spawn(async move {
        loop {
            match listener.recv().await {
                Ok(notification) => {
                    let payload = notification.payload().to_string();
                    tracing::info!("NOTIFY received on {channel}: {payload}");

                    // check this before the if/else that moves `payload`
                    let is_terminal =
                        payload.starts_with("done:") || payload.starts_with("failed:");

                    let event = if payload.starts_with("done:") {
                        let subdomain = payload.trim_start_matches("done:").to_string();
                        Event::default().event("done").data(subdomain)
                    } else {
                        Event::default().event("status").data(payload)
                    };

                    if tx.send(event).await.is_err() {
                        break;
                    }
                    if is_terminal {
                        break;
                    }
                }
                Err(e) => {
                    tracing::error!("PgListener error: {e}");
                    let _ = tx
                        .send(Event::default().event("error").data("listener error"))
                        .await;
                    break;
                }
            }
        }
    });

    let stream: SseStream = Box::pin(ReceiverStream::new(rx).map(Ok::<Event, Infallible>));
    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}
