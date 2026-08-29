CREATE TABLE feed_catalog_entries (
    id          UUID PRIMARY KEY,
    feed_id     UUID NOT NULL UNIQUE REFERENCES feeds(id) ON DELETE CASCADE,
    title       TEXT NOT NULL,
    description TEXT,
    category    TEXT NOT NULL,
    country     TEXT NOT NULL DEFAULT 'Global',
    language    TEXT NOT NULL DEFAULT 'en',
    featured    BOOLEAN NOT NULL DEFAULT false,
    sort_order  INT NOT NULL DEFAULT 0,
    published   BOOLEAN NOT NULL DEFAULT true,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    CHECK (length(trim(title)) > 0),
    CHECK (length(trim(category)) > 0),
    CHECK (length(trim(country)) > 0),
    CHECK (length(trim(language)) > 0)
);
CREATE INDEX feed_catalog_published_idx ON feed_catalog_entries(published, featured DESC, sort_order, title);
CREATE INDEX feed_catalog_category_country_idx ON feed_catalog_entries(category, country);
CREATE TRIGGER feed_catalog_entries_set_updated_at
    BEFORE UPDATE ON feed_catalog_entries
    FOR EACH ROW EXECUTE FUNCTION set_updated_at();

INSERT INTO feeds (id, url, name, site_url, description) VALUES
    (gen_random_uuid(), 'https://blog.rust-lang.org/feed.xml', 'Rust Blog', 'https://blog.rust-lang.org/', 'Official updates from the Rust Project.'),
    (gen_random_uuid(), 'https://news.ycombinator.com/rss', 'Hacker News', 'https://news.ycombinator.com/', 'Technology and startup links ranked by the community.'),
    (gen_random_uuid(), 'https://science.nasa.gov/feed/', 'NASA Science', 'https://science.nasa.gov/', 'Space, Earth science, and NASA research updates.'),
    (gen_random_uuid(), 'https://export.arxiv.org/rss/cs.AI', 'arXiv Computer Science: AI', 'https://arxiv.org/', 'New artificial intelligence research papers from arXiv.'),
    (gen_random_uuid(), 'http://feeds.bbci.co.uk/news/world/rss.xml', 'BBC News: World', 'https://www.bbc.co.uk/news/world', 'World news headlines from BBC News.'),
    (gen_random_uuid(), 'https://www.theguardian.com/world/rss', 'The Guardian: World', 'https://www.theguardian.com/world', 'World news, analysis, and reporting from The Guardian.')
ON CONFLICT (url) DO NOTHING;

INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'Rust Blog', 'Official updates from the Rust Project.', 'Technology', 'Global', 'en', true, 10
FROM feeds WHERE url = 'https://blog.rust-lang.org/feed.xml'
ON CONFLICT (feed_id) DO NOTHING;
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'Hacker News', 'Technology and startup links ranked by the community.', 'Technology', 'United States', 'en', true, 20
FROM feeds WHERE url = 'https://news.ycombinator.com/rss'
ON CONFLICT (feed_id) DO NOTHING;
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'NASA Science', 'Space, Earth science, and NASA research updates.', 'Science', 'United States', 'en', true, 30
FROM feeds WHERE url = 'https://science.nasa.gov/feed/'
ON CONFLICT (feed_id) DO NOTHING;
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'arXiv Computer Science: AI', 'New artificial intelligence research papers from arXiv.', 'Research', 'Global', 'en', true, 40
FROM feeds WHERE url = 'https://export.arxiv.org/rss/cs.AI'
ON CONFLICT (feed_id) DO NOTHING;
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'BBC News: World', 'World news headlines from BBC News.', 'World News', 'United Kingdom', 'en', true, 50
FROM feeds WHERE url = 'http://feeds.bbci.co.uk/news/world/rss.xml'
ON CONFLICT (feed_id) DO NOTHING;
INSERT INTO feed_catalog_entries (id, feed_id, title, description, category, country, language, featured, sort_order)
SELECT gen_random_uuid(), id, 'The Guardian: World', 'World news, analysis, and reporting from The Guardian.', 'World News', 'United Kingdom', 'en', true, 60
FROM feeds WHERE url = 'https://www.theguardian.com/world/rss'
ON CONFLICT (feed_id) DO NOTHING;
