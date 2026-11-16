-- Retention purge support for finished queue, outbox and run rows (FND-238).
-- The existing queue/outbox indexes are partial on pending/unprocessed rows, so
-- the age-based batch select of the purge had only a sequential scan. These
-- partial indexes cover exactly the rows the purge reads and nothing the hot
-- claim/dispatch paths touch.

CREATE INDEX IF NOT EXISTS job_queue_done_updated_idx
    ON job_queue (updated_at) WHERE status = 'done';

CREATE INDEX IF NOT EXISTS outbox_processed_idx
    ON outbox_events (processed_at) WHERE processed_at IS NOT NULL;

CREATE INDEX IF NOT EXISTS agent_runs_finished_started_idx
    ON agent_runs (started_at) WHERE status IN ('success', 'failed');
