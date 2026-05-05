use anyhow::anyhow;
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

    // SSL (not needed with Cloudflare Tunnel — proxied CNAMEs get SSL automatically)
    if mock {
        tracing::info!("[mock] SSL cert issued for {subdomain}");
        tokio::time::sleep(tokio::time::Duration::from_secs(1)).await;
    }

    // warm kv_cache in postgres
    sqlx::query!(
        "INSERT INTO kv_cache (key, value, expires_at)
         VALUES ($1, $2, now() + interval '24 hours')
         ON CONFLICT (key) DO UPDATE
         SET value = EXCLUDED.value, expires_at = now() + interval '24 hours'",
        format!("subdomain:{subdomain}"),
        tenant_id.to_string(),
    )
    .execute(&db)
    .await?;
    tracing::info!("KV cache warmed for {subdomain}");

    // mark tenant active
    db::set_tenant_active(&db, tenant_id).await?;
    tracing::info!("Tenant {tenant_id} marked active");

    // notify SSE handler (handler/provisioning.rs) via postgres LISTEN/NOTIFY
    // channel must match what status_stream() listens on: tenant_{id_with_underscores}
    let channel = format!("tenant_{}", tenant_id.to_string().replace('-', "_"));
    sqlx::query!(
        "SELECT pg_notify($1, $2)",
        channel,
        format!("done:{subdomain}"),
    )
    .execute(&db)
    .await?;

    tracing::info!("Provisioning complete for {subdomain}");
    Ok(())
}

async fn create_cloudflare_dns(subdomain: &str) -> Result<(), anyhow::Error> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN")?;
    let zone_id = std::env::var("CLOUDFLARE_ZONE_ID")?;
    let tunnel_id = std::env::var("CLOUDFLARE_TUNNEL_ID")?;
    let base_domain =
        std::env::var("BASE_DOMAIN").unwrap_or_else(|_| "thegarageos.com".to_string());

    let fqdn = format!("{subdomain}.{base_domain}");
    let url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/dns_records");

    let body = serde_json::json!({
        "type":    "CNAME",
        "name":    fqdn,
        "content": format!("{tunnel_id}.cfargotunnel.com"),
        "ttl":     3600,
        "proxied": true,
        "comment": format!("garageos tenant: {subdomain}")
    });

    let client = reqwest::Client::new();
    let res = client
        .post(&url)
        .header("Content-Type", "application/json")
        .header("Authorization", format!("Bearer {token}"))
        .json(&body)
        .send()
        .await?;

    let status = res.status();
    let json: serde_json::Value = res.json().await?;

    tracing::info!("Cloudflare response {status}: {json}");

    if !status.is_success() {
        // code 81057 = record already exists — fine, continue
        let already_exists = json["errors"]
            .as_array()
            .map(|errs| errs.iter().any(|e| e["code"] == 81057))
            .unwrap_or(false);

        if already_exists {
            tracing::warn!("DNS record already exists for {fqdn}, continuing");
            return Ok(());
        }

        return Err(anyhow!("Cloudflare API error {status}: {json}"));
    }

    tracing::info!("DNS record created for {fqdn}");
    Ok(())
}
