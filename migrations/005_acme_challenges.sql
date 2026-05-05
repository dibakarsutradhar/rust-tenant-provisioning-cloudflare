CREATE TABLE acme_challenges (
    token       TEXT PRIMARY KEY,
    response    TEXT NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);