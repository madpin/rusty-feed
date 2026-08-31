-- Feed blacklist + moderation audit (docs/PUBLIC_LAUNCH_PLAN.md §5, FND-192).
--
-- Admins block feeds by domain (suffix match) or exact normalized URL; blocked
-- feeds cannot be subscribed to via any path and existing shared feed rows are
-- disabled. The same list also blocks article ingest by URL/domain, so a global
-- article takedown can prevent re-ingestion (§4.6).
CREATE TABLE feed_blocklist (
    id          UUID PRIMARY KEY,
    kind        TEXT NOT NULL CHECK (kind IN ('domain', 'url')),
    pattern     TEXT NOT NULL,
    reason      TEXT,
    source      TEXT NOT NULL DEFAULT 'admin' CHECK (source IN ('admin', 'default')),
    created_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (kind, pattern)
);
CREATE INDEX feed_blocklist_kind_idx ON feed_blocklist (kind);

-- Moderation audit (who/what/why), mirroring acquisition_profile_audit (0040).
CREATE TABLE moderation_audit (
    id          UUID PRIMARY KEY,
    actor_id    UUID REFERENCES users(id) ON DELETE SET NULL,
    action      TEXT NOT NULL,
    target_kind TEXT,
    target      TEXT,
    reason      TEXT,
    detail      JSONB NOT NULL DEFAULT '{}',
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX moderation_audit_created_idx ON moderation_audit (created_at DESC);

-- Dedicated block markers on shared feeds (not overloading health_status).
ALTER TABLE feeds
    ADD COLUMN blocked_at     TIMESTAMPTZ,
    ADD COLUMN blocked_reason TEXT;

-- Shipped, curated default blocklist (source='default'; idempotent). Kept small
-- and defensible: purpose-built malware/phishing TEST domains that can never be
-- a legitimate publisher. Operators extend the list via /admin/feeds/blocklist.
-- Provenance: Google Safe Browsing test domains + reserved .test names.
INSERT INTO feed_blocklist (id, kind, pattern, reason, source) VALUES
    (gen_random_uuid(), 'domain', 'testsafebrowsing.appspot.com', 'Safe Browsing malware test domain', 'default'),
    (gen_random_uuid(), 'domain', 'malware.testing.google.test', 'reserved malware test domain', 'default'),
    (gen_random_uuid(), 'domain', 'phishing.testing.google.test', 'reserved phishing test domain', 'default')
ON CONFLICT (kind, pattern) DO NOTHING;
