-- Folder names are unique among siblings, not across the user's entire tree.
-- This preserves nested OPML category paths such as Work/News and Personal/News.
ALTER TABLE user_categories
    DROP CONSTRAINT IF EXISTS user_categories_user_id_name_key;

ALTER TABLE user_categories
    ADD CONSTRAINT user_categories_user_parent_name_key
    UNIQUE NULLS NOT DISTINCT (user_id, parent_id, name);
