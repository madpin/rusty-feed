-- First-run setup wizard trigger (docs/PUBLIC_LAUNCH_PLAN.md §8.4). When NULL,
-- the user is routed to /welcome once; "Finish" or "Skip" stamps it.
ALTER TABLE users ADD COLUMN onboarded_at TIMESTAMPTZ;
