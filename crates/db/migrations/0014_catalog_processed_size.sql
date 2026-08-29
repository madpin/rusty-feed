ALTER TABLE catalog_content_probes
    ADD COLUMN average_processed_body_chars INTEGER CHECK (average_processed_body_chars >= 0);
