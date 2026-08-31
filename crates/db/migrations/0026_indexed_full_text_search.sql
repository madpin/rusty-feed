-- Rebuild the original English-only vector with weighted fields and add a
-- language-neutral vector. Both expressions are fixed, so PostgreSQL can use
-- their GIN indexes safely; runtime configuration selects only between these
-- two known columns rather than interpolating arbitrary regconfig names.
DROP INDEX IF EXISTS articles_search_idx;

ALTER TABLE articles DROP COLUMN search_vector;

ALTER TABLE articles
    ADD COLUMN search_vector tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('english', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('english', coalesce(excerpt, '')), 'B') ||
        setweight(to_tsvector('english', coalesce(content, '')), 'D')
    ) STORED,
    ADD COLUMN search_vector_simple tsvector GENERATED ALWAYS AS (
        setweight(to_tsvector('simple', coalesce(title, '')), 'A') ||
        setweight(to_tsvector('simple', coalesce(excerpt, '')), 'B') ||
        setweight(to_tsvector('simple', coalesce(content, '')), 'D')
    ) STORED;

CREATE INDEX articles_search_idx ON articles USING gin (search_vector);
CREATE INDEX articles_search_simple_idx ON articles USING gin (search_vector_simple);
