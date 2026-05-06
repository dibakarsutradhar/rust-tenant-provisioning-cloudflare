CREATE UNLOGGED TABLE rate_limits (
    key         TEXT NOT NULL,        -- e.g. "register:1.2.3.4"
    count       INT NOT NULL DEFAULT 1,
    window_start TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (key)
);