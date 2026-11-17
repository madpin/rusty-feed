-- Personal Roundup schedules and bounded run history. Generated articles stay
-- in the shared `roundups` table; these rows contain timing, authorization,
-- delivery, and personal history only.

-- A composite folder key lets schedules/runs cascade when the owning user's
-- folder disappears without ever allowing a cross-user scope reference.
ALTER TABLE user_categories
    ADD CONSTRAINT user_categories_user_id_id_key UNIQUE (user_id, id);

CREATE TABLE roundup_schedules (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    scope_kind      TEXT NOT NULL CHECK (scope_kind IN ('feed', 'folder')),
    feed_id         UUID,
    folder_id       UUID,
    scope_id        UUID GENERATED ALWAYS AS (coalesce(feed_id, folder_id)) STORED,
    period          TEXT NOT NULL CHECK (period IN ('day', 'week')),
    local_hour      SMALLINT NOT NULL CHECK (local_hour BETWEEN 0 AND 23),
    weekday         SMALLINT CHECK (weekday BETWEEN 1 AND 7),
    timezone        TEXT NOT NULL,
    email_delivery  BOOLEAN NOT NULL DEFAULT false,
    enabled         BOOLEAN NOT NULL DEFAULT true,
    last_run_at     TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (scope_kind = 'feed' AND feed_id IS NOT NULL AND folder_id IS NULL)
        OR
        (scope_kind = 'folder' AND feed_id IS NULL AND folder_id IS NOT NULL)
    ),
    CHECK (period <> 'day' OR weekday IS NULL),
    CHECK (period <> 'week' OR weekday IS NOT NULL),
    FOREIGN KEY (user_id, feed_id) REFERENCES user_feeds(user_id, feed_id) ON DELETE CASCADE,
    FOREIGN KEY (user_id, folder_id) REFERENCES user_categories(user_id, id) ON DELETE CASCADE,
    UNIQUE (user_id, scope_kind, scope_id, period)
);

CREATE INDEX roundup_schedules_enabled_idx
    ON roundup_schedules (enabled, last_run_at, created_at);
CREATE INDEX roundup_schedules_user_idx
    ON roundup_schedules (user_id, created_at DESC);
CREATE TRIGGER roundup_schedules_updated_at
    BEFORE UPDATE ON roundup_schedules FOR EACH ROW EXECUTE FUNCTION set_updated_at();

CREATE TABLE roundup_runs (
    id              UUID PRIMARY KEY,
    user_id         UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    schedule_id     UUID REFERENCES roundup_schedules(id) ON DELETE SET NULL,
    scope_kind      TEXT NOT NULL CHECK (scope_kind IN ('feed', 'folder')),
    feed_id         UUID,
    folder_id       UUID,
    scope_id        UUID GENERATED ALWAYS AS (coalesce(feed_id, folder_id)) STORED,
    period          TEXT NOT NULL CHECK (period IN ('day', 'week')),
    roundup_id      UUID REFERENCES roundups(id) ON DELETE CASCADE,
    input_hash      TEXT CHECK (input_hash IS NULL OR char_length(input_hash) = 64),
    model_version   TEXT,
    status          TEXT NOT NULL CHECK (status IN ('pending', 'queued', 'ready', 'skipped', 'failed')),
    skip_reason     TEXT CHECK (skip_reason IS NULL OR char_length(skip_reason) <= 500),
    scheduled_for   TIMESTAMPTZ,
    emailed_at      TIMESTAMPTZ,
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (
        (scope_kind = 'feed' AND feed_id IS NOT NULL AND folder_id IS NULL)
        OR
        (scope_kind = 'folder' AND feed_id IS NULL AND folder_id IS NOT NULL)
    ),
    CHECK ((input_hash IS NULL) = (model_version IS NULL)),
    CHECK (status <> 'ready' OR roundup_id IS NOT NULL),
    CHECK (status NOT IN ('skipped', 'failed') OR skip_reason IS NOT NULL),
    FOREIGN KEY (user_id, feed_id) REFERENCES user_feeds(user_id, feed_id) ON DELETE CASCADE,
    FOREIGN KEY (user_id, folder_id) REFERENCES user_categories(user_id, id) ON DELETE CASCADE,
    UNIQUE (schedule_id, scheduled_for)
);

CREATE INDEX roundup_runs_user_idx ON roundup_runs (user_id, created_at DESC);
CREATE INDEX roundup_runs_retention_idx ON roundup_runs (created_at);
CREATE INDEX roundup_runs_unfinished_idx
    ON roundup_runs (status, created_at)
    WHERE status IN ('pending', 'queued');
CREATE INDEX roundup_runs_email_idx
    ON roundup_runs (status, emailed_at, created_at)
    WHERE status = 'ready' AND emailed_at IS NULL;
CREATE TRIGGER roundup_runs_updated_at
    BEFORE UPDATE ON roundup_runs FOR EACH ROW EXECUTE FUNCTION set_updated_at();
