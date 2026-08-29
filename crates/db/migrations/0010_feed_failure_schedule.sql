ALTER TABLE feeds ADD COLUMN failure_started_at TIMESTAMPTZ;
ALTER TABLE feeds ALTER COLUMN auto_disable_threshold SET DEFAULT 2147483647;
UPDATE feeds
SET auto_disable_threshold = 2147483647,
    failure_started_at = coalesce(failure_started_at, last_successful_fetch_at, last_fetched_at, created_at),
    health_status = CASE
        WHEN health_status = 'disabled' AND consecutive_failures >= 3 THEN 'error'
        ELSE health_status
    END
WHERE consecutive_failures > 0 OR auto_disable_threshold = 10;
