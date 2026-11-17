-- Shared feed/folder Roundups. The input hash covers sorted article IDs; the
-- complete uniqueness key also includes period and prompt/model versions.
-- No user or personal organization columns belong here: identical input sets
-- generated for different subscribers share one row and one provider call.

ALTER TABLE cost_ledger DROP CONSTRAINT cost_ledger_kind_check;
ALTER TABLE cost_ledger ADD CONSTRAINT cost_ledger_kind_check
    CHECK (kind IN ('embedding', 'summary', 'digest', 'roundup'));

CREATE TABLE roundups (
    id              UUID PRIMARY KEY,
    input_hash      TEXT NOT NULL CHECK (char_length(input_hash) = 64),
    period          TEXT NOT NULL CHECK (period IN ('day', 'week')),
    prompt_version  TEXT NOT NULL,
    model_version   TEXT NOT NULL,
    provider        TEXT NOT NULL,
    model           TEXT NOT NULL,
    headline        TEXT NOT NULL,
    body_markdown   TEXT NOT NULL,
    source_ids      UUID[] NOT NULL CHECK (cardinality(source_ids) > 0),
    created_at      TIMESTAMPTZ NOT NULL DEFAULT now(),
    UNIQUE (input_hash, period, prompt_version, model_version)
);
