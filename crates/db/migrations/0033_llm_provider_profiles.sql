-- Managed AI providers. User profiles take precedence over administrator
-- profiles; environment configuration remains the final fallback.
CREATE TABLE llm_provider_profiles (
    id                 UUID PRIMARY KEY,
    owner_user_id      UUID REFERENCES users(id) ON DELETE CASCADE,
    scope              TEXT NOT NULL CHECK (scope IN ('user', 'admin')),
    name               TEXT NOT NULL,
    provider           TEXT NOT NULL CHECK (provider IN ('openai', 'ollama')),
    model              TEXT NOT NULL,
    base_url           TEXT NOT NULL,
    api_key_ciphertext TEXT,
    enabled            BOOLEAN NOT NULL DEFAULT true,
    priority           INT NOT NULL DEFAULT 100 CHECK (priority BETWEEN 0 AND 10000),
    created_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at         TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK ((scope = 'user' AND owner_user_id IS NOT NULL) OR
           (scope = 'admin' AND owner_user_id IS NULL))
);
CREATE INDEX llm_provider_profiles_resolution_idx
    ON llm_provider_profiles(scope, owner_user_id, enabled, priority, created_at);

CREATE TRIGGER llm_provider_profiles_updated_at
    BEFORE UPDATE ON llm_provider_profiles
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();
