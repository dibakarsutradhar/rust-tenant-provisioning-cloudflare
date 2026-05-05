-- migrations/004_custom_domains.sql
CREATE TABLE custom_domains (
    id              UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id       UUID NOT NULL REFERENCES tenants(id),
    domain          TEXT NOT NULL UNIQUE,
    status          TEXT NOT NULL DEFAULT 'pending',  -- pending | ssl_pending | active | failed
    cf_hostname_id  TEXT,                              -- cloudflare custom hostname id
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);