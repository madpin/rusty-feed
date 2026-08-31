-- Reserve worker capacity for user-triggered work that must begin promptly.
-- Shared enrichment still deduplicates by article revision; the lane changes
-- scheduling only, never ownership or storage semantics.
ALTER TABLE job_queue
    ADD COLUMN queue_name TEXT NOT NULL DEFAULT 'background'
        CHECK (queue_name IN ('background', 'interactive'));

ALTER TABLE job_queue
    ADD COLUMN last_error_is_safe BOOLEAN NOT NULL DEFAULT false;

CREATE INDEX job_queue_lane_pending_idx
    ON job_queue(queue_name, run_at, created_at)
    WHERE status = 'pending';

ALTER TABLE job_worker_heartbeats
    ADD COLUMN interactive_concurrency INT NOT NULL DEFAULT 0
        CHECK (interactive_concurrency >= 0 AND interactive_concurrency <= concurrency);

ALTER TABLE agent_runs
    ADD COLUMN error_message_is_safe BOOLEAN NOT NULL DEFAULT false;
