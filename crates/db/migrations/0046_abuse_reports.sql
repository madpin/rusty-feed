-- User abuse reports → admin review queue (docs/PUBLIC_LAUNCH_PLAN.md §4.6,
-- FND-192). Because content is shared, one abuser's feed/article is everyone's,
-- so users need a path to flag it and admins a queue to act on.
CREATE TABLE abuse_reports (
    id           UUID PRIMARY KEY,
    reporter_id  UUID REFERENCES users(id) ON DELETE SET NULL,
    target_kind  TEXT NOT NULL CHECK (target_kind IN ('article', 'feed')),
    target_id    UUID NOT NULL,
    target_url   TEXT,
    reason       TEXT NOT NULL,
    status       TEXT NOT NULL DEFAULT 'open' CHECK (status IN ('open', 'reviewing', 'resolved', 'dismissed')),
    resolved_by  UUID REFERENCES users(id) ON DELETE SET NULL,
    resolved_at  TIMESTAMPTZ,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX abuse_reports_status_idx ON abuse_reports (status, created_at DESC);
-- One open report per reporter+target keeps the queue readable and rate-limits
-- duplicate spam reports at the database layer.
CREATE UNIQUE INDEX abuse_reports_open_unique_idx
    ON abuse_reports (reporter_id, target_kind, target_id)
    WHERE status = 'open';
