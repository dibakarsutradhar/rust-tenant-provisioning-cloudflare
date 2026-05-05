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

    tracing::info!("Step 1: waiting for CNAME on {domain}");

    // step 1 — poll for CNAME (grey cloud, so dig returns actual CNAME)
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
            notify(&db, custom_domain_id, "failed:cname_timeout").await?;
            return Err(anyhow!("CNAME verification timed out for {domain}"));
        }

        if cname_resolves(&domain, &expected_cname).await {
            tracing::info!("CNAME verified for {domain} after {attempts} attempts");
            break;
        }

        tracing::info!("CNAME not yet live for {domain}, attempt {attempts}/40, retrying in 30s");
        notify(&db, custom_domain_id, "status:waiting_cname").await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }

    notify(&db, custom_domain_id, "status:cname_verified").await?;

    // step 2 — create CF custom hostname, gets ownership TXT
    let cf_hostname_id = create_cf_custom_hostname(&db, custom_domain_id, &domain).await?;

    sqlx::query!(
        "UPDATE custom_domains SET status = 'ssl_pending', cf_hostname_id = $1 WHERE id = $2",
        cf_hostname_id,
        custom_domain_id
    )
    .execute(&db)
    .await?;

    // fetch the ownership TXT value we just stored
    let ownership_txt_value = sqlx::query_scalar!(
        "SELECT ownership_txt_value FROM custom_domains WHERE id = $1",
        custom_domain_id
    )
    .fetch_one(&db)
    .await?
    .unwrap_or_default();

    let ownership_txt_name = sqlx::query_scalar!(
        "SELECT ownership_txt_name FROM custom_domains WHERE id = $1",
        custom_domain_id
    )
    .fetch_one(&db)
    .await?
    .unwrap_or_default();

    // notify user with the two records they need to add
    notify(
        &db,
        custom_domain_id,
        &format!("records:{ownership_txt_name}={ownership_txt_value}"),
    )
    .await?;

    tracing::info!("Step 2: waiting for TXT ownership record on {domain}");

    // step 3 — poll TXT record using CF DNS directly (bypasses local cache)
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
            notify(&db, custom_domain_id, "failed:txt_timeout").await?;
            return Err(anyhow!("TXT ownership verification timed out for {domain}"));
        }

        if txt_record_verified(&domain, &ownership_txt_value).await {
            tracing::info!("TXT ownership verified for {domain} after {attempts} attempts");
            break;
        }

        tracing::info!("TXT not yet live for {domain}, attempt {attempts}/40, retrying in 30s");
        notify(&db, custom_domain_id, "status:waiting_txt").await?;
        tokio::time::sleep(tokio::time::Duration::from_secs(30)).await;
    }

    notify(&db, custom_domain_id, "status:txt_verified").await?;

    // step 4 — trigger CF recheck now that TXT is live
    trigger_cf_recheck(&cf_hostname_id).await?;

    // step 5 — poll SSL status
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
            notify(&db, custom_domain_id, "failed:ssl_timeout").await?;
            return Err(anyhow!("SSL provisioning timed out for {domain}"));
        }

        let ssl_active = check_cf_ssl_status(&cf_hostname_id).await?;
        if ssl_active {
            tracing::info!("SSL active for {domain}");
            break;
        }

        tracing::info!("SSL pending for {domain}, attempt {attempts}/40");
        tokio::time::sleep(tokio::time::Duration::from_secs(15)).await;
    }

    notify(&db, custom_domain_id, "status:ssl_active").await?;

    // step 6 — warm cache + mark active
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

async fn cname_resolves(domain: &str, expected: &str) -> bool {
    let output = std::process::Command::new("dig")
        .args(["+short", "CNAME", domain, "@1.1.1.1"])
        .output();

    match output {
        Ok(out) => {
            let result = String::from_utf8_lossy(&out.stdout);
            let resolved = result.trim().trim_end_matches('.');
            tracing::debug!("dig CNAME {domain} @1.1.1.1 → '{resolved}'");
            resolved == expected
        }
        Err(e) => {
            tracing::error!("dig failed: {e}");
            false
        }
    }
}

async fn txt_record_verified(domain: &str, expected_value: &str) -> bool {
    // extract base domain for TXT record name
    // demo-gos.dibakar.me → _cf-custom-hostname.demo-gos.dibakar.me
    let txt_name = format!("_cf-custom-hostname.{domain}");

    let output = std::process::Command::new("dig")
        .args(["+short", "TXT", &txt_name, "@1.1.1.1"])
        .output();

    match output {
        Ok(out) => {
            let result = String::from_utf8_lossy(&out.stdout);
            tracing::debug!("dig TXT {txt_name} @1.1.1.1 → '{}'", result.trim());
            result.contains(expected_value)
        }
        Err(e) => {
            tracing::error!("dig failed: {e}");
            false
        }
    }
}

async fn trigger_cf_recheck(cf_hostname_id: &str) -> Result<(), anyhow::Error> {
    let token = std::env::var("CLOUDFLARE_API_TOKEN")?;
    let zone_id = std::env::var("CLOUDFLARE_ZONE_ID")?;

    let url = format!(
        "https://api.cloudflare.com/client/v4/zones/{zone_id}/custom_hostnames/{cf_hostname_id}"
    );

    let client = reqwest::Client::new();
    client
        .patch(&url)
        .header("Authorization", format!("Bearer {token}"))
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({}))
        .send()
        .await?;

    tracing::info!("Triggered CF recheck for {cf_hostname_id}");
    Ok(())
}

async fn create_cf_custom_hostname(
    db: &PgPool,
    custom_domain_id: Uuid,
    domain: &str,
) -> Result<String, anyhow::Error> {
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

    // store ownership TXT value so we can show it to the user
    // and poll for it to verify ownership
    let ownership_txt_value = json["result"]["ownership_verification"]["value"]
        .as_str()
        .unwrap_or("")
        .to_string();

    let ownership_txt_name = json["result"]["ownership_verification"]["name"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if !ownership_txt_value.is_empty() {
        sqlx::query!(
            "UPDATE custom_domains 
             SET ownership_txt_name = $1, ownership_txt_value = $2 
             WHERE id = $3",
            ownership_txt_name,
            ownership_txt_value,
            custom_domain_id,
        )
        .execute(db)
        .await?;

        tracing::info!("Stored ownership TXT: {ownership_txt_name} = {ownership_txt_value}");
    }

    // also store cf-custom-hostname-challenge for HTTP ownership verification
    // CF uses a different path than ACME: /.well-known/cf-custom-hostname-challenge/{id}
    let http_challenge_body = json["result"]["ownership_verification_http"]["http_body"]
        .as_str()
        .unwrap_or("")
        .to_string();

    if !http_challenge_body.is_empty() {
        // token is the custom hostname id itself
        sqlx::query!(
            "INSERT INTO acme_challenges (token, response)
             VALUES ($1, $2)
             ON CONFLICT (token) DO UPDATE SET response = EXCLUDED.response",
            id, // the CF hostname id is the token in the URL
            http_challenge_body,
        )
        .execute(db)
        .await?;

        tracing::info!("Stored CF hostname challenge token: {id}");
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
