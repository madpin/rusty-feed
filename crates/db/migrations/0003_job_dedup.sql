-- Atomic job de-duplication (M7): guarantee at most one *active* job per key so
-- a feed with N subscribers still enqueues exactly one fetch (O(1) shared work).
--
-- `dedup_key` is optional; when set, a partial unique index enforces uniqueness
-- among pending/running jobs, so enqueue can use `ON CONFLICT DO NOTHING`.

ALTER TABLE job_queue ADD COLUMN dedup_key TEXT;

CREATE UNIQUE INDEX job_queue_dedup_active_idx
    ON job_queue (dedup_key)
    WHERE dedup_key IS NOT NULL AND status IN ('pending', 'running');
