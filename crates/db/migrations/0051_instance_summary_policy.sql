CREATE TABLE instance_summary_policy (
    singleton         BOOLEAN PRIMARY KEY DEFAULT true CHECK (singleton),
    enabled           BOOLEAN NOT NULL,
    min_content_chars INT NOT NULL CHECK (min_content_chars BETWEEN 0 AND 1000000),
    updated_at        TIMESTAMPTZ NOT NULL DEFAULT now()
);

CREATE TRIGGER instance_summary_policy_updated_at
    BEFORE UPDATE ON instance_summary_policy
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
