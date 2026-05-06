use axum::{Extension, Json};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::{db, error::AppError, middleware::tenant::TenantContext, services, state::AppState};

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
    headers: axum::http::HeaderMap,
    Json(body): Json<RegisterRequest>,
) -> Result<Json<RegisterResponse>, AppError> {
    // get IP from Cloudflare header
    let ip = headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    // rate limit — 5 registrations per IP per hour
    crate::services::rate_limit::check_register(&state.db, &state.config, &ip)
        .await
        .map_err(|e| AppError::RateLimit(e))?;

    // basic validation
    let subdomain = body.subdomain.trim().to_lowercase();
    if subdomain.is_empty() || subdomain.contains('.') {
        return Err(AppError::BadRequest("invalid subdomain".into()));
    }

    if BLOCKED_SUBDOMAINS.contains(&subdomain.as_str()) {
        return Err(AppError::BadRequest("subdomain not available".into()));
    }

    // also block subdomains shorter than 3 chars or longer than 32
    if subdomain.len() < 3 || subdomain.len() > 32 {
        return Err(AppError::BadRequest(
            "subdomain must be between 3 and 32 characters".into(),
        ));
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
    let config = state.config.clone();
    tokio::spawn(async move {
        crate::services::provisioning::run(db_clone, config, tenant_id, subdomain).await;
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
    headers: axum::http::HeaderMap,
    Json(body): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, AppError> {
    let ip = headers
        .get("cf-connecting-ip")
        .or_else(|| headers.get("x-forwarded-for"))
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string());

    crate::services::rate_limit::check_login(&state.db, &state.config, &ip)
        .await
        .map_err(|e| AppError::RateLimit(e))?;

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

    // extract request metadata
    let ip = headers
        .get("cf-connecting-ip") // real IP from Cloudflare
        .or_else(|| headers.get("x-forwarded-for")) // fallback
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim().to_string());

    let country = headers
        .get("cf-ipcountry")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let city = headers
        .get("cf-ipcity")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let cf_ray = headers
        .get("cf-ray")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let user_agent = headers
        .get("user-agent")
        .and_then(|v| v.to_str().ok())
        .map(|s| s.to_string());

    let (browser, os, device) = parse_ua(user_agent.as_deref().unwrap_or(""));

    // store session
    sqlx::query!(
        "INSERT INTO sessions (tenant_id, user_id, ip, country, city, user_agent, browser, os, device, cf_ray)
         VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        user.tenant_id,
        user.id,
        ip,
        country,
        city,
        user_agent,
        browser,
        os,
        device,
        cf_ray,
    )
    .execute(&state.db)
    .await?;

    // issue JWT
    let token = services::jwt::issue(&state.config, user.id, user.tenant_id, &user.role)
        .map_err(|e| AppError::BadRequest(e.to_string()))?;

    Ok(Json(LoginResponse {
        token,
        tenant_id: user.tenant_id,
        role: user.role,
    }))
}

fn parse_ua(ua: &str) -> (String, String, String) {
    // browser
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome/") && !ua.contains("Chromium") {
        "Chrome"
    } else if ua.contains("Firefox/") {
        "Firefox"
    } else if ua.contains("Safari/") && !ua.contains("Chrome") {
        "Safari"
    } else if ua.contains("OPR/") || ua.contains("Opera/") {
        "Opera"
    } else {
        "Unknown"
    };

    // OS
    let os = if ua.contains("Windows NT") {
        "Windows"
    } else if ua.contains("Mac OS X") {
        "macOS"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("iPhone") || ua.contains("iPad") {
        "iOS"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Unknown"
    };

    // device
    let device = if ua.contains("Mobile") || ua.contains("Android") || ua.contains("iPhone") {
        "Mobile"
    } else if ua.contains("iPad") || ua.contains("Tablet") {
        "Tablet"
    } else {
        "Desktop"
    };

    (browser.to_string(), os.to_string(), device.to_string())
}

// ── me ──────────────────────────────────────────────────────────────────

#[derive(Serialize)]
pub struct MeResponse {
    pub tenant_id: uuid::Uuid,
    pub user_id: uuid::Uuid,
    pub role: String,
    pub last_session: Option<SessionInfo>,
}

#[derive(Serialize)]
pub struct SessionInfo {
    pub ip: Option<String>,
    pub country: Option<String>,
    pub city: Option<String>,
    pub browser: Option<String>,
    pub os: Option<String>,
    pub device: Option<String>,
    pub cf_ray: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
}

pub async fn me(
    Extension(state): Extension<AppState>,
    Extension(ctx): Extension<TenantContext>,
    Extension(claims): Extension<crate::services::jwt::Claims>,
) -> Result<Json<MeResponse>, AppError> {
    let session = sqlx::query_as!(
        SessionInfo,
        "SELECT ip, country, city, browser, os, device, cf_ray, created_at
         FROM sessions
         WHERE user_id = $1
         ORDER BY created_at DESC
         LIMIT 1",
        claims.sub,
    )
    .fetch_optional(&state.db)
    .await?;

    Ok(Json(MeResponse {
        tenant_id: ctx.tenant_id,
        user_id: claims.sub,
        role: claims.role.clone(),
        last_session: session,
    }))
}

const BLOCKED_SUBDOMAINS: &[&str] = &[
    // system
    "app",
    "api",
    "admin",
    "www",
    "mail",
    "smtp",
    "ftp",
    "ssh",
    "dev",
    "staging",
    "prod",
    "production",
    "test",
    "demo",
    "static",
    "assets",
    "cdn",
    "media",
    "img",
    "images",
    "status",
    "health",
    "metrics",
    "monitor",
    "dashboard",
    "auth",
    "login",
    "logout",
    "signup",
    "register",
    "billing",
    "pay",
    "payment",
    "invoice",
    "pricing",
    "docs",
    "help",
    "support",
    "blog",
    "about",
    "contact",
    "careers",
    "jobs",
    "legal",
    "privacy",
    "terms",
    // brands
    "google",
    "apple",
    "microsoft",
    "amazon",
    "facebook",
    "meta",
    "twitter",
    "instagram",
    "github",
    "stripe",
    // your own
    "thegarageos",
    "garageos",
    "garage",
];
