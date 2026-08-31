-- Demo mode + read-only admin (docs/PUBLIC_LAUNCH_PLAN.md §7, §7.6).
--
-- Ephemeral demo users are flagged so the daily reset can delete only them, and
-- a first-class read-only admin role lets support/auditors (and the demo "see as
-- admin" experience) view the admin dashboards without any write access.
ALTER TABLE users ADD COLUMN is_demo BOOLEAN NOT NULL DEFAULT false;
CREATE INDEX users_is_demo_idx ON users (is_demo) WHERE is_demo;

-- Accept the new read-only admin role value.
ALTER TABLE users DROP CONSTRAINT IF EXISTS users_role_check;
ALTER TABLE users
    ADD CONSTRAINT users_role_check
    CHECK (role IN ('admin', 'admin_readonly', 'user', 'guest'));
