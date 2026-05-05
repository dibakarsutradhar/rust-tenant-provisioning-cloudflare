use sqlx::PgPool;
use uuid::Uuid;

use crate::db;

pub async fn run(db: PgPool, tenant_id: Uuid, subdomain: String) {
    tracing::info!("Provisioning started for {subdomain}");

    if let Err(e) = provision(db, tenant_id, subdomain.clone()).await {
        tracing::error!("Provisioning failed for {subdomain}: {e}");
    }
}

async fn provision(db: PgPool, tenant_id: Uuid, subdomain: String) -> Result<(), anyhow::Error> {
    // cloudflare DNS (mocked for MVP)
    let mock = std::env::var("MOCK_CLOUDFLARE").unwrap_or_default() == "true";
    if mock {
        tracing::info!("[mock] DNS record created for {subdomain}");
        // simulate network delay
        tokio::time::sleep(tokio::time::Duration::from_secs(2)).await;
    } else {
        create_cloudflare_dns(&subdomain).await?;
    }

    // SSL Certificate (mocked)
    if mock {
        tracing::info!("[mock] SSL cert issued for {subdomain}");
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    } else {
        create_cloudflare_cert(&subdomain).await?;
    }

    // warm kv_cache in postgres
    sqlx::query!(
        "INSERT INTO kv_cache (key, value, expires_at)
         VALUES ($1, $2, now() + interval '24 hours')
         ON CONFLICT (key) DO UPDATE SET value = $2, expires_at = now() + interval '24 hours'",
        format!("subdomain:{subdomain}"),
        tenant_id.to_string(),
    )
    .execute(&db)
    .await?;

    tracing::info!("KV cache warmed for {subdomain}");

    // mark tenant active
    db::set_tenant_active(&db, tenant_id).await?;
    tracing::info!("Tenant {tenant_id} marked active");

    // notify via postgres LISTEN/NOTIFY
    sqlx::query(&format!(
        "SELECT pg_notify('provisioning', '{tenant_id}:done')"
    ))
    .execute(&db)
    .await?;

    tracing::info!("Provisioning complete for {subdomain}");
    Ok(())
}

// placeholder — fill these in when moving to production
async fn create_cloudflare_dns(_subdomain: &str) -> Result<(), anyhow::Error> {
    todo!("implement Cloudflare DNS API call")
}

async fn create_cloudflare_cert(_subdomain: &str) -> Result<(), anyhow::Error> {
    todo!("implement Cloudflare TLS API call")
}
