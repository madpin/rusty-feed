-- Treat all pre-existing accounts as already onboarded so upgrades never force
-- the first-run wizard on current users (docs/PUBLIC_LAUNCH_PLAN.md §8.4). Only
-- accounts created after this migration (onboarded_at NULL) see the wizard.
UPDATE users SET onboarded_at = created_at WHERE onboarded_at IS NULL;
