-- Shared YouTube metadata cache. One video identifier is enriched once for all users.
CREATE TABLE youtube_video_metadata (
    video_id       TEXT PRIMARY KEY CHECK (char_length(video_id) = 11),
    duration_secs  INT CHECK (duration_secs > 0),
    status         TEXT NOT NULL DEFAULT 'pending' CHECK (status IN ('pending','ready','failed')),
    error_summary  TEXT,
    attempted_at   TIMESTAMPTZ,
    expires_at     TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at     TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX youtube_video_metadata_expiry_idx ON youtube_video_metadata(expires_at);
