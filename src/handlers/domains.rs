use axum::{Extension, Json, response::sse::Event};
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
    pub cname_target: String, // show immediately
    pub cname_proxy: String,  // instruction
    pub message: String,
}

#[derive(Serialize)]
pub struct DomainStatus {
    pub id: Uuid,
    pub domain: String,
    pub status: String,
    pub cname_target: String,
}

#[derive(Serialize)]
pub struct DomainStatusDetail {
    pub id: Uuid,
    pub domain: String,
    pub status: String,
    pub cf_hostname_id: Option<String>,
    pub ownership_txt_name: Option<String>,
    pub ownership_txt_value: Option<String>,
    pub ssl_status: Option<String>,
    pub ssl_ca: Option<String>,
    pub acme_challenges: Vec<AcmeChallenge>,
    pub verification_errors: Vec<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Serialize)]
pub struct AcmeChallenge {
    pub token: String,
    pub url: String,
    pub served: bool, // we check if it's in our DB
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

    let id = services::custom_domain::add_custom_domain(
        &state.db,
        &state.config,
        ctx.tenant_id,
        domain.clone(),
    )
    .await
    .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(AddDomainResponse {
        id,
        domain: domain.clone(),
        cname_target: format!("{tunnel_id}.cfargotunnel.com"),
        cname_proxy: "OFF — must be grey cloud, not orange".into(),
        message: "Step 1: add the CNAME below. Step 2: we will send the TXT record via the status stream once ready.".into(),
    }))
}

pub async fn list_domains(
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<Vec<DomainStatus>>, AppError> {
    let tunnel_id = state.config.cloudflare_tunnel_id.clone();
    let cname_target = format!("{tunnel_id}.cfargotunnel.com");

    let domains = sqlx::query!(
        "SELECT id, domain, status FROM custom_domains
         WHERE tenant_id = $1
         ORDER BY created_at DESC",
        ctx.tenant_id
    )
    .fetch_all(&state.db)
    .await?;

    let result = domains
        .into_iter()
        .map(|d| DomainStatus {
            id: d.id,
            domain: d.domain,
            status: d.status,
            cname_target: cname_target.clone(),
        })
        .collect();

    Ok(Json(result))
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

    // fetch current status + ownership info in one query
    let row = sqlx::query!(
        "SELECT status, ownership_txt_name, ownership_txt_value, domain
         FROM custom_domains WHERE id = $1",
        domain_id
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("domain not found".into()))?;

    match row.status.as_str() {
        "active" => {
            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("done")
                .data(row.domain))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        "failed" => {
            let stream: SseStream = Box::pin(tokio_stream::once(Ok(Event::default()
                .event("failed")
                .data("provisioning failed"))));
            return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
        }
        "ssl_pending" => {
            // client reconnected after missing the records: event
            // re-send the TXT records immediately so UI can catch up
            if let (Some(name), Some(value)) = (&row.ownership_txt_name, &row.ownership_txt_value) {
                let records_event = format!("{name}={value}");
                let channel = format!("domain_{}", domain_id.to_string().replace('-', "_"));
                let mut listener = PgListener::connect_with(&state.db).await?;
                listener.listen(&channel).await?;

                let (tx, rx) = tokio::sync::mpsc::channel::<Event>(8);

                // send the records event immediately
                let _ = tx
                    .send(Event::default().event("records").data(records_event))
                    .await;

                // then forward future NOTIFY events
                tokio::spawn(async move {
                    loop {
                        match listener.recv().await {
                            Ok(notification) => {
                                let payload = notification.payload().to_string();
                                tracing::info!("Domain NOTIFY: {payload}");
                                let event = build_event(&payload);
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

                let stream: SseStream =
                    Box::pin(ReceiverStream::new(rx).map(Ok::<Event, Infallible>));
                return Ok(Sse::new(stream).keep_alive(KeepAlive::default()));
            }
        }
        _ => {} // pending — fall through to normal LISTEN
    }

    // normal pending flow
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
                    let event = build_event(&payload);
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

fn build_event(payload: &str) -> Event {
    if payload.starts_with("done:") {
        Event::default()
            .event("done")
            .data(payload.trim_start_matches("done:"))
    } else if payload.starts_with("failed:") {
        Event::default()
            .event("failed")
            .data(payload.trim_start_matches("failed:"))
    } else if payload.starts_with("records:") {
        Event::default()
            .event("records")
            .data(payload.trim_start_matches("records:"))
    } else {
        Event::default().event("status").data(payload)
    }
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

pub async fn delete_domain(
    axum::extract::Path(domain_id): axum::extract::Path<Uuid>,
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<axum::http::StatusCode, AppError> {
    // verify domain belongs to this tenant
    let exists = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM custom_domains
         WHERE id = $1 AND tenant_id = $2",
        domain_id,
        ctx.tenant_id,
    )
    .fetch_one(&state.db)
    .await?
    .unwrap_or(0);

    if exists == 0 {
        return Err(AppError::NotFound("domain not found".into()));
    }

    // delete from kv_cache too
    let domain = sqlx::query_scalar!("SELECT domain FROM custom_domains WHERE id = $1", domain_id)
        .fetch_one(&state.db)
        .await?;

    sqlx::query!(
        "DELETE FROM kv_cache WHERE key = $1",
        format!("custom:{domain}")
    )
    .execute(&state.db)
    .await?;

    // clear primary domain if this was it
    sqlx::query!(
        "UPDATE tenants SET primary_domain = NULL
         WHERE id = $1 AND primary_domain = $2",
        ctx.tenant_id,
        domain,
    )
    .execute(&state.db)
    .await?;

    // clear primary domain cache
    state.primary_domain_cache.remove(&ctx.tenant_id);

    sqlx::query!("DELETE FROM custom_domains WHERE id = $1", domain_id)
        .execute(&state.db)
        .await?;

    tracing::info!(
        "Deleted custom domain {domain} for tenant {}",
        ctx.tenant_id
    );
    Ok(axum::http::StatusCode::NO_CONTENT)
}

pub async fn verify_domain(
    axum::extract::Path(domain_id): axum::extract::Path<Uuid>,
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<axum::http::StatusCode, AppError> {
    // confirm belongs to tenant
    let row = sqlx::query!(
        "SELECT domain, status, cf_hostname_id FROM custom_domains
         WHERE id = $1 AND tenant_id = $2",
        domain_id,
        ctx.tenant_id,
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("domain not found".into()))?;

    // if we have a CF hostname id, trigger a CF recheck directly
    if let Some(ref cf_id) = row.cf_hostname_id {
        let token = state.config.cloudflare_api_token.clone();
        let zone_id = state.config.cloudflare_zone_id.clone();
        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames/{cf_id}"
        );
        let client = reqwest::Client::new();
        let _ = client
            .patch(&url)
            .header("Authorization", format!("Bearer {token}"))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({}))
            .send()
            .await;
        tracing::info!("Triggered CF recheck for {cf_id}");
    }

    if row.status != "active" {
        sqlx::query!(
            "UPDATE custom_domains SET status = 'pending' WHERE id = $1",
            domain_id
        )
        .execute(&state.db)
        .await?;

        let db = state.db.clone();
        let config = state.config.clone();
        let domain = row.domain.clone();

        tokio::spawn(async move {
            if let Err(e) =
                crate::services::custom_domain::verify_and_provision(db, config, domain_id, domain)
                    .await
            {
                tracing::error!("Re-verify failed: {e}");
            }
        });
    }

    Ok(axum::http::StatusCode::ACCEPTED)
}

pub async fn domain_status(
    axum::extract::Path(domain_id): axum::extract::Path<Uuid>,
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
) -> Result<Json<DomainStatusDetail>, AppError> {
    let row = sqlx::query!(
        "SELECT id, domain, status, cf_hostname_id,
                ownership_txt_name, ownership_txt_value, created_at
         FROM custom_domains
         WHERE id = $1 AND tenant_id = $2",
        domain_id,
        ctx.tenant_id,
    )
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| AppError::NotFound("domain not found".into()))?;

    // fetch live CF status if we have a hostname id
    let mut ssl_status = None;
    let mut ssl_ca = None;
    let mut acme_challenges = vec![];
    let mut verification_errors = vec![];

    if let Some(ref cf_id) = row.cf_hostname_id {
        let token = state.config.cloudflare_api_token.clone();
        let zone_id = state.config.cloudflare_zone_id.clone();

        let url = format!(
            "https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames/{cf_id}"
        );

        let client = reqwest::Client::new();
        if let Ok(res) = client
            .get(&url)
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await
        {
            if let Ok(json) = res.json::<serde_json::Value>().await {
                ssl_status = json["result"]["ssl"]["status"]
                    .as_str()
                    .map(|s| s.to_string());

                ssl_ca = json["result"]["ssl"]["certificate_authority"]
                    .as_str()
                    .map(|s| s.to_string());

                // get verification errors
                if let Some(errs) = json["result"]["verification_errors"].as_array() {
                    verification_errors = errs
                        .iter()
                        .filter_map(|e| e.as_str().map(|s| s.to_string()))
                        .collect();
                }

                // get ACME challenges and check if we're serving them
                if let Some(records) = json["result"]["ssl"]["validation_records"].as_array() {
                    for record in records {
                        let http_url = record["http_url"].as_str().unwrap_or("").to_string();
                        let token_str = http_url.split('/').last().unwrap_or("").to_string();

                        // auto-store new challenges
                        let http_body = record["http_body"].as_str().unwrap_or("").to_string();
                        if !token_str.is_empty() && !http_body.is_empty() {
                            sqlx::query!(
                                "INSERT INTO acme_challenges (token, response)
                                 VALUES ($1, $2)
                                 ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response",
                                token_str,
                                http_body,
                            )
                            .execute(&state.db)
                            .await
                            .ok();
                        }

                        // check if we're serving it
                        let served = sqlx::query_scalar!(
                            "SELECT COUNT(*) FROM acme_challenges WHERE token = $1",
                            token_str,
                        )
                        .fetch_one(&state.db)
                        .await
                        .unwrap_or(Some(0))
                        .unwrap_or(0)
                            > 0;

                        acme_challenges.push(AcmeChallenge {
                            token: token_str,
                            url: http_url,
                            served,
                        });
                    }
                }
            }
        }
    }

    Ok(Json(DomainStatusDetail {
        id: row.id,
        domain: row.domain,
        status: row.status,
        cf_hostname_id: row.cf_hostname_id,
        ownership_txt_name: row.ownership_txt_name,
        ownership_txt_value: row.ownership_txt_value,
        ssl_status,
        ssl_ca,
        acme_challenges,
        verification_errors,
        created_at: row.created_at,
    }))
}
