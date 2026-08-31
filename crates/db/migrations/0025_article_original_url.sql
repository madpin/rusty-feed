-- Preserve publisher-provided article URLs while `articles.url` becomes the
-- canonical privacy/dedup key for newly ingested items. Existing rows retain
-- their current URL in both columns; current publisher-fetch/render paths clean
-- those legacy values dynamically. This avoids destructive, collision-prone
-- migration rewrites.
ALTER TABLE articles ADD COLUMN original_url TEXT;
UPDATE articles SET original_url = url WHERE original_url IS NULL;
ALTER TABLE articles ALTER COLUMN original_url SET NOT NULL;
