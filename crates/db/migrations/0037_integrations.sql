-- Personal outbound destinations and durable delivery history.
CREATE TABLE integration_profiles (
    id                    UUID PRIMARY KEY,
    user_id               UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name                  TEXT NOT NULL,
    provider              TEXT NOT NULL,
    endpoint_url          TEXT NOT NULL,
    credential_ciphertext TEXT,
    credential_kind       TEXT NOT NULL DEFAULT 'bearer'
                          CHECK (credential_kind IN ('none', 'bearer', 'basic', 'hmac')),
    payload_template      TEXT NOT NULL DEFAULT '{{title}}\n{{url}}',
    tags                  TEXT[] NOT NULL DEFAULT '{}',
    event_types           TEXT[] NOT NULL DEFAULT '{article.saved}',
    max_attempts          INT NOT NULL DEFAULT 5 CHECK (max_attempts BETWEEN 1 AND 12),
    rate_limit_per_minute INT NOT NULL DEFAULT 30 CHECK (rate_limit_per_minute BETWEEN 1 AND 600),
    is_default            BOOLEAN NOT NULL DEFAULT false,
    enabled               BOOLEAN NOT NULL DEFAULT true,
    paused_at             TIMESTAMPTZ,
    created_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at            TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
CREATE UNIQUE INDEX integration_profiles_one_default_idx
    ON integration_profiles(user_id) WHERE is_default AND enabled AND paused_at IS NULL;
CREATE INDEX integration_profiles_user_idx ON integration_profiles(user_id, created_at DESC);

CREATE TABLE integration_deliveries (
    id              UUID PRIMARY KEY,
    profile_id      UUID NOT NULL REFERENCES integration_profiles(id) ON DELETE CASCADE,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id      UUID REFERENCES articles(id) ON DELETE SET NULL,
    event_type      TEXT NOT NULL,
    payload_version INT NOT NULL DEFAULT 1,
    status          TEXT NOT NULL DEFAULT 'queued'
                    CHECK (status IN ('queued', 'delivering', 'delivered', 'retrying', 'dead')),
    attempt_count   INT NOT NULL DEFAULT 0,
    response_status INT,
    error_summary   TEXT,
    next_attempt_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    finished_at     TIMESTAMPTZ
);
CREATE INDEX integration_deliveries_due_idx
    ON integration_deliveries(status, next_attempt_at) WHERE status IN ('queued', 'retrying');
CREATE INDEX integration_deliveries_user_idx
    ON integration_deliveries(user_id, created_at DESC);
