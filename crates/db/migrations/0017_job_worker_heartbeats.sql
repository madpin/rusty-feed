-- Live worker registration for queue observability. Rows become stale after a
-- missed heartbeat, so a crashed process never looks permanently online.
CREATE TABLE job_worker_heartbeats (
    worker_id         TEXT PRIMARY KEY,
    concurrency       INT NOT NULL CHECK (concurrency > 0),
    scheduler_enabled BOOLEAN NOT NULL,
    started_at        TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_heartbeat_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE INDEX job_worker_heartbeats_recent_idx
    ON job_worker_heartbeats(last_heartbeat_at DESC);
