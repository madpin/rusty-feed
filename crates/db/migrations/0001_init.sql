-- RustSS initial schema.
--
-- Design invariant: content is shared, state is personal (docs/ARCHITECTURE.md
-- §6, §7). Shared tables (feeds, articles, article_embeddings, ...) hold one row
-- regardless of how many users subscribe; per-user tables (user_feeds,
-- read_articles, bookmarks, article_feedback, user_patterns, saved_searches,
-- ...) layer personal state on top.

CREATE EXTENSION IF NOT EXISTS vector;

-- Shared trigger to maintain updated_at columns.
CREATE OR REPLACE FUNCTION set_updated_at() RETURNS trigger AS $$
BEGIN
    NEW.updated_at = now();
    RETURN NEW;
END;
$$ LANGUAGE plpgsql;

-- ── Accounts / auth ──────────────────────────────────────────────────────────
CREATE TABLE users (
    id                UUID PRIMARY KEY,
    email             TEXT NOT NULL UNIQUE,
    name              TEXT,
    image             TEXT,
    role              TEXT NOT NULL DEFAULT 'user' CHECK (role IN ('admin', 'user', 'guest')),
    password_hash     TEXT,
    email_verified_at TIMESTAMPTZ,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE oauth_accounts (
    id                  UUID PRIMARY KEY,
    user_id             UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    provider            TEXT NOT NULL,
    provider_account_id TEXT NOT NULL,
    access_token        TEXT,
    refresh_token       TEXT,
    expires_at          TIMESTAMPTZ,
    token_type          TEXT,
    scope               TEXT,
    id_token            TEXT,
    created_at          TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (provider, provider_account_id)
);
CREATE INDEX oauth_accounts_user_id_idx ON oauth_accounts(user_id);

-- App-specific tokens (native API + GReader ClientLogin); stored hashed.
CREATE TABLE api_tokens (
    id           UUID PRIMARY KEY,
    user_id      UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name         TEXT NOT NULL,
    token_hash   TEXT NOT NULL UNIQUE,
    scopes       TEXT[] NOT NULL DEFAULT '{}',
    last_used_at TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    revoked_at   TIMESTAMPTZ
);
CREATE INDEX api_tokens_user_id_idx ON api_tokens(user_id);

-- ── Feeds (SHARED) ───────────────────────────────────────────────────────────
CREATE TABLE feeds (
    id                       UUID PRIMARY KEY,
    url                      TEXT NOT NULL UNIQUE,          -- normalized; dedupes across users
    name                     TEXT NOT NULL,
    site_url                 TEXT,
    description              TEXT,
    image_url                TEXT,
    etag                     TEXT,                          -- conditional GET validators (FND-001)
    last_modified            TEXT,
    last_fetched_at          TIMESTAMPTZ,
    last_successful_fetch_at TIMESTAMPTZ,
    fetch_interval_minutes   INT NOT NULL DEFAULT 60,
    health_status            TEXT NOT NULL DEFAULT 'healthy'
                             CHECK (health_status IN ('healthy', 'warning', 'error', 'disabled')),
    consecutive_failures     INT NOT NULL DEFAULT 0,
    error_count              INT NOT NULL DEFAULT 0,
    last_error               TEXT,
    auto_disable_threshold   INT NOT NULL DEFAULT 10,
    notify_on_error          BOOLEAN NOT NULL DEFAULT false,
    http_status              INT,
    redirect_url             TEXT,
    extraction_settings      JSONB,
    respect_robots_txt       BOOLEAN NOT NULL DEFAULT true,
    extraction_delay_ms      INT,
    created_at               TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at               TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX feeds_health_status_idx ON feeds(health_status);
CREATE INDEX feeds_last_fetched_idx ON feeds(last_fetched_at);

CREATE TABLE feed_error_log (
    id            UUID PRIMARY KEY,
    feed_id       UUID NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    occurred_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    error_type    TEXT NOT NULL,
    error_message TEXT NOT NULL,
    http_status   INT,
    details       JSONB,
    resolved      BOOLEAN NOT NULL DEFAULT false
);
CREATE INDEX feed_error_log_feed_idx ON feed_error_log(feed_id);
CREATE INDEX feed_error_log_time_idx ON feed_error_log(occurred_at DESC);

-- Per-domain extraction politeness state (SHARED across all feeds/users).
CREATE TABLE domain_extraction_state (
    domain            TEXT PRIMARY KEY,
    last_extracted_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    failure_count     INT NOT NULL DEFAULT 0,
    rate_limit_count  INT NOT NULL DEFAULT 0,
    crawl_delay_ms    INT,
    last_error        TEXT,
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- ── Articles (SHARED) ────────────────────────────────────────────────────────
CREATE TABLE articles (
    id                UUID PRIMARY KEY,
    feed_id           UUID NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    url               TEXT NOT NULL UNIQUE,               -- global dedup key
    guid              TEXT,                                -- feed-scoped dedup key
    title             TEXT NOT NULL,
    author            TEXT,
    content           TEXT NOT NULL DEFAULT '',
    excerpt           TEXT,
    content_hash      TEXT,                                -- sha256(trim(content)) (FND dedup)
    image_url         TEXT,
    published_at      TIMESTAMPTZ,
    extraction_status TEXT NOT NULL DEFAULT 'none'
                      CHECK (extraction_status IN ('none', 'success', 'failed')),
    extraction_error  TEXT,
    summary           TEXT,                                -- shared enrichment (first-writer-wins)
    key_points        JSONB,
    topics            TEXT[] NOT NULL DEFAULT '{}',
    summarized_at     TIMESTAMPTZ,
    summary_model     TEXT,
    search_vector     tsvector GENERATED ALWAYS AS (
                          to_tsvector('english',
                              coalesce(title, '') || ' ' ||
                              coalesce(excerpt, '') || ' ' ||
                              coalesce(content, ''))
                      ) STORED,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX articles_feed_published_idx ON articles(feed_id, published_at DESC);
CREATE INDEX articles_guid_idx ON articles(guid);
CREATE INDEX articles_content_hash_idx ON articles(content_hash);
CREATE INDEX articles_published_idx ON articles(published_at DESC);
CREATE INDEX articles_search_idx ON articles USING gin (search_vector);

-- Single embedding space per instance (docs/ARCHITECTURE.md §13; FND-002).
-- Default dimension 384 matches the local BGE-small model. Switching models is
-- an explicit re-embed/migration; the `model` column guards against mismatches.
CREATE TABLE article_embeddings (
    article_id UUID PRIMARY KEY REFERENCES articles(id) ON DELETE CASCADE,
    embedding  vector(384) NOT NULL,
    model      TEXT NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX article_embeddings_hnsw_idx ON article_embeddings USING hnsw (embedding vector_cosine_ops);

-- ── Subscriptions & organization (PER-USER) ──────────────────────────────────
CREATE TABLE user_categories (
    id                UUID PRIMARY KEY,
    user_id           UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name              TEXT NOT NULL,
    parent_id         UUID REFERENCES user_categories(id) ON DELETE SET NULL,
    icon              TEXT DEFAULT '📁',
    color             TEXT,
    sort_order        INT NOT NULL DEFAULT 0,
    collapsed         BOOLEAN NOT NULL DEFAULT false,
    include_in_search BOOLEAN NOT NULL DEFAULT true,
    is_default        BOOLEAN NOT NULL DEFAULT false,
    is_read_only      BOOLEAN NOT NULL DEFAULT false,
    settings          JSONB,
    created_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, name)
);
CREATE INDEX user_categories_user_idx ON user_categories(user_id);

CREATE TABLE user_feeds (
    id            UUID PRIMARY KEY,
    user_id       UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    feed_id       UUID NOT NULL REFERENCES feeds(id) ON DELETE CASCADE,
    custom_name   TEXT,
    tags          TEXT[] NOT NULL DEFAULT '{}',
    settings      JSONB,
    subscribed_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, feed_id)
);
CREATE INDEX user_feeds_user_idx ON user_feeds(user_id);
CREATE INDEX user_feeds_feed_idx ON user_feeds(feed_id);
CREATE INDEX user_feeds_tags_idx ON user_feeds USING gin (tags);

CREATE TABLE user_feed_categories (
    id               UUID PRIMARY KEY,
    user_feed_id     UUID NOT NULL REFERENCES user_feeds(id) ON DELETE CASCADE,
    user_category_id UUID NOT NULL REFERENCES user_categories(id) ON DELETE CASCADE,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_feed_id, user_category_id)
);
CREATE INDEX user_feed_categories_cat_idx ON user_feed_categories(user_category_id);

-- ── Reading state & learning (PER-USER) ──────────────────────────────────────
CREATE TABLE read_articles (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    read_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, article_id)
);
CREATE INDEX read_articles_user_idx ON read_articles(user_id, read_at);

CREATE TABLE seen_articles (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    seen_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, article_id)
);

CREATE TABLE bookmarks (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    note       TEXT,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, article_id)
);
CREATE INDEX bookmarks_user_idx ON bookmarks(user_id);

CREATE TABLE article_feedback (
    id                     UUID PRIMARY KEY,
    user_id                UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    article_id             UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    feedback_type          TEXT NOT NULL CHECK (feedback_type IN ('explicit', 'implicit')),
    feedback_value         DOUBLE PRECISION NOT NULL,
    time_spent_seconds     INT,
    estimated_time_seconds INT,
    created_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at             TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, article_id)
);
CREATE INDEX article_feedback_user_idx ON article_feedback(user_id, created_at);

CREATE TABLE user_patterns (
    id             UUID PRIMARY KEY,
    user_id        UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    keyword        TEXT NOT NULL,
    weight         DOUBLE PRECISION NOT NULL,
    feedback_count INT NOT NULL DEFAULT 1,
    created_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (user_id, keyword)
);
CREATE INDEX user_patterns_user_weight_idx ON user_patterns(user_id, weight);

-- ── Saved searches (PER-USER) ────────────────────────────────────────────────
CREATE TABLE saved_searches (
    id               UUID PRIMARY KEY,
    user_id          UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    name             TEXT NOT NULL,
    query            TEXT NOT NULL,
    icon             TEXT DEFAULT '🔍',
    threshold        DOUBLE PRECISION NOT NULL DEFAULT 0.6,
    category_id      UUID REFERENCES user_categories(id) ON DELETE SET NULL,
    notify_on_match  BOOLEAN NOT NULL DEFAULT false,
    notify_threshold DOUBLE PRECISION NOT NULL DEFAULT 0.85,
    daily_digest     BOOLEAN NOT NULL DEFAULT false,
    recency_bias     DOUBLE PRECISION NOT NULL DEFAULT 0.0,
    priority_sources JSONB,
    last_matched_at  TIMESTAMPTZ,
    total_matches    INT NOT NULL DEFAULT 0,
    archived         BOOLEAN NOT NULL DEFAULT false,
    created_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at       TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX saved_searches_user_idx ON saved_searches(user_id, archived);

CREATE TABLE saved_search_matches (
    id              UUID PRIMARY KEY,
    saved_search_id UUID NOT NULL REFERENCES saved_searches(id) ON DELETE CASCADE,
    article_id      UUID NOT NULL REFERENCES articles(id) ON DELETE CASCADE,
    relevance_score DOUBLE PRECISION NOT NULL,
    matched_terms   JSONB,
    match_reason    TEXT,
    notified        BOOLEAN NOT NULL DEFAULT false,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (saved_search_id, article_id)
);
CREATE INDEX saved_search_matches_score_idx ON saved_search_matches(saved_search_id, relevance_score);
CREATE INDEX saved_search_matches_article_idx ON saved_search_matches(article_id);

-- ── Preferences & notifications (PER-USER) ───────────────────────────────────
-- Preferences are stored as a typed JSON document (deserialized into a Rust
-- struct in the web layer) to avoid churn from the reference's 30+ columns.
CREATE TABLE user_preferences (
    user_id     UUID PRIMARY KEY REFERENCES users(id) ON DELETE CASCADE,
    preferences JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TABLE user_notifications (
    id         UUID PRIMARY KEY,
    user_id    UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    kind       TEXT NOT NULL,
    title      TEXT NOT NULL,
    message    TEXT NOT NULL,
    metadata   JSONB,
    read       BOOLEAN NOT NULL DEFAULT false,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX user_notifications_created_idx ON user_notifications(user_id, created_at);
CREATE INDEX user_notifications_read_idx ON user_notifications(user_id, read);

-- ── System / ops ─────────────────────────────────────────────────────────────
CREATE TABLE admin_settings (
    key         TEXT PRIMARY KEY,
    value       JSONB NOT NULL,
    description TEXT,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- Durable cost accounting (replaces the reference's in-memory trackers; FND-003).
CREATE TABLE cost_ledger (
    id             UUID PRIMARY KEY,
    user_id        UUID REFERENCES users(id) ON DELETE SET NULL,
    provider       TEXT NOT NULL,
    model          TEXT NOT NULL,
    kind           TEXT NOT NULL CHECK (kind IN ('embedding', 'summary', 'digest')),
    input_tokens   BIGINT NOT NULL DEFAULT 0,
    output_tokens  BIGINT NOT NULL DEFAULT 0,
    cost_micro_usd BIGINT NOT NULL DEFAULT 0,
    occurred_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX cost_ledger_user_idx ON cost_ledger(user_id, occurred_at);
CREATE INDEX cost_ledger_time_idx ON cost_ledger(occurred_at);

-- AGENTS run history (docs/ARCHITECTURE.md §8).
CREATE TABLE agent_runs (
    id            UUID PRIMARY KEY,
    agent_name    TEXT NOT NULL,
    status        TEXT NOT NULL CHECK (status IN ('pending', 'running', 'success', 'failed')),
    trigger       TEXT NOT NULL CHECK (trigger IN ('scheduler', 'manual', 'system', 'event')),
    started_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    completed_at  TIMESTAMPTZ,
    duration_ms   BIGINT,
    stats         JSONB,
    logs          JSONB,
    error_message TEXT,
    metadata      JSONB
);
CREATE INDEX agent_runs_name_time_idx ON agent_runs(agent_name, started_at DESC);
CREATE INDEX agent_runs_name_status_idx ON agent_runs(agent_name, status);

-- Lease-based singleton locks for scheduled agents.
CREATE TABLE agent_locks (
    agent_name TEXT PRIMARY KEY,
    locked_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    locked_by  TEXT NOT NULL,
    expires_at TIMESTAMPTZ NOT NULL
);
CREATE INDEX agent_locks_expires_idx ON agent_locks(expires_at);

-- Durable work queue (SKIP LOCKED consumers).
CREATE TABLE job_queue (
    id           UUID PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      JSONB NOT NULL DEFAULT '{}',
    status       TEXT NOT NULL DEFAULT 'pending'
                 CHECK (status IN ('pending', 'running', 'done', 'failed', 'dead')),
    run_at       TIMESTAMPTZ NOT NULL DEFAULT now(),
    attempts     INT NOT NULL DEFAULT 0,
    max_attempts INT NOT NULL DEFAULT 5,
    locked_by    TEXT,
    locked_at    TIMESTAMPTZ,
    last_error   TEXT,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX job_queue_pending_idx ON job_queue(run_at) WHERE status = 'pending';

-- Transactional outbox: content writes emit events consumed by agents.
CREATE TABLE outbox_events (
    id           UUID PRIMARY KEY,
    kind         TEXT NOT NULL,
    payload      JSONB NOT NULL DEFAULT '{}',
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    processed_at TIMESTAMPTZ
);
CREATE INDEX outbox_unprocessed_idx ON outbox_events(created_at) WHERE processed_at IS NULL;

-- ── updated_at triggers ──────────────────────────────────────────────────────
DO $$
DECLARE
    t text;
BEGIN
    FOREACH t IN ARRAY ARRAY[
        'users', 'feeds', 'articles', 'domain_extraction_state', 'user_categories',
        'user_feeds', 'article_feedback', 'user_patterns', 'saved_searches',
        'user_preferences', 'user_notifications', 'admin_settings', 'job_queue'
    ]
    LOOP
        EXECUTE format(
            'CREATE TRIGGER %I_set_updated_at BEFORE UPDATE ON %I FOR EACH ROW EXECUTE FUNCTION set_updated_at()',
            t, t
        );
    END LOOP;
END $$;
