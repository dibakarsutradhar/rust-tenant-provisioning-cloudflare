use sqlx::PgPool;
use uuid::Uuid;

pub async fn get_tenant_id_by_subdomain(
    db: &PgPool,
    subdomain: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    let row = sqlx::query_scalar!(
        "SELECT id FROM tenants WHERE subdomain = $1 AND status = 'active'",
        subdomain
    )
    .fetch_optional(db)
    .await?;

    Ok(row)
}
