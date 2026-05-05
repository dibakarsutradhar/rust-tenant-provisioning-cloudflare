use anyhow::anyhow;
use sqlx::PgPool;
use uuid::Uuid;

pub async fn add_custom_domain(
    db: &PgPool,
    tenant_id: Uuid,
    domain: String,
) -> Result<Uuid, anyhow::Error> {
    // check not already taken
    let exists = sqlx::query_scalar!(
        "SELECT COUNT(*) FROM custom_domains WHERE domain = $1",
        domain
    )
    .fetch_one(db)
    .await?
    .unwrap_or(0);

    if exists > 0 {
        return Err(anyhow!("domain already registered"));
    }

    // insert as pending
    let id = sqlx::query_scalar!(
        "INSERT INTO custom_domains (tenant_id, domain, status)
         VALUES ($1, $2, 'pending')
         RETURNING id",
        tenant_id,
        domain,
    )
    .fetch_one(db)
    .await?;

    // spawn background verification + provisioning task
    let db2 = db.clone();
    let domain2 = domain.clone();
    tokio::spawn(async move {
        if let Err(e) = verify_and_provision(db2, id, domain2).await {
            tracing::error!("Custom domain provisioning failed for {domain}: {e}");
            // mark failed
        }
    });

    Ok(id)
}

async fn verify_and_provision(
    db: PgPool,
    custom_domain_id: Uuid,
    domain: String,
) -> Result<(), anyhow::Error> {
    let tunnel_id = std::env::var("CLOUDFLARE_TUNNEL_ID")?;
    let expected_cname = format!("{tunnel_id}.cfargotunnel.com");

    tracing::info!("Waiting for CNAME on {domain} → {expected_cname}");

    // poll for CNAME — every 30s, max 40 attempts (20 minutes)
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 40 {
            sqlx::query!(
                "UPDATE custom_domains SET status = 'failed' WHERE id = $1",
                custom_domain_id
            )
            .execute(&db)
            .await?;

            notify(&db, custom_domain_id, "failed:timeout").await?;
            return Err(anyhow!("CNAME verification timed out for {domain}"));
        }

        if cname_resolves(&domain, &expected_cname).await {
            tracing::info!("CNAME verified for {domain} after {attempts} attempts");
            break;
        }

        tracing::info!("CNAME not yet live for {domain}, attempt {attempts}/20, retrying in 30s");
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }

    // CNAME is live — create Cloudflare custom hostname for SSL
    notify(&db, custom_domain_id, "status:cname_verified").await?;

    let cf_hostname_id = create_cf_custom_hostname(&db, &domain).await?;
    tracing::info!("CF custom hostname created: {cf_hostname_id}");

    sqlx::query!(
        "UPDATE custom_domains SET status = 'ssl_pending', cf_hostname_id = $1 WHERE id = $2",
        cf_hostname_id,
        custom_domain_id
    )
    .execute(&db)
    .await?;

    notify(&db, custom_domain_id, "status:ssl_pending").await?;

    // poll CF until SSL is active — every 15s, max 20 attempts (5 minutes)
    let mut attempts = 0;
    loop {
        attempts += 1;
        if attempts > 20 {
            sqlx::query!(
                "UPDATE custom_domains SET status = 'failed' WHERE id = $1",
                custom_domain_id
            )
            .execute(&db)
            .await?;
            notify(&db, custom_domain_id, "failed:ssl_timeout").await?;
            return Err(anyhow!("SSL provisioning timed out for {domain}"));
        }

        let ssl_active = check_cf_ssl_status(&cf_hostname_id).await?;
        if ssl_active {
            tracing::info!("SSL active for {domain}");
            break;
        }

        tracing::info!("SSL pending for {domain}, attempt {attempts}/20, retrying in 15s");
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
    }

    // warm kv cache so tenant middleware resolves this domain
    let tenant_id = sqlx::query_scalar!(
        "SELECT tenant_id FROM custom_domains WHERE id = $1",
        custom_domain_id
    )
    .fetch_one(&db)
    .await?;

    sqlx::query!(
        "INSERT INTO kv_cache (key, value, expires_at)
         VALUES ($1, $2, now() + interval '24 hours')
         ON CONFLICT (key) DO UPDATE
         SET value = EXCLUDED.value, expires_at = now() + interval '24 hours'",
        format!("custom:{domain}"),
        tenant_id.to_string(),
    )
    .execute(&db)
    .await?;

    // mark active
    sqlx::query!(
        "UPDATE custom_domains SET status = 'active' WHERE id = $1",
        custom_domain_id
    )
    .execute(&db)
    .await?;

    notify(&db, custom_domain_id, &format!("done:{domain}")).await?;
    tracing::info!("Custom domain {domain} fully active");
    Ok(())
}

async fn cname_resolves(domain: &str, _expected: &str) -> bool {
    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .unwrap_or_default();

    // try hitting the domain — if CF is routing it we'll get any response
    // (even 404 means it's reaching our server)
    match client.get(format!("https://{domain}/health")).send().await {
        Ok(res) => {
            tracing::info!("Domain {domain} reachable, status: {}", res.status());
            true
        }
        Err(e) => {
            tracing::info!("Domain {domain} not yet reachable: {e}");
            false
        }
    }
}

async fn create_cf_custom_hostname(db: &PgPool, domain: &str) -> Result<String, anyhow::Error> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN")?;
    let zone_id = std::env::var("CLOUDFLARE_ZONE_ID")?;

    let url = format!("https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames");

    let body = serde_json::json!({
        "hostname": domain,
        "ssl": {
            "method": "http",
            "type": "dv",
            "settings": {
                "min_tls_version": "1.2"
            }
        }
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

    tracing::info!("CF custom hostname response {status}: {json}");

    if !status.is_success() {
        // 1406 = hostname already exists — get its id
        let already_exists = json["errors"]
            .as_array()
            .map(|e| e.iter().any(|e| e["code"] == 1406))
            .unwrap_or(false);

        if already_exists {
            tracing::warn!("Custom hostname already exists for {domain}");
            return get_cf_custom_hostname_id(domain).await;
        }

        return Err(anyhow!("CF custom hostname error {status}: {json}"));
    }

    let id = json["result"]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("no id in CF response"))?
        .to_string();

    // store HTTP challenge token + response so our server can serve it
    if let Some(http) = json["result"]["ssl"]["http_url"].as_str() {
        // http_url looks like: http://domain/.well-known/acme-challenge/TOKEN
        if let Some(token_str) = http.split('/').last() {
            let challenge_response = json["result"]["ssl"]["http_body"]
                .as_str()
                .unwrap_or("")
                .to_string();

            if !token_str.is_empty() && !challenge_response.is_empty() {
                sqlx::query!(
                    "INSERT INTO acme_challenges (token, response)
                     VALUES ($1, $2)
                     ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response",
                    token_str,
                    challenge_response,
                )
                .execute(db)
                .await?;

                tracing::info!("Stored ACME challenge for token: {token_str}");
            }
        }
    }

    Ok(id)
}

async fn get_cf_custom_hostname_id(domain: &str) -> Result<String, anyhow::Error> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN")?;
    let zone_id = std::env::var("CLOUDFLARE_ZONE_ID")?;

    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames?hostname={domain}"
    );

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    let json: serde_json::Value = res.json().await?;

    let id = json["result"][0]["id"]
        .as_str()
        .ok_or_else(|| anyhow!("custom hostname not found"))?
        .to_string();

    Ok(id)
}

async fn check_cf_ssl_status(cf_hostname_id: &str) -> Result<bool, anyhow::Error> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN")?;
    let zone_id = std::env::var("CLOUDFLARE_ZONE_ID")?;

    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames/{cf_hostname_id}"
    );

    let client = reqwest::Client::new();
    let res = client
        .get(&url)
        .header("Authorization", format!("Bearer {token}"))
        .send()
        .await?;

    let json: serde_json::Value = res.json().await?;
    let ssl_status = json["result"]["ssl"]["status"].as_str().unwrap_or("");

    tracing::info!("SSL status for {cf_hostname_id}: {ssl_status}");
    Ok(ssl_status == "active")
}

async fn notify(db: &PgPool, id: Uuid, payload: &str) -> Result<(), anyhow::Error> {
    let channel = format!("domain_{}", id.to_string().replace('-', "_"));
    sqlx::query!("SELECT pg_notify($1, $2)", channel, payload,)
        .execute(db)
        .await?;
    Ok(())
}
