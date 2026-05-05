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

pub async fn get_tenant_id_by_custom_domain(
    db: &PgPool,
    domain: &str,
) -> Result<Option<Uuid>, sqlx::Error> {
    sqlx::query_scalar!(
        "SELECT tenant_id FROM custom_domains WHERE domain = $1 AND status = 'active'",
        domain
    )
    .fetch_optional(db)
    .await
}

pub async fn insert_tenant(
    db: &PgPool,
    subdomain: &str,
    company: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = sqlx::query_scalar!(
        "INSERT INTO tenants (subdomain, company, status)
         VALUES ($1, $2, 'pending')
         RETURNING id",
        subdomain,
        company,
    )
    .fetch_one(db)
    .await?;

    Ok(id)
}

pub async fn insert_user(
    db: &PgPool,
    tenant_id: Uuid,
    email: &str,
    password_hash: &str,
) -> Result<Uuid, sqlx::Error> {
    let id = sqlx::query_scalar!(
        "INSERT INTO users (tenant_id, email, password_hash, role)
         VALUES ($1, $2, $3, 'owner')
         RETURNING id",
        tenant_id,
        email,
        password_hash,
    )
    .fetch_one(db)
    .await?;

    Ok(id)
}

pub async fn set_tenant_active(db: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE tenants SET status = 'active' WHERE id = $1",
        tenant_id
    )
    .execute(db)
    .await?;

    Ok(())
}

pub async fn set_tenant_failed(db: &PgPool, tenant_id: Uuid) -> Result<(), sqlx::Error> {
    sqlx::query!(
        "UPDATE tenants SET status = 'failed' WHERE id = $1",
        tenant_id
    )
    .execute(db)
    .await?;

    Ok(())
}
