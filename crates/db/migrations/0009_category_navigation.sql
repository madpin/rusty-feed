CREATE INDEX user_categories_user_parent_order_idx
    ON user_categories (user_id, parent_id, sort_order, name);
