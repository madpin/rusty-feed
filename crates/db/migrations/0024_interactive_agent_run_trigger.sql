-- Interactive jobs have their own queue lane and should retain the same run
-- history as scheduled work. The original constraint predates that lane.
ALTER TABLE agent_runs
    DROP CONSTRAINT agent_runs_trigger_check;

ALTER TABLE agent_runs
    ADD CONSTRAINT agent_runs_trigger_check
        CHECK (trigger IN ('scheduler', 'manual', 'system', 'event', 'interactive'));
