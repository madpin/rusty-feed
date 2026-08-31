-- Track the last time each user was seen making an authenticated request, so the
-- admin console can show a persistent "last accessed" independent of session
-- rows (which are deleted when they expire).
ALTER TABLE users ADD COLUMN last_seen_at TIMESTAMPTZ;
