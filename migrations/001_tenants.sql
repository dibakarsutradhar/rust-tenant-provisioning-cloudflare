-- migrations/001_tenants.sql
CREATE TABLE tenants (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    subdomain   TEXT NOT NULL UNIQUE,
    company     TEXT NOT NULL,
    status      TEXT NOT NULL DEFAULT 'pending',  -- pending | active | failed
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);