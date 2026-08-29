UPDATE feeds
SET extraction_settings = jsonb_set(extraction_settings, '{mode}', '"rss"'::jsonb, true)
WHERE extraction_settings ->> 'mode' = 'paywalled';
