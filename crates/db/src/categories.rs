use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{DbError, Result};

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct UserCategory {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub sort_order: i32,
    pub collapsed: bool,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct CategoryWithFeedCount {
    pub id: Uuid,
    pub name: String,
    pub parent_id: Option<Uuid>,
    pub sort_order: i32,
    pub collapsed: bool,
    pub feed_count: i64,
}

#[derive(Debug, Clone, sqlx::FromRow)]
pub struct TagWithFeedCount {
    pub name: String,
    pub feed_count: i64,
}

/// How many categories this user has (for quota enforcement, FND-188).
pub async fn count_for_user(pool: &PgPool, user_id: Uuid) -> Result<i64> {
    Ok(
        sqlx::query_scalar("SELECT count(*) FROM user_categories WHERE user_id = $1")
            .bind(user_id)
            .fetch_one(pool)
            .await?,
    )
}

pub async fn create(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
    parent_id: Option<Uuid>,
) -> Result<UserCategory> {
    let category = sqlx::query_as(
        "INSERT INTO user_categories (id, user_id, name, parent_id) \
         SELECT $1, $2, $3, $4 WHERE $4::uuid IS NULL OR EXISTS \
           (SELECT 1 FROM user_categories WHERE id = $4 AND user_id = $2) \
         ON CONFLICT (user_id, parent_id, name) DO NOTHING \
         RETURNING id, name, parent_id, sort_order, collapsed",
    )
    .bind(Uuid::now_v7())
    .bind(user_id)
    .bind(name)
    .bind(parent_id)
    .fetch_optional(pool)
    .await?;
    match category {
        Some(category) => Ok(category),
        None => sqlx::query_as(
            "SELECT id, name, parent_id, sort_order, collapsed FROM user_categories \
             WHERE user_id = $1 AND name = $2 AND parent_id IS NOT DISTINCT FROM $3",
        )
        .bind(user_id)
        .bind(name)
        .bind(parent_id)
        .fetch_optional(pool)
        .await?
        .ok_or(DbError::NotFound),
    }
}

pub async fn list_for_user(pool: &PgPool, user_id: Uuid) -> Result<Vec<UserCategory>> {
    Ok(sqlx::query_as(
        "SELECT id, name, parent_id, sort_order, collapsed FROM user_categories WHERE user_id = $1 \
         ORDER BY sort_order, name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Find one of the caller's folders by its exact display name.
pub async fn find_by_name(
    pool: &PgPool,
    user_id: Uuid,
    name: &str,
) -> Result<Option<UserCategory>> {
    Ok(sqlx::query_as(
        "SELECT id, name, parent_id, sort_order, collapsed FROM user_categories \
         WHERE user_id = $1 AND name = $2 ORDER BY parent_id NULLS FIRST LIMIT 1",
    )
    .bind(user_id)
    .bind(name)
    .fetch_optional(pool)
    .await?)
}

pub async fn list_with_feed_counts(
    pool: &PgPool,
    user_id: Uuid,
) -> Result<Vec<CategoryWithFeedCount>> {
    Ok(sqlx::query_as(
        "WITH RECURSIVE descendants(root_id, id) AS ( \
             SELECT id, id FROM user_categories WHERE user_id = $1 \
             UNION \
             SELECT descendants.root_id, child.id FROM descendants \
             JOIN user_categories child ON child.parent_id = descendants.id AND child.user_id = $1 \
         ) \
         SELECT c.id, c.name, c.parent_id, c.sort_order, c.collapsed, count(DISTINCT uf.id)::bigint AS feed_count \
         FROM user_categories c \
         LEFT JOIN descendants ON descendants.root_id = c.id \
         LEFT JOIN user_feed_categories ufc ON ufc.user_category_id = descendants.id \
         LEFT JOIN user_feeds uf ON uf.id = ufc.user_feed_id AND uf.user_id = $1 \
         WHERE c.user_id = $1 \
         GROUP BY c.id ORDER BY c.sort_order, c.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}

/// Persist one user's collapsed/expanded state for a personal folder.
pub async fn set_collapsed(
    pool: &PgPool,
    user_id: Uuid,
    category_id: Uuid,
    collapsed: bool,
) -> Result<()> {
    let result = sqlx::query(
        "UPDATE user_categories SET collapsed = $3, updated_at = now() \
         WHERE id = $1 AND user_id = $2",
    )
    .bind(category_id)
    .bind(user_id)
    .bind(collapsed)
    .execute(pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(DbError::NotFound);
    }
    Ok(())
}

pub async fn belongs_to_user(pool: &PgPool, user_id: Uuid, category_id: Uuid) -> Result<bool> {
    Ok(
        sqlx::query_scalar::<_, i32>(
            "SELECT 1 FROM user_categories WHERE id = $1 AND user_id = $2",
        )
        .bind(category_id)
        .bind(user_id)
        .fetch_optional(pool)
        .await?
        .is_some(),
    )
}

pub async fn list_tags(pool: &PgPool, user_id: Uuid) -> Result<Vec<TagWithFeedCount>> {
    Ok(sqlx::query_as(
        "SELECT tag.name, count(DISTINCT uf.id)::bigint AS feed_count \
         FROM user_feeds uf CROSS JOIN LATERAL unnest(uf.tags) AS tag(name) \
         WHERE uf.user_id = $1 AND tag.name <> '' GROUP BY tag.name ORDER BY tag.name",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?)
}
