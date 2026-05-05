CREATE TABLE sessions (
    id          UUID PRIMARY KEY DEFAULT gen_random_uuid(),
    tenant_id   UUID NOT NULL REFERENCES tenants(id),
    user_id     UUID NOT NULL REFERENCES users(id),
    ip          TEXT,
    country     TEXT,
    city        TEXT,
    user_agent  TEXT,
    browser     TEXT,
    os          TEXT,
    device      TEXT,
    cf_ray      TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_seen   TIMESTAMPTZ NOT NULL DEFAULT now()
);