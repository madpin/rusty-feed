-- Narration belongs to the one shared public-page copy, never a subscription.
ALTER TABLE articles ADD COLUMN article_page_audio jsonb NOT NULL DEFAULT '[]'::jsonb
    CHECK (jsonb_typeof(article_page_audio) = 'array' AND jsonb_array_length(article_page_audio) <= 8);
