use sqlx::PgPool;

use crate::config::Config;

pub async fn check_register(db: &PgPool, config: &Config, ip: &str) -> Result<(), String> {
    check(
        db,
        &format!("register:{ip}"),
        config.rate_limit_register_max,
        config.rate_limit_register_window,
    )
    .await
}

pub async fn check_login(db: &PgPool, config: &Config, ip: &str) -> Result<(), String> {
    check(
        db,
        &format!("login:{ip}"),
        config.rate_limit_login_max,
        config.rate_limit_login_window,
    )
    .await
}

async fn check(db: &PgPool, key: &str, max: i32, window_secs: i64) -> Result<(), String> {
    // atomic upsert — increment count or reset if window expired
    let count: i32 = sqlx::query_scalar(
        r#"
        INSERT INTO rate_limits (key, count, window_start)
        VALUES ($1, 1, now())
        ON CONFLICT (key) DO UPDATE SET
            count = CASE
                WHEN rate_limits.window_start < now() - make_interval(secs => $2)
                THEN 1
                ELSE rate_limits.count + 1
            END,
            window_start = CASE
                WHEN rate_limits.window_start < now() - make_interval(secs => $2)
                THEN now()
                ELSE rate_limits.window_start
            END
        RETURNING count
        "#,
    )
    .bind(key)
    .bind(window_secs as f64)
    .fetch_one(db)
    .await
    .map_err(|e| e.to_string())?;

    if count > max {
        return Err(format!(
            "too many attempts — try again in {} minutes",
            window_secs / 60
        ));
    }

    Ok(())
}
