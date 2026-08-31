-- Record signup consent to the Terms of Service / Privacy Policy (FND-195).
-- Set when a user accepts at registration; supports lawful contact/consent
-- records for a public service (docs/PUBLIC_LAUNCH_PLAN.md §9).
ALTER TABLE users ADD COLUMN tos_accepted_at TIMESTAMPTZ;
