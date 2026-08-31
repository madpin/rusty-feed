-- Versioned UI assets. CSS is personal; JavaScript is administrator-owned.
CREATE TABLE ui_customizations (
    id          UUID PRIMARY KEY,
    owner_id    UUID REFERENCES users(id) ON DELETE CASCADE,
    kind        TEXT NOT NULL CHECK (kind IN ('css', 'javascript')),
    version     INT NOT NULL CHECK (version > 0),
    content     TEXT NOT NULL,
    enabled     BOOLEAN NOT NULL DEFAULT true,
    created_by  UUID NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((kind = 'css' AND owner_id IS NOT NULL) OR (kind = 'javascript' AND owner_id IS NULL)),
    UNIQUE NULLS NOT DISTINCT (owner_id, kind, version)
);
CREATE INDEX ui_customizations_current_idx
    ON ui_customizations(owner_id, kind, version DESC) WHERE enabled;
