-- Per-account login lockout state for signup/login anti-abuse (FND-191).
-- Complements the per-IP auth rate limiter: repeated failed logins for one
-- account trigger a temporary lock that survives restarts.
-- (`email_verified_at` already exists from 0001_init.sql and is now written.)
ALTER TABLE users
    ADD COLUMN failed_login_count INT NOT NULL DEFAULT 0,
    ADD COLUMN locked_until TIMESTAMPTZ;
