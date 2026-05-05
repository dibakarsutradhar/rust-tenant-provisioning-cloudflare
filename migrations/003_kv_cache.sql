-- migrations/003_kv_cache.sql
-- unlogged = no WAL writes, fast, data lost on crash (fine, it's just a cache)
CREATE UNLOGGED TABLE kv_cache (
    key         TEXT PRIMARY KEY,
    value       TEXT NOT NULL,
    expires_at  TIMESTAMPTZ
);

-- background cleanup of expired keys
CREATE OR REPLACE FUNCTION delete_expired_kv() RETURNS void AS $$
  DELETE FROM kv_cache WHERE expires_at < now();
$$ LANGUAGE sql;